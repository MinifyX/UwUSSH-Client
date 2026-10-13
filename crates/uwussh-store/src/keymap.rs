//! The user's own keyboard shortcuts.
//!
//! A keymap is one record under a fixed id, like the command assistant's
//! settings, so two devices that change shortcuts write the same record and
//! the newer one wins. There are two of them, one per family of keyboards:
//! `mac` and `other` (Windows and Linux). A shortcut with ⌘ means nothing on a
//! PC and one with Ctrl is usually the wrong one on a Mac, so each family
//! keeps its own and both sync everywhere.
//!
//! Besides rebinding what the app can already do, a keymap holds the user's
//! own actions: texts a shortcut types into the terminal. Such a text is as
//! likely a password as a command, so it is never part of the record: each is
//! a [`EntityKind::Secret`](uwussh_proto::EntityKind) in the vault, exactly
//! like a host's password, and the record points at it by id. Without the
//! vault open a text can be forgotten, never read or set — the shortcuts
//! themselves can be read and changed either way.

use crate::assist::secret_alive;
use crate::hosts::PasswordChange;
use crate::sync::parse_uuid;
use crate::vault::{forget_secret, seal_secret};
use crate::{tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
use uwussh_proto::{KeymapPayload, KeymapTextPayload};
use zeroize::Zeroizing;

/// One key with its modifiers, as the page sends and reads it.
pub use uwussh_proto::ChordPayload as Chord;

/// Fixed ids, version-8 UUIDs that no random id can meet. One step beside the
/// command assistant's (`…6801…`), so its cache slots can never reach them.
const BASE: u128 = 0x7577_7373_6802_8000_a000_0000_0000_0000;

/// The families a keymap exists for.
pub const KEYMAP_FAMILIES: [&str; 2] = ["mac", "other"];

const MAX_ACTIONS: usize = 300;
const MAX_ACTION_CHARS: usize = 80;
const MAX_CHORDS: usize = 8;
const MAX_CODE_CHARS: usize = 32;
const MAX_TEXTS: usize = 100;
const MAX_LABEL_CHARS: usize = 100;
const MAX_TEXT_CHARS: usize = 4000;

/// What an action id starts with when it types one of the user's texts.
const TEXT_ACTION: &str = "text:";

/// The id the keymap of a family lives under, in every vault. `None` for a
/// family there is no keymap for.
pub fn keymap_id(family: &str) -> Option<Uuid> {
    KEYMAP_FAMILIES
        .iter()
        .position(|known| *known == family)
        .map(|index| Uuid::from_u128(BASE + index as u128))
}

/// What the settings page shows: every shortcut, and the texts without what
/// they type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeymapView {
    /// Action id to its chords. An action that is missing keeps the shortcut
    /// the app ships with; one with no chords has none.
    pub bindings: BTreeMap<String, Vec<Chord>>,
    /// In the order they were saved in.
    pub texts: Vec<KeymapText>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeymapText {
    pub id: Uuid,
    pub label: String,
    /// Press Enter after typing it.
    pub enter: bool,
    /// A text is kept in the vault for it. `false` for one that was never
    /// set, was forgotten, or has not arrived from the device that set it.
    pub has_text: bool,
}

/// What the settings page saves: the whole keymap of one family.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeymapDraft {
    #[serde(default)]
    pub bindings: BTreeMap<String, Vec<Chord>>,
    #[serde(default)]
    pub texts: Vec<KeymapTextDraft>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeymapTextDraft {
    pub id: Uuid,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub enter: bool,
    #[serde(default)]
    pub text: PasswordChange,
}

fn invalid(field: &'static str, problem: &'static str) -> StoreError {
    StoreError::Invalid { field, problem }
}

fn family_id(family: &str) -> Result<Uuid> {
    keymap_id(family).ok_or_else(|| invalid("family", "unknown"))
}

/// Action ids are the app's own names (`tab:new`, `text:<uuid>`), never
/// something a user types, so the alphabet is as small as they are.
fn valid_action(id: &str) -> bool {
    (1..=MAX_ACTION_CHARS).contains(&id.len())
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b':' | b'_' | b'-')
        })
}

fn valid_code(code: &str) -> bool {
    (1..=MAX_CODE_CHARS).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The text of the user's action a binding is for, when it is for one.
/// `Some(None)` is a `text:` action whose id does not read.
fn text_of_action(action: &str) -> Option<Option<Uuid>> {
    action
        .strip_prefix(TEXT_ACTION)
        .map(|id| Uuid::parse_str(id).ok())
}

/// A text as it is kept: line ends as `\n`, and nothing in it a terminal
/// would take for a control key.
fn normalised_text(value: &str) -> Result<Zeroizing<String>> {
    let text = Zeroizing::new(value.replace("\r\n", "\n"));
    let chars = text.chars().count();
    if chars == 0 {
        return Err(invalid("text", "empty"));
    }
    if chars > MAX_TEXT_CHARS {
        return Err(invalid("text", "too-long"));
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(invalid("text", "invalid"));
    }
    Ok(text)
}

/// The keymap as stored, or the empty one.
fn load(conn: &rusqlite::Connection, id: Uuid) -> Result<KeymapPayload> {
    let body: Option<String> = conn
        .query_row(
            "SELECT body FROM keymaps WHERE id = ?1 AND deleted = 0",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(body
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default())
}

fn write(tx: &Transaction, device: u32, id: Uuid, keymap: &KeymapPayload) -> Result<()> {
    let body = serde_json::to_string(keymap).map_err(|_| invalid("record", "unserialisable"))?;
    let clock = tick(tx, device)?;
    tx.execute(
        "INSERT INTO keymaps
            (id, vault_id, body, hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1)
         ON CONFLICT (id) DO UPDATE SET
            body = excluded.body, hlc_wall_ms = excluded.hlc_wall_ms,
            hlc_counter = excluded.hlc_counter, hlc_device = excluded.hlc_device,
            deleted = 0, dirty = 1, rev = rev + 1",
        params![
            id.to_string(),
            vault_id(tx)?,
            body,
            clock.wall_ms as i64,
            clock.counter,
            clock.device,
        ],
    )?;
    Ok(())
}

fn view_of(conn: &rusqlite::Connection, keymap: &KeymapPayload) -> Result<KeymapView> {
    let mut texts = Vec::with_capacity(keymap.texts.len());
    for (id, text) in &keymap.texts {
        // An entry whose id does not read cannot be bound to anything; the
        // next save drops it.
        let Ok(id) = Uuid::parse_str(id) else {
            continue;
        };
        let has_text = match text.secret_id {
            Some(secret) => secret_alive(conn, secret)?,
            None => false,
        };
        texts.push((
            text.position,
            KeymapText {
                id,
                label: text.label.clone(),
                enter: text.enter,
                has_text,
            },
        ));
    }
    texts.sort_by_key(|(position, text)| (*position, text.id));
    Ok(KeymapView {
        bindings: keymap.bindings.clone(),
        texts: texts.into_iter().map(|(_, text)| text).collect(),
    })
}

impl Store {
    /// The keymap of one family (`mac` or `other`), without what its texts
    /// type. Readable with the vault locked: which keys do what is not a
    /// secret, and whether a text is kept is not one either.
    pub fn keymap(&self, family: &str) -> Result<KeymapView> {
        let id = family_id(family)?;
        let conn = self.conn.lock();
        let keymap = load(&conn, id)?;
        view_of(&conn, &keymap)
    }

    /// Replace the keymap of one family with what the page holds.
    ///
    /// A text that is no longer in the draft goes, and what it typed is
    /// forgotten with it, ciphertext and all. A new text is sealed in the
    /// vault, which must be open for that; the one it replaces is forgotten
    /// like a replaced password. A binding for a text that does not exist is
    /// dropped rather than refused: the page removes a text and its shortcut
    /// in one go and need not get the order right.
    pub fn save_keymap(&self, family: &str, draft: KeymapDraft) -> Result<KeymapView> {
        let id = family_id(family)?;

        if draft.bindings.len() > MAX_ACTIONS {
            return Err(invalid("bindings", "too-many"));
        }
        for (action, chords) in &draft.bindings {
            if !valid_action(action) {
                return Err(invalid("action", "invalid"));
            }
            if chords.len() > MAX_CHORDS {
                return Err(invalid("chords", "too-many"));
            }
            if !chords.iter().all(|chord| valid_code(&chord.code)) {
                return Err(invalid("code", "invalid"));
            }
        }
        if draft.texts.len() > MAX_TEXTS {
            return Err(invalid("texts", "too-many"));
        }
        let mut seen = BTreeSet::new();
        let mut texts = Vec::with_capacity(draft.texts.len());
        for text in &draft.texts {
            if !seen.insert(text.id) {
                return Err(invalid("texts", "duplicate"));
            }
            let label = text.label.trim();
            if label.chars().count() > MAX_LABEL_CHARS {
                return Err(invalid("label", "too-long"));
            }
            if label.chars().any(char::is_control) {
                return Err(invalid("label", "invalid"));
            }
            let set = match &text.text {
                PasswordChange::Set { value } => Some(normalised_text(value.expose())?),
                PasswordChange::Keep | PasswordChange::Forget => None,
            };
            texts.push((text, label, set));
        }
        let sets_a_text = texts.iter().any(|(_, _, set)| set.is_some());

        // Connection first, vault second: the order every other path takes.
        let mut conn = self.conn.lock();
        let guard = self.vault.lock();
        if sets_a_text && guard.is_none() {
            return Err(StoreError::VaultLocked);
        }
        let tx = conn.transaction()?;
        let mut keymap = load(&tx, id)?;
        let mut previous = std::mem::take(&mut keymap.texts);
        let mut touched_a_secret = false;

        for (position, (text, label, set)) in texts.into_iter().enumerate() {
            let old = previous.remove(&text.id.to_string()).unwrap_or_default();
            let secret_id = match (&text.text, set) {
                (PasswordChange::Set { .. }, Some(value)) => {
                    let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
                    if let Some(old) = old.secret_id {
                        forget_secret(&tx, self.device, &old.to_string())?;
                    }
                    touched_a_secret = true;
                    let sealed = seal_secret(&tx, self.device, vault, value.as_bytes())?;
                    Some(parse_uuid(&sealed)?)
                }
                (PasswordChange::Forget, _) => {
                    if let Some(old) = old.secret_id {
                        forget_secret(&tx, self.device, &old.to_string())?;
                        touched_a_secret = true;
                    }
                    None
                }
                _ => old.secret_id,
            };
            keymap.texts.insert(
                text.id.to_string(),
                KeymapTextPayload {
                    label: label.to_string(),
                    enter: text.enter,
                    secret_id,
                    position: position as u32,
                    extra: old.extra,
                },
            );
        }
        // Whatever is left was removed on the page.
        for gone in previous.into_values() {
            if let Some(old) = gone.secret_id {
                forget_secret(&tx, self.device, &old.to_string())?;
                touched_a_secret = true;
            }
        }

        keymap.bindings = draft
            .bindings
            .into_iter()
            .filter(|(action, _)| match text_of_action(action) {
                None => true,
                Some(Some(text)) => keymap.texts.contains_key(&text.to_string()),
                Some(None) => false,
            })
            .collect();

        write(&tx, self.device, id, &keymap)?;
        let view = view_of(&tx, &keymap)?;
        tx.commit()?;
        drop(guard);
        if touched_a_secret {
            crate::vault::truncate_wal(&conn);
        }
        Ok(view)
    }

    /// What one of the user's texts types, opened, and whether Enter follows.
    /// `Ok(None)` when there is no such text or nothing is kept for it — or
    /// it has not arrived from the device that set it yet.
    /// [`StoreError::VaultLocked`] when there is one and the vault is closed.
    pub fn keymap_text(
        &self,
        family: &str,
        text_id: Uuid,
    ) -> Result<Option<(Zeroizing<String>, bool)>> {
        let id = family_id(family)?;
        let (secret, enter) = {
            let conn = self.conn.lock();
            let keymap = load(&conn, id)?;
            let Some(text) = keymap.texts.get(&text_id.to_string()) else {
                return Ok(None);
            };
            match text.secret_id {
                Some(secret) if secret_alive(&conn, secret)? => (secret, text.enter),
                _ => return Ok(None),
            }
        };
        let revealed = self.reveal_secret(secret)?;
        let text = String::from_utf8(revealed.to_vec()).map_err(|_| invalid("text", "invalid"))?;
        Ok(Some((Zeroizing::new(text), enter)))
    }
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

    fn chord(mods: &str, code: &str) -> Chord {
        Chord {
            ctrl: mods.contains('c'),
            shift: mods.contains('s'),
            alt: mods.contains('a'),
            meta: mods.contains('m'),
            code: code.into(),
        }
    }

    fn bindings(entries: &[(&str, &[Chord])]) -> BTreeMap<String, Vec<Chord>> {
        entries
            .iter()
            .map(|(action, chords)| (action.to_string(), chords.to_vec()))
            .collect()
    }

    fn text(id: Uuid, label: &str, change: PasswordChange) -> KeymapTextDraft {
        KeymapTextDraft {
            id,
            label: label.into(),
            enter: true,
            text: change,
        }
    }

    fn set(value: &str) -> PasswordChange {
        PasswordChange::Set {
            value: SecretText::new(value),
        }
    }

    fn live_secrets(store: &Store) -> i64 {
        store
            .conn
            .lock()
            .query_row(
                "SELECT count(*) FROM secrets WHERE deleted = 0",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn is_invalid(result: Result<KeymapView>, field: &str) -> bool {
        matches!(result, Err(StoreError::Invalid { field: found, .. }) if found == field)
    }

    #[test]
    fn the_fixed_ids_are_version_8_and_meet_nothing_else() {
        let mac = keymap_id("mac").unwrap();
        let other = keymap_id("other").unwrap();
        assert_ne!(mac, other);
        for id in [mac, other] {
            assert_eq!(id.get_version_num(), 8);
            assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
            assert_ne!(id, crate::assist_config_id());
            assert!((0..crate::ASSIST_CACHE_SLOTS).all(|n| crate::assist_cache_slot_id(n) != id));
        }
        assert_eq!(keymap_id("linux"), None);
    }

    #[test]
    fn a_keymap_reads_back_as_saved_and_each_family_keeps_its_own() {
        let store = store();
        assert_eq!(store.keymap("mac").unwrap(), KeymapView::default());

        let saved = store
            .save_keymap(
                "mac",
                KeymapDraft {
                    bindings: bindings(&[
                        ("tab:new", &[chord("m", "KeyT"), chord("cs", "KeyT")]),
                        ("terminal:clear", &[]),
                    ]),
                    texts: vec![],
                },
            )
            .unwrap();
        assert_eq!(saved.bindings["tab:new"].len(), 2);
        assert!(saved.bindings["tab:new"][0].meta);
        assert!(
            saved.bindings["terminal:clear"].is_empty(),
            "an action with no chords stays: it means no shortcut at all"
        );
        assert_eq!(store.keymap("mac").unwrap(), saved);
        assert_eq!(store.keymap("other").unwrap(), KeymapView::default());

        store
            .save_keymap(
                "other",
                KeymapDraft {
                    bindings: bindings(&[("tab:new", &[chord("cs", "KeyT")])]),
                    texts: vec![],
                },
            )
            .unwrap();
        assert_eq!(store.keymap("mac").unwrap(), saved);

        // A save replaces the whole keymap.
        let emptied = store.save_keymap("mac", KeymapDraft::default()).unwrap();
        assert!(emptied.bindings.is_empty());
        let rows: i64 = store
            .conn
            .lock()
            .query_row("SELECT count(*) FROM keymaps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 2, "one record per family, however often it is saved");
    }

    #[test]
    fn what_does_not_look_like_a_keymap_is_refused() {
        let store = store();
        let with = |entries: &[(&str, &[Chord])]| KeymapDraft {
            bindings: bindings(entries),
            texts: vec![],
        };
        let ok = [chord("c", "KeyK")];

        assert!(is_invalid(
            store.save_keymap("linux", KeymapDraft::default()),
            "family"
        ));
        assert!(matches!(
            store.keymap("windows"),
            Err(StoreError::Invalid {
                field: "family",
                ..
            })
        ));
        assert!(matches!(
            store.keymap_text("", Uuid::nil()),
            Err(StoreError::Invalid {
                field: "family",
                ..
            })
        ));

        for action in ["", "Tab:New", "tab new", "täb", &"a".repeat(81)] {
            assert!(
                is_invalid(store.save_keymap("mac", with(&[(action, &ok)])), "action"),
                "{action:?}"
            );
        }
        assert!(store
            .save_keymap("mac", with(&[(&"a".repeat(80), &ok)]))
            .is_ok());

        for code in ["", "Key C", "Key-C", "Taste", &"K".repeat(33)] {
            let refused = code != "Taste";
            let result = store.save_keymap("mac", with(&[("a", &[chord("c", code)])]));
            assert_eq!(is_invalid(result, "code"), refused, "{code:?}");
        }

        let nine = vec![chord("c", "KeyK"); 9];
        assert!(is_invalid(
            store.save_keymap("mac", with(&[("a", &nine)])),
            "chords"
        ));
        assert!(store.save_keymap("mac", with(&[("a", &nine[..8])])).is_ok());

        let many = |count: usize| KeymapDraft {
            bindings: (0..count).map(|n| (format!("a{n}"), ok.to_vec())).collect(),
            texts: vec![],
        };
        assert!(is_invalid(store.save_keymap("mac", many(301)), "bindings"));
        assert!(store.save_keymap("mac", many(300)).is_ok());
    }

    #[test]
    fn texts_are_checked_before_anything_is_written() {
        let store = store();
        let with = |texts: Vec<KeymapTextDraft>| KeymapDraft {
            bindings: BTreeMap::new(),
            texts,
        };
        let id = Uuid::now_v7();

        let label = |label: &str| with(vec![text(id, label, PasswordChange::Keep)]);
        assert!(is_invalid(
            store.save_keymap("mac", label(&"l".repeat(101))),
            "label"
        ));
        assert!(is_invalid(
            store.save_keymap("mac", label("zwei\nzeilen")),
            "label"
        ));
        assert!(store.save_keymap("mac", label(&"ä".repeat(100))).is_ok());

        for bad in ["", "esc\u{1b}[2J", "allein\rstehend", &"x".repeat(4001)] {
            assert!(
                is_invalid(
                    store.save_keymap("mac", with(vec![text(id, "t", set(bad))])),
                    "text"
                ),
                "{:?}",
                bad.chars().take(12).collect::<String>()
            );
        }
        assert_eq!(live_secrets(&store), 0, "a refused text is sealed nowhere");

        assert!(is_invalid(
            store.save_keymap(
                "mac",
                with(vec![
                    text(id, "a", PasswordChange::Keep),
                    text(id, "b", PasswordChange::Keep)
                ])
            ),
            "texts"
        ));
        let many = |count: usize| {
            with(
                (0..count)
                    .map(|_| text(Uuid::now_v7(), "t", PasswordChange::Keep))
                    .collect(),
            )
        };
        assert!(is_invalid(store.save_keymap("mac", many(101)), "texts"));
        assert!(store.save_keymap("mac", many(100)).is_ok());

        // Windows line ends become plain ones; tabs and umlauts stay.
        store
            .save_keymap(
                "mac",
                with(vec![text(id, "t", set("eins\r\n\tzwei ä\r\n"))]),
            )
            .unwrap();
        let (typed, enter) = store.keymap_text("mac", id).unwrap().unwrap();
        assert_eq!(typed.as_str(), "eins\n\tzwei ä\n");
        assert!(enter);
        assert!(store
            .save_keymap("mac", with(vec![text(id, "t", set(&"ä".repeat(4000)))]))
            .is_ok());
    }

    #[test]
    fn a_text_is_sealed_in_the_vault_and_never_in_the_keymap() {
        let store = store();
        let id = Uuid::now_v7();
        let typed = "sudo -i\nhunter2"; // gitleaks:allow
        let action = format!("text:{id}");
        let saved = store
            .save_keymap(
                "other",
                KeymapDraft {
                    bindings: bindings(&[(&action, &[chord("ca", "KeyP")])]),
                    texts: vec![text(id, "  root werden ", set(typed))],
                },
            )
            .unwrap();
        assert_eq!(saved.texts.len(), 1);
        assert_eq!(saved.texts[0].id, id);
        assert_eq!(saved.texts[0].label, "root werden");
        assert!(saved.texts[0].has_text && saved.texts[0].enter);
        assert!(saved.bindings.contains_key(&action));

        let body: String = store
            .conn
            .lock()
            .query_row("SELECT body FROM keymaps", [], |row| row.get(0))
            .unwrap();
        assert!(!body.contains("hunter2"), "the text is not in the record");
        let blob: Vec<u8> = store
            .conn
            .lock()
            .query_row("SELECT blob FROM secrets WHERE deleted = 0", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!blob.windows(7).any(|w| w == b"hunter2"));
        assert_eq!(
            store.keymap_text("other", id).unwrap().unwrap().0.as_str(),
            typed
        );
        assert_eq!(store.keymap_text("mac", id).unwrap(), None);
        assert_eq!(store.keymap_text("other", Uuid::now_v7()).unwrap(), None);
    }

    #[test]
    fn keeping_replacing_and_forgetting_a_text() {
        let store = store();
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let save = |texts: Vec<KeymapTextDraft>| {
            store
                .save_keymap(
                    "mac",
                    KeymapDraft {
                        bindings: bindings(&[
                            (&format!("text:{a}"), &[chord("m", "Digit1")]),
                            (&format!("text:{b}"), &[chord("m", "Digit2")]),
                            ("text:not-an-id", &[chord("m", "Digit3")]),
                        ]),
                        texts,
                    },
                )
                .unwrap()
        };
        let typed = |id| {
            store
                .keymap_text("mac", id)
                .unwrap()
                .map(|(text, _)| text.to_string())
        };

        let view = save(vec![
            text(b, "zwei", set("two")),
            text(a, "eins", set("one")),
        ]);
        assert_eq!(
            view.texts.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![b, a],
            "in the order the page sent them"
        );
        assert_eq!(view.bindings.len(), 2, "a binding for no text is dropped");
        assert_eq!(live_secrets(&store), 2);

        // Keep leaves the text alone while its label changes.
        let view = save(vec![
            text(a, "erstes", PasswordChange::Keep),
            text(b, "zwei", PasswordChange::Keep),
        ]);
        assert_eq!(view.texts[0].label, "erstes");
        assert!(view.texts[0].has_text);
        assert_eq!(typed(a).as_deref(), Some("one"));

        // A new text replaces the old one, which is forgotten.
        save(vec![
            text(a, "erstes", set("uno")),
            text(b, "zwei", PasswordChange::Keep),
        ]);
        assert_eq!(typed(a).as_deref(), Some("uno"));
        assert_eq!(live_secrets(&store), 2);

        // Forget keeps the entry and drops what it typed.
        let view = save(vec![
            text(a, "erstes", PasswordChange::Forget),
            text(b, "zwei", PasswordChange::Keep),
        ]);
        assert!(!view.texts[0].has_text);
        assert_eq!(typed(a), None);
        assert_eq!(live_secrets(&store), 1);

        // A text that is not in the draft goes, with its secret and binding.
        let view = save(vec![text(a, "erstes", PasswordChange::Keep)]);
        assert_eq!(view.texts.len(), 1);
        assert_eq!(
            view.bindings.keys().collect::<Vec<_>>(),
            vec![&format!("text:{a}")]
        );
        assert_eq!(typed(b), None);
        assert_eq!(live_secrets(&store), 0);
        let wiped: i64 = store
            .conn
            .lock()
            .query_row(
                "SELECT count(*) FROM secrets WHERE deleted = 1 AND length(blob) = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(wiped, 3, "forgotten texts leave no ciphertext behind");
    }

    #[test]
    fn a_locked_vault_shows_and_rebinds_but_neither_reads_nor_sets_a_text() {
        let store = store();
        let id = Uuid::now_v7();
        let draft = |change| KeymapDraft {
            bindings: bindings(&[(&format!("text:{id}"), &[chord("c", "F5")])]),
            texts: vec![text(id, "t", change)],
        };
        store.save_keymap("other", draft(set("geheim"))).unwrap();
        store.lock_vault();

        let view = store.keymap("other").unwrap();
        assert!(view.texts[0].has_text);
        assert!(matches!(
            store.keymap_text("other", id),
            Err(StoreError::VaultLocked)
        ));
        assert!(matches!(
            store.save_keymap("other", draft(set("anders"))),
            Err(StoreError::VaultLocked)
        ));
        let kept = store
            .save_keymap("other", draft(PasswordChange::Keep))
            .unwrap();
        assert!(kept.texts[0].has_text, "a refused save changed nothing");

        // Forgetting works locked, and leaves nothing to open.
        let gone = store
            .save_keymap("other", draft(PasswordChange::Forget))
            .unwrap();
        assert!(!gone.texts[0].has_text);
        assert_eq!(store.keymap_text("other", id).unwrap(), None);
    }

    #[test]
    fn what_a_newer_build_wrote_survives_a_save_here() {
        let store = store();
        let id = Uuid::now_v7();
        store
            .save_keymap(
                "mac",
                KeymapDraft {
                    bindings: BTreeMap::new(),
                    texts: vec![text(id, "t", set("x"))],
                },
            )
            .unwrap();
        // A newer build adds a field to the keymap and one to the text.
        {
            let conn = store.conn.lock();
            let body: String = conn
                .query_row("SELECT body FROM keymaps", [], |row| row.get(0))
                .unwrap();
            let mut json: serde_json::Value = serde_json::from_str(&body).unwrap();
            json["sequences"] = serde_json::json!({"leader": "KeyB"});
            json["texts"][id.to_string()]["colour"] = "red".into();
            conn.execute("UPDATE keymaps SET body = ?1", [json.to_string()])
                .unwrap();
        }

        store
            .save_keymap(
                "mac",
                KeymapDraft {
                    bindings: bindings(&[("tab:new", &[chord("m", "KeyT")])]),
                    texts: vec![text(id, "neu", PasswordChange::Keep)],
                },
            )
            .unwrap();

        let body: String = store
            .conn
            .lock()
            .query_row("SELECT body FROM keymaps", [], |row| row.get(0))
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["sequences"]["leader"], "KeyB");
        assert_eq!(json["texts"][id.to_string()]["colour"], "red");
        assert_eq!(json["texts"][id.to_string()]["label"], "neu");
        assert_eq!(json["bindings"]["tab:new"][0]["code"], "KeyT");
        assert_eq!(
            store.keymap_text("mac", id).unwrap().unwrap().0.as_str(),
            "x"
        );
    }

    #[test]
    fn the_page_and_the_store_agree_on_the_json() {
        let draft: KeymapDraft = serde_json::from_value(serde_json::json!({
            "bindings": {"tab:new": [{"ctrl": true, "code": "KeyT"}]},
            "texts": [
                {"id": Uuid::nil(), "label": "a", "enter": false, "text": {"kind": "keep"}},
                {"id": Uuid::max(), "label": "b", "enter": true,
                 "text": {"kind": "set", "value": "hallo"}},
                {"id": Uuid::now_v7(), "label": "c", "enter": true, "text": {"kind": "forget"}}
            ]
        }))
        .unwrap();
        assert!(draft.bindings["tab:new"][0].ctrl);
        assert!(matches!(draft.texts[1].text, PasswordChange::Set { .. }));
        assert!(matches!(draft.texts[2].text, PasswordChange::Forget));

        let view = KeymapView {
            bindings: bindings(&[("tab:new", &[chord("cs", "KeyT")])]),
            texts: vec![KeymapText {
                id: Uuid::nil(),
                label: "a".into(),
                enter: true,
                has_text: true,
            }],
        };
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            serde_json::json!({
                "bindings": {"tab:new": [{"ctrl": true, "shift": true, "code": "KeyT"}]},
                "texts": [{"id": Uuid::nil(), "label": "a", "enter": true, "hasText": true}]
            })
        );
    }
}
