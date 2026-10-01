//! The command assistant's records: its settings and its answer cache.
//!
//! Both sync like any record, sealed end to end. The settings are one record
//! under a fixed id, so two devices that set them up write the same record and
//! the newer one wins, instead of each adding a second set. The API keys are
//! not part of it: each one is a [`EntityKind::Secret`](uwussh_proto::EntityKind)
//! in the vault, exactly like a host's password, and the settings point at it
//! by id. Without the vault open a key can be forgotten, never read or set.
//!
//! The cache has a fixed number of slots with fixed ids. A new answer takes a
//! free slot or the one used longest ago, and clearing a slot writes it empty
//! rather than deleting it — a tombstone beats every later write, so a deleted
//! slot could never be used again. That keeps the cache's footprint on the
//! server bounded too: at most [`ASSIST_CACHE_SLOTS`] records, however long it
//! is used, which also bounds what a build that does not know these kinds has
//! to page past.

use crate::hosts::PasswordChange;
use crate::sync::parse_uuid;
use crate::vault::{forget_secret, seal_secret};
use crate::{now_ms, tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;
use uwussh_proto::{AssistConfigPayload, AssistProviderPayload};
use zeroize::Zeroizing;

/// How many answers the cache keeps, on this device and on the server.
pub const ASSIST_CACHE_SLOTS: usize = 200;

/// Fixed ids, version-8 UUIDs that no random id can meet: the settings at the
/// base, the cache slots after it.
const BASE: u128 = 0x7577_7373_6801_8000_a000_0000_0000_0000;

const MAX_MODEL_CHARS: usize = 200;
const MAX_URL_CHARS: usize = 2048;
const MAX_KEY_CHARS: usize = 4096;
const MAX_TEXT_CHARS: usize = 4000;

/// The id the assistant's settings live under, in every vault.
pub fn assist_config_id() -> Uuid {
    Uuid::from_u128(BASE)
}

/// The id of one cache slot.
pub fn assist_cache_slot_id(slot: usize) -> Uuid {
    Uuid::from_u128(BASE + 1 + slot as u128)
}

/// What the settings page shows: everything but the keys themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistSettings {
    /// The provider in use, or empty for none.
    pub provider: String,
    pub providers: BTreeMap<String, AssistProviderSettings>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistProviderSettings {
    pub model: String,
    pub base_url: Option<String>,
    /// A key is kept in the vault for this provider.
    pub has_key: bool,
}

/// What the settings page saves for one provider.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistProviderDraft {
    /// The provider to use from now on; empty switches the assistant off.
    pub active: String,
    /// The provider these fields belong to.
    pub kind: String,
    pub model: String,
    pub base_url: Option<String>,
    #[serde(default)]
    pub key: PasswordChange,
}

/// One cached answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistCacheEntry {
    pub id: Uuid,
    pub platform: String,
    pub request: String,
    pub normalized: String,
    pub command: String,
    pub explanation: String,
    pub dangerous: bool,
    pub created_ms: u64,
    /// When this device last used it, 0 for never. Local.
    pub used_ms: u64,
    /// How often this device used it. Local.
    pub hits: u64,
}

/// An answer on its way into the cache.
#[derive(Debug, Clone, Default)]
pub struct NewCacheEntry {
    pub platform: String,
    pub request: String,
    pub normalized: String,
    pub command: String,
    pub explanation: String,
    pub dangerous: bool,
}

fn invalid(field: &'static str, problem: &'static str) -> StoreError {
    StoreError::Invalid { field, problem }
}

fn too_long(text: &str, max: usize) -> bool {
    text.chars().count() > max
}

/// The settings as stored, or the empty ones.
fn load_config(conn: &rusqlite::Connection) -> Result<AssistConfigPayload> {
    let body: Option<String> = conn
        .query_row(
            "SELECT body FROM assist_config WHERE id = ?1 AND deleted = 0",
            [assist_config_id().to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(body
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default())
}

fn write_config(tx: &Transaction, device: u32, config: &AssistConfigPayload) -> Result<()> {
    let body = serde_json::to_string(config).map_err(|_| invalid("record", "unserialisable"))?;
    let clock = tick(tx, device)?;
    tx.execute(
        "INSERT INTO assist_config
            (id, vault_id, body, hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1)
         ON CONFLICT (id) DO UPDATE SET
            body = excluded.body, hlc_wall_ms = excluded.hlc_wall_ms,
            hlc_counter = excluded.hlc_counter, hlc_device = excluded.hlc_device,
            deleted = 0, dirty = 1, rev = rev + 1",
        params![
            assist_config_id().to_string(),
            vault_id(tx)?,
            body,
            clock.wall_ms as i64,
            clock.counter,
            clock.device,
        ],
    )?;
    Ok(())
}

fn secret_alive(conn: &rusqlite::Connection, id: Uuid) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM secrets WHERE id = ?1 AND deleted = 0 AND length(blob) > 0)",
        [id.to_string()],
        |row| row.get(0),
    )?)
}

fn settings_of(
    conn: &rusqlite::Connection,
    config: &AssistConfigPayload,
) -> Result<AssistSettings> {
    let mut providers = BTreeMap::new();
    for (kind, provider) in &config.providers {
        let has_key = match provider.key_secret_id {
            Some(id) => secret_alive(conn, id)?,
            None => false,
        };
        providers.insert(
            kind.clone(),
            AssistProviderSettings {
                model: provider.model.clone(),
                base_url: provider.base_url.clone(),
                has_key,
            },
        );
    }
    Ok(AssistSettings {
        provider: config.provider.clone(),
        providers,
    })
}

fn entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, AssistCacheEntry)> {
    let id: String = row.get(0)?;
    let dangerous: bool = row.get(6)?;
    let created: i64 = row.get(7)?;
    let used: i64 = row.get(8)?;
    let hits: i64 = row.get(9)?;
    Ok((
        id,
        AssistCacheEntry {
            id: Uuid::nil(),
            platform: row.get(1)?,
            request: row.get(2)?,
            normalized: row.get(3)?,
            command: row.get(4)?,
            explanation: row.get(5)?,
            dangerous,
            created_ms: created as u64,
            used_ms: used as u64,
            hits: hits as u64,
        },
    ))
}

const ENTRY_COLUMNS: &str = "id, platform, request, normalized, command, explanation, dangerous, \
                             created_ms, used_ms, hits";

impl Store {
    /// The assistant's settings, without the keys. Readable with the vault
    /// locked: whether a key is kept is not a secret.
    pub fn assist_settings(&self) -> Result<AssistSettings> {
        let conn = self.conn.lock();
        let config = load_config(&conn)?;
        settings_of(&conn, &config)
    }

    /// Save one provider's settings and which provider is in use.
    ///
    /// A new key is sealed in the vault, which must be open for that; the key
    /// it replaces is forgotten, ciphertext and all, like a replaced password.
    pub fn save_assist_settings(&self, draft: AssistProviderDraft) -> Result<AssistSettings> {
        let kind = draft.kind.trim();
        if kind.is_empty() || too_long(kind, 40) {
            return Err(invalid("provider", "invalid"));
        }
        let model = draft.model.trim();
        if too_long(model, MAX_MODEL_CHARS) || model.chars().any(char::is_control) {
            return Err(invalid("model", "invalid"));
        }
        let base_url = draft
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty());
        if base_url.is_some_and(|url| too_long(url, MAX_URL_CHARS)) {
            return Err(invalid("baseUrl", "too-long"));
        }
        if let PasswordChange::Set { value } = &draft.key {
            let key = value.expose().trim();
            if key.is_empty() || too_long(key, MAX_KEY_CHARS) || key.chars().any(char::is_control) {
                return Err(invalid("apiKey", "invalid"));
            }
        }

        // Connection first, vault second: the order every other path takes.
        let mut conn = self.conn.lock();
        let guard = self.vault.lock();
        if matches!(draft.key, PasswordChange::Set { .. }) && guard.is_none() {
            return Err(StoreError::VaultLocked);
        }
        let tx = conn.transaction()?;
        let mut config = load_config(&tx)?;
        let previous = config.providers.get(kind).cloned().unwrap_or_default();
        let key_secret_id = match &draft.key {
            PasswordChange::Keep => previous.key_secret_id,
            PasswordChange::Set { value } => {
                let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
                if let Some(old) = previous.key_secret_id {
                    forget_secret(&tx, self.device, &old.to_string())?;
                }
                let id = seal_secret(&tx, self.device, vault, value.expose().trim().as_bytes())?;
                Some(parse_uuid(&id)?)
            }
            PasswordChange::Forget => {
                if let Some(old) = previous.key_secret_id {
                    forget_secret(&tx, self.device, &old.to_string())?;
                }
                None
            }
        };
        config.provider = draft.active.trim().to_string();
        config.providers.insert(
            kind.to_string(),
            AssistProviderPayload {
                model: model.to_string(),
                base_url: base_url.map(str::to_string),
                key_secret_id,
                extra: previous.extra,
            },
        );
        write_config(&tx, self.device, &config)?;
        let settings = settings_of(&tx, &config)?;
        tx.commit()?;
        drop(guard);
        if matches!(
            draft.key,
            PasswordChange::Set { .. } | PasswordChange::Forget
        ) {
            crate::vault::truncate_wal(&conn);
        }
        Ok(settings)
    }

    /// The API key kept for a provider, opened. `Ok(None)` when there is none
    /// — or when it has not arrived from the device that set it yet.
    /// [`StoreError::VaultLocked`] when there is one and the vault is closed.
    pub fn assist_api_key(&self, kind: &str) -> Result<Option<Zeroizing<String>>> {
        let id = {
            let conn = self.conn.lock();
            let config = load_config(&conn)?;
            match config
                .providers
                .get(kind)
                .and_then(|provider| provider.key_secret_id)
            {
                Some(id) if secret_alive(&conn, id)? => id,
                _ => return Ok(None),
            }
        };
        let revealed = self.reveal_secret(id)?;
        let text =
            String::from_utf8(revealed.to_vec()).map_err(|_| invalid("apiKey", "invalid"))?;
        Ok(Some(Zeroizing::new(text)))
    }

    /// Cached answers, newest use first. With a platform, only that
    /// platform's: a command for bash is never offered to PowerShell.
    pub fn assist_cache_entries(&self, platform: Option<&str>) -> Result<Vec<AssistCacheEntry>> {
        let conn = self.conn.lock();
        let sql = format!(
            "SELECT {ENTRY_COLUMNS} FROM assist_cache
              WHERE deleted = 0 AND command <> '' AND (?1 IS NULL OR platform = ?1)
              ORDER BY max(used_ms, created_ms) DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([platform], entry_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, entry)| {
                Ok(AssistCacheEntry {
                    id: parse_uuid(&id)?,
                    ..entry
                })
            })
            .collect()
    }

    /// Keep an answer. The same question on the same platform takes the slot
    /// it had; otherwise a free slot, otherwise the one used longest ago.
    pub fn put_assist_cache(&self, entry: NewCacheEntry) -> Result<AssistCacheEntry> {
        if entry.command.trim().is_empty() {
            return Err(invalid("command", "empty"));
        }
        let cut = |text: &str| -> String { text.chars().take(MAX_TEXT_CHARS).collect() };
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        let same: Option<String> = tx
            .query_row(
                "SELECT id FROM assist_cache
                  WHERE deleted = 0 AND command <> '' AND platform = ?1 AND normalized = ?2
                  ORDER BY max(used_ms, created_ms) DESC LIMIT 1",
                params![entry.platform, entry.normalized],
                |row| row.get(0),
            )
            .optional()?;
        let slot = match same {
            Some(id) => parse_uuid(&id)?,
            None => free_or_oldest_slot(&tx)?,
        };

        let clock = tick(&tx, self.device)?;
        let now = now_ms() as i64;
        tx.execute(
            "INSERT INTO assist_cache
                (id, vault_id, platform, request, normalized, command, explanation, dangerous,
                 created_ms, used_ms, hits, hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, 0, ?10, ?11, ?12, 0, 1)
             ON CONFLICT (id) DO UPDATE SET
                platform = excluded.platform, request = excluded.request,
                normalized = excluded.normalized, command = excluded.command,
                explanation = excluded.explanation, dangerous = excluded.dangerous,
                created_ms = excluded.created_ms, used_ms = excluded.used_ms, hits = 0,
                hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                hlc_device = excluded.hlc_device, deleted = 0, dirty = 1,
                sync_extra = NULL, rev = rev + 1",
            params![
                slot.to_string(),
                vault_id(&tx)?,
                cut(&entry.platform),
                cut(&entry.request),
                cut(&entry.normalized),
                cut(&entry.command),
                cut(&entry.explanation),
                entry.dangerous,
                now,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        let stored = tx.query_row(
            &format!("SELECT {ENTRY_COLUMNS} FROM assist_cache WHERE id = ?1"),
            [slot.to_string()],
            entry_from_row,
        )?;
        tx.commit()?;
        Ok(AssistCacheEntry {
            id: slot,
            ..stored.1
        })
    }

    /// Note that an answer was used here. Local only: a hit is not worth a
    /// sync on every device.
    pub fn touch_assist_cache(&self, id: Uuid) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE assist_cache SET used_ms = ?2, hits = hits + 1 WHERE id = ?1",
            params![id.to_string(), now_ms() as i64],
        )?;
        Ok(())
    }

    /// Empty one slot, on every device.
    pub fn clear_assist_cache_entry(&self, id: Uuid) -> Result<bool> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let cleared = clear_slots(&tx, self.device, Some(id))?;
        tx.commit()?;
        Ok(cleared > 0)
    }

    /// Empty the whole cache, on every device. Returns how many answers went.
    pub fn clear_assist_cache(&self) -> Result<usize> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let cleared = clear_slots(&tx, self.device, None)?;
        tx.commit()?;
        Ok(cleared)
    }
}

fn clear_slots(tx: &Transaction, device: u32, only: Option<Uuid>) -> Result<usize> {
    let ids: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM assist_cache
              WHERE deleted = 0 AND command <> '' AND (?1 IS NULL OR id = ?1)",
        )?;
        let rows = stmt
            .query_map([only.map(|id| id.to_string())], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for id in &ids {
        let clock = tick(tx, device)?;
        tx.execute(
            "UPDATE assist_cache
                SET platform = '', request = '', normalized = '', command = '',
                    explanation = '', dangerous = 0, used_ms = 0, hits = 0,
                    sync_extra = NULL, dirty = 1, rev = rev + 1,
                    hlc_wall_ms = ?2, hlc_counter = ?3, hlc_device = ?4
              WHERE id = ?1",
            params![id, clock.wall_ms as i64, clock.counter, clock.device],
        )?;
    }
    Ok(ids.len())
}

/// The slot a new answer goes into: the first one never used or emptied,
/// else the one whose answer was used (or made) longest ago.
fn free_or_oldest_slot(tx: &Transaction) -> Result<Uuid> {
    let mut taken = BTreeMap::<Uuid, i64>::new();
    {
        let mut stmt = tx.prepare(
            "SELECT id, max(used_ms, created_ms) FROM assist_cache
              WHERE deleted = 0 AND command <> ''",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, used) in rows {
            if let Ok(id) = Uuid::parse_str(&id) {
                taken.insert(id, used);
            }
        }
    }
    let slots = (0..ASSIST_CACHE_SLOTS).map(assist_cache_slot_id);
    if let Some(free) = slots.clone().find(|id| !taken.contains_key(id)) {
        return Ok(free);
    }
    Ok(slots
        .min_by_key(|id| taken.get(id).copied().unwrap_or(0))
        .unwrap_or_else(|| assist_cache_slot_id(0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretText;
    use uwussh_vault::KdfParams;

    fn store() -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .create_vault_with(b"pw", KdfParams::INSECURE_FOR_TESTS)
            .unwrap();
        store
    }

    fn entry(platform: &str, normalized: &str, command: &str) -> NewCacheEntry {
        NewCacheEntry {
            platform: platform.into(),
            request: normalized.into(),
            normalized: normalized.into(),
            command: command.into(),
            explanation: "erklärt".into(),
            dangerous: false,
        }
    }

    fn draft(key: PasswordChange) -> AssistProviderDraft {
        AssistProviderDraft {
            active: "anthropic".into(),
            kind: "anthropic".into(),
            model: "claude-haiku-4-5".into(),
            base_url: None,
            key,
        }
    }

    #[test]
    fn the_fixed_ids_are_version_8_and_distinct() {
        assert_eq!(assist_config_id().get_version_num(), 8);
        assert_eq!(assist_config_id().get_variant(), uuid::Variant::RFC4122);
        assert_eq!(assist_cache_slot_id(0).get_version_num(), 8);
        assert_ne!(assist_cache_slot_id(0), assist_config_id());
        assert_ne!(
            assist_cache_slot_id(0),
            assist_cache_slot_id(ASSIST_CACHE_SLOTS - 1)
        );
    }

    #[test]
    fn an_api_key_is_sealed_in_the_vault_and_never_in_the_settings() {
        let store = store();
        let key = "sk-ant-test-0123456789"; // gitleaks:allow
        let settings = store
            .save_assist_settings(draft(PasswordChange::Set {
                value: SecretText::new(key),
            }))
            .unwrap();
        assert!(settings.providers["anthropic"].has_key);
        assert_eq!(settings.provider, "anthropic");

        let body: String = store
            .conn
            .lock()
            .query_row("SELECT body FROM assist_config", [], |row| row.get(0))
            .unwrap();
        assert!(!body.contains(key), "the key must not be in the settings");
        let blobs: Vec<Vec<u8>> = {
            let conn = store.conn.lock();
            let mut stmt = conn.prepare("SELECT blob FROM secrets").unwrap();
            let rows = stmt
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            rows
        };
        assert!(blobs
            .iter()
            .all(|blob| !blob.windows(key.len()).any(|w| w == key.as_bytes())));
        assert_eq!(
            store
                .assist_api_key("anthropic")
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some(key)
        );

        // Locked: the key is there but cannot be read, and a new one cannot be
        // set; the model can still change.
        store.lock_vault();
        assert!(matches!(
            store.assist_api_key("anthropic"),
            Err(StoreError::VaultLocked)
        ));
        assert!(matches!(
            store.save_assist_settings(draft(PasswordChange::Set {
                value: SecretText::new("other"),
            })),
            Err(StoreError::VaultLocked)
        ));
        let kept = store
            .save_assist_settings(draft(PasswordChange::Keep))
            .unwrap();
        assert!(kept.providers["anthropic"].has_key);

        // Forgetting works locked, and leaves nothing to open.
        let gone = store
            .save_assist_settings(draft(PasswordChange::Forget))
            .unwrap();
        assert!(!gone.providers["anthropic"].has_key);
        assert_eq!(store.assist_api_key("anthropic").unwrap(), None);
    }

    #[test]
    fn a_replaced_key_is_forgotten() {
        let store = store();
        for key in ["first-key", "second-key"] {
            store
                .save_assist_settings(draft(PasswordChange::Set {
                    value: SecretText::new(key),
                }))
                .unwrap();
        }
        let live: i64 = store
            .conn
            .lock()
            .query_row(
                "SELECT count(*) FROM secrets WHERE deleted = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live, 1);
        assert_eq!(
            store
                .assist_api_key("anthropic")
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("second-key")
        );
    }

    #[test]
    fn the_cache_is_per_platform_and_reuses_its_slots() {
        let store = store();
        let bash = store
            .put_assist_cache(entry(
                "linux/bash/apt",
                "benutzer list",
                "cut -d: -f1 /etc/passwd",
            ))
            .unwrap();
        store
            .put_assist_cache(entry(
                "windows/powershell",
                "benutzer list",
                "Get-LocalUser",
            ))
            .unwrap();
        let only_bash = store.assist_cache_entries(Some("linux/bash/apt")).unwrap();
        assert_eq!(only_bash.len(), 1);
        assert_eq!(only_bash[0].command, "cut -d: -f1 /etc/passwd");

        // Asking again on the same platform overwrites, it does not add.
        let again = store
            .put_assist_cache(entry("linux/bash/apt", "benutzer list", "getent passwd"))
            .unwrap();
        assert_eq!(again.id, bash.id);
        assert_eq!(store.assist_cache_entries(None).unwrap().len(), 2);

        store.touch_assist_cache(again.id).unwrap();
        assert_eq!(
            store.assist_cache_entries(Some("linux/bash/apt")).unwrap()[0].hits,
            1
        );

        assert!(store.clear_assist_cache_entry(again.id).unwrap());
        assert_eq!(store.assist_cache_entries(None).unwrap().len(), 1);
        // The emptied slot is the next one used.
        let next = store
            .put_assist_cache(entry("linux/bash/apt", "prozess list", "ps aux"))
            .unwrap();
        assert_eq!(next.id, bash.id);
        assert_eq!(store.clear_assist_cache().unwrap(), 2);
        assert!(store.assist_cache_entries(None).unwrap().is_empty());
    }

    #[test]
    fn a_full_cache_gives_up_the_answer_used_longest_ago() {
        let store = store();
        let mut first = None;
        for n in 0..ASSIST_CACHE_SLOTS {
            let put = store
                .put_assist_cache(entry("linux/sh", &format!("frage {n}"), "true"))
                .unwrap();
            first.get_or_insert(put.id);
        }
        // Make every slot but the first look recently used.
        store
            .conn
            .lock()
            .execute(
                "UPDATE assist_cache SET used_ms = 9999999999999 WHERE id <> ?1",
                [first.unwrap().to_string()],
            )
            .unwrap();
        let replaced = store
            .put_assist_cache(entry("linux/sh", "neue frage", "false"))
            .unwrap();
        assert_eq!(Some(replaced.id), first);
        assert_eq!(
            store.assist_cache_entries(None).unwrap().len(),
            ASSIST_CACHE_SLOTS
        );
    }
}
