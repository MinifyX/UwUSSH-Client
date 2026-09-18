//! What the sync engine needs from the store: records on their way out, and
//! records on their way in.
//!
//! A local write marks its row `dirty` in the same transaction that writes it,
//! so the outbox is the database rather than something next to it that a crash
//! can lose. [`Store::pending_envelopes`] turns those rows into sealed
//! envelopes, and [`Store::apply_envelopes`] takes envelopes apart again — one
//! decision per record, inside one transaction, so a sync interrupted halfway
//! leaves no half-applied record behind.
//!
//! Three rules hold everywhere in here:
//!
//! 1. **The server never sees a field.** Every payload is sealed with the vault
//!    key before it leaves, so pushing needs an unlocked vault, and so does
//!    applying.
//! 2. **An envelope that fails its tag is dropped, not applied.** The header —
//!    clock and tombstone flag included — is part of the associated data, so a
//!    server cannot mark a host deleted or replay an old version.
//! 3. **Local-only columns stay local**: the key file path, when this device
//!    last connected, and what system it found. They mean nothing anywhere
//!    else.

use crate::vault::truncate_wal;
use crate::{now_ms, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use uuid::Uuid;
use uwussh_proto::{
    resolve, EntityKind, Envelope, Extra, GroupPayload, Hlc, HostPayload, IdentityPayload,
    KeyPayload, KnownHostPayload, Resolution, SnippetPayload, Version, MAX_BLOB_BYTES,
};
use uwussh_vault::{Sealed, UnlockedVault};
use zeroize::Zeroizing;

/// Where this device stands with its server. Local, never synced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub server_url: Option<String>,
    pub account_id: Option<Uuid>,
    pub device_id: Option<Uuid>,
    /// The server sequence number everything up to which this device has seen.
    pub cursor: u64,
    pub last_sync_ms: Option<u64>,
}

/// What one record the server accepted is called, so the store can clear the
/// right row — and only if it has not changed since.
#[derive(Debug, Clone, Copy)]
pub struct Pushed {
    pub id: Uuid,
    pub kind: EntityKind,
    pub updated_at: Hlc,
    pub seq: u64,
}

/// What came of a batch of pulled records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    /// Records written: new ones, updates and tombstones.
    pub applied: usize,
    /// Records where this device's version was the newer one and stays.
    pub kept: usize,
    /// Records that were already what we have.
    pub identical: usize,
    /// Records this build has no table for, or a tombstone for something we
    /// never had.
    pub skipped: usize,
    /// Records whose seal did not hold, or that belong to another vault.
    /// Anything above zero means the server handed back something it should
    /// not have.
    pub rejected: usize,
    /// Two devices trusted a different key for the same `address:port`.
    pub host_key_conflicts: usize,
}

impl ApplyReport {
    /// Fold another batch's counts into these, for a pass that takes more
    /// than one round.
    pub fn add(&mut self, other: ApplyReport) {
        self.applied += other.applied;
        self.kept += other.kept;
        self.identical += other.identical;
        self.skipped += other.skipped;
        self.rejected += other.rejected;
        self.host_key_conflicts += other.host_key_conflicts;
    }
}

/// The table a kind lives in, or `None` for a kind this build does not keep.
fn table_of(kind: EntityKind) -> Option<&'static str> {
    Some(match kind {
        EntityKind::Host => "hosts",
        EntityKind::Group => "host_groups",
        EntityKind::Identity => "identities",
        EntityKind::Key => "keys",
        EntityKind::Snippet => "snippets",
        EntityKind::KnownHost => "known_hosts",
        EntityKind::Secret => "secrets",
        // Port forwards and terminal profiles have no table yet. A newer
        // build's records for them stay on the server, where they do no harm.
        EntityKind::PortForward | EntityKind::TerminalProfile => return None,
    })
}

/// Every table whose rows travel, in the order records must be applied:
/// whatever a record can point at comes first.
const SYNCED_KINDS: [EntityKind; 7] = EntityKind::APPLY_ORDER;

/// A row waiting to be pushed.
struct Pending {
    id: Uuid,
    kind: EntityKind,
    updated_at: Hlc,
    deleted: bool,
    base_seq: u64,
    payload: Zeroizing<Vec<u8>>,
}

fn parse_uuid(text: &str) -> Result<Uuid> {
    Uuid::parse_str(text).map_err(|_| StoreError::Invalid {
        field: "id",
        problem: "invalid",
    })
}

fn clock_of(wall: i64, counter: i64, device: i64) -> Hlc {
    Hlc::new(wall as u64, counter as u32, device as u32)
}

/// `sync_extra` as it comes out of the database: the fields a newer build
/// wrote that this one does not know.
fn extra_from(text: Option<String>) -> Extra {
    text.and_then(|text| serde_json::from_str::<Extra>(&text).ok())
        .unwrap_or_default()
}

fn extra_to(extra: &Extra) -> Option<String> {
    if extra.is_empty() {
        None
    } else {
        serde_json::to_string(extra).ok()
    }
}

impl Store {
    pub fn sync_state(&self) -> Result<SyncState> {
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT server_url, account_id, device_id, cursor, last_sync_ms
               FROM sync_state WHERE id = 1",
            [],
            |row| {
                let account: Option<String> = row.get(1)?;
                let device: Option<String> = row.get(2)?;
                let cursor: i64 = row.get(3)?;
                let last: Option<i64> = row.get(4)?;
                Ok(SyncState {
                    server_url: row.get(0)?,
                    account_id: account.and_then(|id| Uuid::parse_str(&id).ok()),
                    device_id: device.and_then(|id| Uuid::parse_str(&id).ok()),
                    cursor: cursor as u64,
                    last_sync_ms: last.map(|ms| ms as u64),
                })
            },
        )?)
    }

    pub fn set_sync_cursor(&self, cursor: u64) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE sync_state SET cursor = ?1, last_sync_ms = ?2 WHERE id = 1",
            params![cursor as i64, now_ms() as i64],
        )?;
        Ok(())
    }

    /// How many records are waiting to be pushed.
    pub fn pending_count(&self) -> Result<usize> {
        let conn = self.conn.lock();
        let mut total = 0i64;
        for kind in SYNCED_KINDS {
            let Some(table) = table_of(kind) else {
                continue;
            };
            total += conn.query_row(
                &format!("SELECT count(*) FROM {table} WHERE dirty = 1"),
                [],
                |row| row.get::<_, i64>(0),
            )?;
        }
        Ok(total as usize)
    }

    /// Records changed here and not yet accepted by the server, sealed and
    /// ready to push. Needs the vault unlocked, since that is what seals them.
    ///
    /// Returns how many records were **left out** as well. A record too large
    /// for the protocol, or one whose payload cannot be built at all, is left
    /// where it is rather than offered and refused: one oversized snippet must
    /// not stop everything else from syncing. The count comes back so the UI
    /// can say that something stayed behind instead of quietly dropping it.
    pub fn pending_envelopes(&self, limit: usize) -> Result<(Vec<Envelope>, usize)> {
        let conn = self.conn.lock();
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let vault_id = vault.vault_id();

        let mut envelopes = Vec::new();
        let mut left_out = 0;
        for kind in SYNCED_KINDS {
            if envelopes.len() >= limit {
                break;
            }
            let (rows, unreadable) = pending_rows(&conn, vault, kind, limit - envelopes.len())?;
            left_out += unreadable;
            for row in rows {
                let sealed = vault.seal_synced(
                    row.id,
                    row.kind,
                    row.updated_at,
                    row.deleted,
                    &row.payload,
                )?;
                if sealed.blob.len() > MAX_BLOB_BYTES {
                    tracing::warn!(
                        id = %row.id,
                        kind = ?row.kind,
                        bytes = sealed.blob.len(),
                        "record too large for the protocol, left out of the sync"
                    );
                    left_out += 1;
                    continue;
                }
                envelopes.push(Envelope {
                    id: row.id,
                    vault_id,
                    kind: row.kind,
                    updated_at: row.updated_at,
                    base_seq: row.base_seq,
                    deleted: row.deleted,
                    nonce: sealed.nonce,
                    blob: sealed.blob,
                    seq: None,
                });
            }
        }
        Ok((envelopes, left_out))
    }

    /// Clear the pending flag for records the server took, and remember the
    /// version it gave them.
    ///
    /// A row that changed while the push was in flight keeps its flag: what
    /// the server accepted is not what is here any more.
    pub fn mark_pushed(&self, pushed: &[Pushed]) -> Result<usize> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut cleared = 0;
        for record in pushed {
            let Some(table) = table_of(record.kind) else {
                continue;
            };
            cleared += tx.execute(
                &format!(
                    "UPDATE {table}
                        SET dirty = 0, server_seq = ?2
                      WHERE id = ?1 AND hlc_wall_ms = ?3 AND hlc_counter = ?4 AND hlc_device = ?5"
                ),
                params![
                    record.id.to_string(),
                    record.seq as i64,
                    record.updated_at.wall_ms as i64,
                    record.updated_at.counter,
                    record.updated_at.device,
                ],
            )?;
        }
        tx.commit()?;
        Ok(cleared)
    }

    /// Apply records pulled from the server.
    ///
    /// One transaction for the batch, so an interrupted sync leaves the store
    /// consistent; the cursor moves separately, once the batch is in.
    pub fn apply_envelopes(&self, envelopes: &[Envelope]) -> Result<ApplyReport> {
        if envelopes.is_empty() {
            return Ok(ApplyReport::default());
        }
        let mut conn = self.conn.lock();
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;

        // Whatever a record points at is applied before the record itself, so
        // a batch needs as few placeholder rows as possible.
        let mut order: Vec<&Envelope> = envelopes.iter().collect();
        order.sort_by_key(|env| (env.kind.apply_rank(), env.seq.unwrap_or(0)));

        let tx = conn.transaction()?;
        let mut report = ApplyReport::default();
        let mut newest = None::<Hlc>;
        let mut wiped_a_secret = false;
        for env in order {
            let outcome = apply_one(&tx, vault, env, self.device)?;
            report.add(outcome.report);
            wiped_a_secret |= outcome.wiped_a_secret;
            if outcome.report.rejected == 0 {
                newest = Some(match newest {
                    Some(seen) if seen > env.updated_at => seen,
                    _ => env.updated_at,
                });
            }
        }
        // The local clock catches up with what arrived, capped against a
        // device whose own clock is far in the future.
        if let Some(remote) = newest {
            let (wall, counter): (i64, i64) = tx.query_row(
                "SELECT clock_wall_ms, clock_counter FROM meta WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let merged = clock_of(wall, counter, self.device as i64).merge(remote, now_ms());
            tx.execute(
                "UPDATE meta SET clock_wall_ms = ?1, clock_counter = ?2 WHERE id = 1",
                params![merged.wall_ms as i64, merged.counter as i64],
            )?;
        }
        tx.commit()?;
        drop(guard);
        if wiped_a_secret {
            // A password another device forgot must not linger in a free page.
            truncate_wal(&conn);
        }
        Ok(report)
    }

    /// Take over an account's vault: its id and its key.
    ///
    /// This is what joining an account does on a device that already has hosts
    /// of its own. Every secret is opened with the old key and sealed again
    /// with the new one — the ids stay, but a sealed record is bound to its
    /// vault, so the ciphertext has to be rewritten. Everything then waits to
    /// be pushed, because to the account this is all new.
    ///
    /// The vault must be unlocked if there is anything sealed to carry over,
    /// and `key` must be the one `header`'s wrapped key opens to — callers get
    /// it from `VaultHeader::unlock`, which is the only way to know.
    pub fn adopt_vault(
        &self,
        header: &uwussh_vault::VaultHeader,
        key: Zeroizing<[u8; 32]>,
    ) -> Result<()> {
        let adopted = UnlockedVault::from_key(header.vault_id, key);
        let mut conn = self.conn.lock();
        let mut guard = self.vault.lock();
        let tx = conn.transaction()?;

        let current: String =
            tx.query_row("SELECT vault_id FROM meta WHERE id = 1", [], |row| {
                row.get(0)
            })?;
        if current == header.vault_id.to_string() {
            return Ok(());
        }

        let sealed = {
            let mut stmt = tx.prepare(
                "SELECT id, nonce, blob FROM secrets WHERE deleted = 0 AND length(blob) > 0",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        Sealed {
                            nonce: row.get(1)?,
                            blob: row.get(2)?,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        if !sealed.is_empty() {
            let old = guard.as_ref().ok_or(StoreError::VaultLocked)?;
            for (id, old_sealed) in &sealed {
                let uuid = parse_uuid(id)?;
                let plain = old.open(uuid, EntityKind::Secret, old_sealed)?;
                let new_sealed = adopted.seal(uuid, EntityKind::Secret, &plain)?;
                tx.execute(
                    "UPDATE secrets SET nonce = ?2, blob = ?3 WHERE id = ?1",
                    params![id, new_sealed.nonce, new_sealed.blob],
                )?;
            }
        }

        let vault_id = header.vault_id.to_string();
        tx.execute("DELETE FROM vault", [])?;
        tx.execute(
            "INSERT INTO vault
                (id, vault_id, kdf_memory_kib, kdf_time_cost, kdf_parallelism,
                 salt, wrapped_nonce, wrapped_blob)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                vault_id,
                header.kdf.memory_kib,
                header.kdf.time_cost,
                header.kdf.parallelism,
                header.salt.to_vec(),
                header.wrapped_key.nonce,
                header.wrapped_key.blob,
            ],
        )?;
        tx.execute("UPDATE meta SET vault_id = ?1 WHERE id = 1", [&vault_id])?;
        for kind in SYNCED_KINDS {
            let Some(table) = table_of(kind) else {
                continue;
            };
            tx.execute(
                &format!("UPDATE {table} SET vault_id = ?1, server_seq = 0"),
                [&vault_id],
            )?;
            // A placeholder for a record that has not arrived is not a
            // record: it keeps the zero clock and stays out of the outbox.
            tx.execute(
                &format!(
                    "UPDATE {table} SET dirty = 1
                      WHERE deleted = 0
                        AND (hlc_wall_ms > 0 OR hlc_counter > 0 OR hlc_device > 0)"
                ),
                [],
            )?;
        }
        // The key this device had sealed for itself opens the old vault, and
        // the cursor counted a server this account does not share.
        tx.execute("DELETE FROM device_unlock", [])?;
        tx.execute(
            "UPDATE sync_state SET cursor = 0, last_sync_ms = NULL WHERE id = 1",
            [],
        )?;
        tx.commit()?;

        *guard = Some(adopted);
        tracing::info!(vault = %header.vault_id, "vault of an account adopted");
        Ok(())
    }
}

/// Rows of one kind waiting to be pushed, with their payload built.
fn pending_rows(
    conn: &rusqlite::Connection,
    vault: &UnlockedVault,
    kind: EntityKind,
    limit: usize,
) -> Result<(Vec<Pending>, usize)> {
    let Some(table) = table_of(kind) else {
        return Ok((Vec::new(), 0));
    };
    let columns = match kind {
        EntityKind::Host => "name, address, port, workspace, position, group_id, identity_id",
        EntityKind::Group => "workspace, name, position",
        EntityKind::Identity => "label, username, auth_type, key_id, password_secret_id",
        EntityKind::Key => "label, key_type, public_key, private_secret_id, passphrase_secret_id",
        EntityKind::Snippet => "label, body, group_path",
        EntityKind::KnownHost => {
            "address, port, algorithm, fingerprint_sha256, public_key, first_seen_ms"
        }
        EntityKind::Secret => "nonce, blob",
        EntityKind::PortForward | EntityKind::TerminalProfile => return Ok((Vec::new(), 0)),
    };
    let extra = if matches!(kind, EntityKind::Secret) {
        "NULL"
    } else {
        "sync_extra"
    };
    let sql = format!(
        "SELECT id, hlc_wall_ms, hlc_counter, hlc_device, deleted, server_seq, {extra}, {columns}
           FROM {table} WHERE dirty = 1 ORDER BY hlc_wall_ms, hlc_counter LIMIT {limit}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut pending = Vec::new();
    let mut unreadable = 0;
    while let Some(row) = rows.next()? {
        let id = parse_uuid(&row.get::<_, String>(0)?)?;
        let updated_at = clock_of(row.get(1)?, row.get(2)?, row.get(3)?);
        let deleted: bool = row.get(4)?;
        let base_seq: i64 = row.get(5)?;
        let extra = extra_from(row.get(6)?);

        // A tombstone has no fields worth sealing — only its header, which the
        // seal covers either way.
        let payload = if deleted {
            Ok(Zeroizing::new(Vec::new()))
        } else {
            payload_of(vault, kind, id, row, extra)
        };
        // A row whose payload cannot be built is left behind rather than
        // failing the whole pass: one broken record must not stop sync for
        // everything else.
        let payload = match payload {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!(%id, kind = ?kind, %error, "a record that cannot be sealed, left out of the sync");
                unreadable += 1;
                continue;
            }
        };
        pending.push(Pending {
            id,
            kind,
            updated_at,
            deleted,
            base_seq: base_seq as u64,
            payload,
        });
    }
    Ok((pending, unreadable))
}

/// The bytes that go into an envelope: the record's fields as JSON, or — for a
/// secret — the secret itself, opened from its stored form and sealed again
/// for the way out.
fn payload_of(
    vault: &UnlockedVault,
    kind: EntityKind,
    id: Uuid,
    row: &rusqlite::Row<'_>,
    extra: Extra,
) -> Result<Zeroizing<Vec<u8>>> {
    let json = match kind {
        EntityKind::Host => {
            let port: i64 = row.get(9)?;
            serde_json::to_vec(&HostPayload {
                name: row.get(7)?,
                address: row.get(8)?,
                port: port as u16,
                workspace: row.get(10)?,
                position: row.get(11)?,
                group_id: optional_uuid(row.get(12)?)?,
                identity_id: optional_uuid(row.get(13)?)?,
                extra,
            })
        }
        EntityKind::Group => serde_json::to_vec(&GroupPayload {
            workspace: row.get(7)?,
            name: row.get(8)?,
            position: row.get(9)?,
            extra,
        }),
        EntityKind::Identity => serde_json::to_vec(&IdentityPayload {
            label: row.get(7)?,
            username: row.get(8)?,
            auth_type: row.get(9)?,
            key_id: optional_uuid(row.get(10)?)?,
            password_secret_id: optional_uuid(row.get(11)?)?,
            extra,
        }),
        EntityKind::Key => serde_json::to_vec(&KeyPayload {
            label: row.get(7)?,
            key_type: row.get(8)?,
            public_key: row.get(9)?,
            private_secret_id: optional_uuid(row.get(10)?)?,
            passphrase_secret_id: optional_uuid(row.get(11)?)?,
            extra,
        }),
        EntityKind::Snippet => serde_json::to_vec(&SnippetPayload {
            label: row.get(7)?,
            body: row.get(8)?,
            group_path: row.get(9)?,
            extra,
        }),
        EntityKind::KnownHost => {
            let port: i64 = row.get(8)?;
            let first_seen: i64 = row.get(12)?;
            serde_json::to_vec(&KnownHostPayload {
                address: row.get(7)?,
                port: port as u16,
                algorithm: row.get(9)?,
                fingerprint_sha256: row.get(10)?,
                public_key: row.get(11)?,
                first_seen_ms: first_seen as u64,
                extra,
            })
        }
        EntityKind::Secret => {
            let sealed = Sealed {
                nonce: row.get(7)?,
                blob: row.get(8)?,
            };
            return Ok(vault.open(id, EntityKind::Secret, &sealed)?);
        }
        EntityKind::PortForward | EntityKind::TerminalProfile => {
            return Ok(Zeroizing::new(Vec::new()))
        }
    };
    json.map(Zeroizing::new).map_err(|_| StoreError::Invalid {
        field: "record",
        problem: "unserialisable",
    })
}

fn optional_uuid(text: Option<String>) -> Result<Option<Uuid>> {
    text.as_deref().map(parse_uuid).transpose()
}

struct Applied {
    report: ApplyReport,
    wiped_a_secret: bool,
}

fn one(report: ApplyReport) -> Applied {
    Applied {
        report,
        wiped_a_secret: false,
    }
}

fn skipped() -> Applied {
    one(ApplyReport {
        skipped: 1,
        ..Default::default()
    })
}

fn rejected() -> Applied {
    one(ApplyReport {
        rejected: 1,
        ..Default::default()
    })
}

/// Apply one pulled record. `this_device` is this device's own id, which tells
/// a record another device wrote from one of ours coming back off the server.
fn apply_one(
    tx: &Transaction,
    vault: &UnlockedVault,
    env: &Envelope,
    this_device: u32,
) -> Result<Applied> {
    if env.vault_id != vault.vault_id() {
        tracing::warn!(%env.id, "a record from another vault, dropped");
        return Ok(rejected());
    }
    let Some(table) = table_of(env.kind) else {
        return Ok(skipped());
    };

    // The seal covers the whole header, so this is where a flipped tombstone
    // flag, a rewritten clock or a swapped blob is caught.
    let payload = match vault.open_synced(
        env.id,
        env.kind,
        env.updated_at,
        env.deleted,
        &Sealed {
            nonce: env.nonce.clone(),
            blob: env.blob.clone(),
        },
    ) {
        Ok(payload) => payload,
        Err(_) => {
            tracing::warn!(%env.id, kind = ?env.kind, "a record that failed its seal, dropped");
            return Ok(rejected());
        }
    };

    let local: Option<(i64, i64, i64, bool)> = tx
        .query_row(
            &format!(
                "SELECT hlc_wall_ms, hlc_counter, hlc_device, deleted FROM {table} WHERE id = ?1"
            ),
            [env.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let seq = env.seq.unwrap_or(0) as i64;

    let Some((wall, counter, device, deleted)) = local else {
        if env.deleted {
            // Something we never had was deleted somewhere else.
            return Ok(skipped());
        }
        return insert_record(tx, vault, env, &payload, seq, this_device);
    };

    let decision = resolve(
        Version::new(clock_of(wall, counter, device), deleted),
        Version::new(env.updated_at, env.deleted),
    );
    match decision {
        Resolution::Identical => {
            // The server confirms what we have; remember its version so the
            // next push is based on the right one.
            tx.execute(
                &format!("UPDATE {table} SET dirty = 0, server_seq = ?2 WHERE id = ?1"),
                params![env.id.to_string(), seq],
            )?;
            Ok(one(ApplyReport {
                identical: 1,
                ..Default::default()
            }))
        }
        Resolution::Local => {
            // Ours is newer. Keep it pending, but base the next push on the
            // version the server holds, or it would be refused forever.
            tx.execute(
                &format!("UPDATE {table} SET server_seq = ?2 WHERE id = ?1"),
                params![env.id.to_string(), seq],
            )?;
            Ok(one(ApplyReport {
                kept: 1,
                ..Default::default()
            }))
        }
        Resolution::Remote if env.deleted => tombstone_record(tx, env, table, seq),
        Resolution::Remote => insert_record(tx, vault, env, &payload, seq, this_device),
    }
}

fn tombstone_record(tx: &Transaction, env: &Envelope, table: &str, seq: i64) -> Result<Applied> {
    let clock = env.updated_at;
    // A secret's ciphertext goes with its tombstone: a password another device
    // forgot has no reason to stay here, not even sealed.
    let wipe = if matches!(env.kind, EntityKind::Secret) {
        ", nonce = x'', blob = x''"
    } else {
        ""
    };
    tx.execute(
        &format!(
            "UPDATE {table}
                SET deleted = 1, rev = rev + 1, dirty = 0, server_seq = ?2{wipe},
                    hlc_wall_ms = ?3, hlc_counter = ?4, hlc_device = ?5
              WHERE id = ?1"
        ),
        params![
            env.id.to_string(),
            seq,
            clock.wall_ms as i64,
            clock.counter,
            clock.device
        ],
    )?;
    Ok(Applied {
        report: ApplyReport {
            applied: 1,
            ..Default::default()
        },
        wiped_a_secret: matches!(env.kind, EntityKind::Secret),
    })
}

/// Write a record, whether it is new here or a newer version of one we have.
fn insert_record(
    tx: &Transaction,
    vault: &UnlockedVault,
    env: &Envelope,
    payload: &[u8],
    seq: i64,
    this_device: u32,
) -> Result<Applied> {
    let vault_id = env.vault_id.to_string();
    let id = env.id.to_string();
    let clock = env.updated_at;

    match env.kind {
        EntityKind::Host => {
            let host: HostPayload = from_payload(payload)?;
            // What the host points at must exist first, or the foreign keys
            // refuse the row. A placeholder is a live row with the oldest
            // possible clock, so the real record wins as soon as it arrives.
            if let Some(group) = host.group_id {
                stub_group(tx, &vault_id, group)?;
            }
            if let Some(identity) = host.identity_id {
                stub_identity(tx, &vault_id, identity)?;
            }
            let extra = extra_to(&host.extra);
            tx.execute(
                "INSERT INTO hosts
                    (id, vault_id, name, address, port, identity_id, group_id, workspace,
                     position, hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty,
                     server_seq, sync_extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, 0, ?13, ?14)
                 ON CONFLICT (id) DO UPDATE SET
                    name = excluded.name, address = excluded.address, port = excluded.port,
                    identity_id = excluded.identity_id, group_id = excluded.group_id,
                    workspace = excluded.workspace, position = excluded.position,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, sync_extra = excluded.sync_extra,
                    rev = rev + 1",
                params![
                    id,
                    vault_id,
                    host.name,
                    host.address,
                    host.port.max(1),
                    host.identity_id.map(|i| i.to_string()),
                    host.group_id.map(|g| g.to_string()),
                    workspace_of(&host.workspace),
                    host.position,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                    extra,
                ],
            )?;
        }
        EntityKind::Group => {
            let group: GroupPayload = from_payload(payload)?;
            let extra = extra_to(&group.extra);
            tx.execute(
                "INSERT INTO host_groups
                    (id, vault_id, workspace, name, position, hlc_wall_ms, hlc_counter,
                     hlc_device, deleted, dirty, server_seq, sync_extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, ?9, ?10)
                 ON CONFLICT (id) DO UPDATE SET
                    workspace = excluded.workspace, name = excluded.name,
                    position = excluded.position,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, sync_extra = excluded.sync_extra,
                    rev = rev + 1",
                params![
                    id,
                    vault_id,
                    workspace_of(&group.workspace),
                    group.name,
                    group.position,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                    extra,
                ],
            )?;
        }
        EntityKind::Identity => {
            let identity: IdentityPayload = from_payload(payload)?;
            if let Some(key) = identity.key_id {
                stub_key(tx, &vault_id, key)?;
            }
            if let Some(secret) = identity.password_secret_id {
                stub_secret(tx, &vault_id, secret)?;
            }
            let extra = extra_to(&identity.extra);
            tx.execute(
                "INSERT INTO identities
                    (id, vault_id, label, username, auth_type, key_id, password_secret_id,
                     hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty, server_seq, sync_extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, ?12)
                 ON CONFLICT (id) DO UPDATE SET
                    label = excluded.label, username = excluded.username,
                    auth_type = excluded.auth_type, key_id = excluded.key_id,
                    password_secret_id = excluded.password_secret_id,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, sync_extra = excluded.sync_extra,
                    rev = rev + 1",
                params![
                    id,
                    vault_id,
                    identity.label,
                    identity.username,
                    auth_type_of(&identity.auth_type),
                    identity.key_id.map(|k| k.to_string()),
                    identity.password_secret_id.map(|s| s.to_string()),
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                    extra,
                ],
            )?;
        }
        EntityKind::Key => {
            let key: KeyPayload = from_payload(payload)?;
            for secret in [key.private_secret_id, key.passphrase_secret_id]
                .into_iter()
                .flatten()
            {
                stub_secret(tx, &vault_id, secret)?;
            }
            let extra = extra_to(&key.extra);
            tx.execute(
                "INSERT INTO keys
                    (id, vault_id, label, key_type, public_key, private_secret_id,
                     passphrase_secret_id, hlc_wall_ms, hlc_counter, hlc_device, deleted,
                     dirty, server_seq, sync_extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, ?12)
                 ON CONFLICT (id) DO UPDATE SET
                    label = excluded.label, key_type = excluded.key_type,
                    public_key = excluded.public_key,
                    private_secret_id = excluded.private_secret_id,
                    passphrase_secret_id = excluded.passphrase_secret_id,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, sync_extra = excluded.sync_extra,
                    rev = rev + 1",
                params![
                    id,
                    vault_id,
                    key.label,
                    key.key_type,
                    key.public_key,
                    key.private_secret_id.map(|s| s.to_string()),
                    key.passphrase_secret_id.map(|s| s.to_string()),
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                    extra,
                ],
            )?;
        }
        EntityKind::Snippet => {
            let snippet: SnippetPayload = from_payload(payload)?;
            let extra = extra_to(&snippet.extra);
            tx.execute(
                "INSERT INTO snippets
                    (id, vault_id, label, body, group_path, hlc_wall_ms, hlc_counter,
                     hlc_device, deleted, dirty, server_seq, sync_extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, ?9, ?10)
                 ON CONFLICT (id) DO UPDATE SET
                    label = excluded.label, body = excluded.body,
                    group_path = excluded.group_path,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, sync_extra = excluded.sync_extra,
                    rev = rev + 1",
                params![
                    id,
                    vault_id,
                    snippet.label,
                    snippet.body,
                    snippet.group_path,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                    extra,
                ],
            )?;
        }
        EntityKind::KnownHost => {
            let known: KnownHostPayload = from_payload(payload)?;
            let address = known.address.trim().to_ascii_lowercase();
            // One key per address and port. Two devices that trusted a
            // different key for the same machine without ever seeing each
            // other's produce two records for one slot; the newer one takes
            // it, on every device, so they end up agreeing.
            let rival: Option<(String, String)> = tx
                .query_row(
                    "SELECT id, fingerprint_sha256 FROM known_hosts
                      WHERE address = ?1 AND port = ?2 AND id != ?3",
                    params![address, known.port, id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let mut report = ApplyReport {
                applied: 1,
                ..Default::default()
            };
            if let Some((rival_id, fingerprint)) = rival {
                let (wall, counter, device): (i64, i64, i64) = tx.query_row(
                    "SELECT hlc_wall_ms, hlc_counter, hlc_device FROM known_hosts WHERE id = ?1",
                    [&rival_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                // Worth a word to the user — but only for a record another
                // device wrote. A pull can hand this device its own record back
                // in the same pass that brought the rival one, and that is the
                // same disagreement over again, not a second one.
                if fingerprint != known.fingerprint_sha256 && env.updated_at.device != this_device {
                    tracing::warn!(
                        %address,
                        port = known.port,
                        theirs = %known.fingerprint_sha256,
                        ours = %fingerprint,
                        "two devices trust a different host key for the same machine"
                    );
                    report.host_key_conflicts = 1;
                }
                if clock_of(wall, counter, device) > env.updated_at {
                    // What is here is the newer decision; the record that
                    // arrived loses its slot.
                    report.applied = 0;
                    report.kept = 1;
                    return Ok(one(report));
                }
                tx.execute("DELETE FROM known_hosts WHERE id = ?1", [&rival_id])?;
            }
            let extra = extra_to(&known.extra);
            tx.execute(
                "INSERT INTO known_hosts
                    (id, vault_id, address, port, algorithm, fingerprint_sha256, public_key,
                     first_seen_ms, hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty,
                     server_seq, sync_extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, 0, ?12, ?13)
                 ON CONFLICT (id) DO UPDATE SET
                    address = excluded.address, port = excluded.port,
                    algorithm = excluded.algorithm,
                    fingerprint_sha256 = excluded.fingerprint_sha256,
                    public_key = excluded.public_key, first_seen_ms = excluded.first_seen_ms,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, sync_extra = excluded.sync_extra,
                    rev = rev + 1",
                params![
                    id,
                    vault_id,
                    address,
                    known.port,
                    known.algorithm,
                    known.fingerprint_sha256,
                    known.public_key,
                    known.first_seen_ms as i64,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                    extra,
                ],
            )?;
            return Ok(one(report));
        }
        EntityKind::Secret => {
            // The payload is the secret itself. It is sealed again for
            // storage, under the form the local store reads.
            let sealed = vault.seal(env.id, EntityKind::Secret, payload)?;
            tx.execute(
                "INSERT INTO secrets
                    (id, vault_id, nonce, blob, hlc_wall_ms, hlc_counter, hlc_device,
                     deleted, dirty, server_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, ?8)
                 ON CONFLICT (id) DO UPDATE SET
                    nonce = excluded.nonce, blob = excluded.blob,
                    hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                    hlc_device = excluded.hlc_device, deleted = 0, dirty = 0,
                    server_seq = excluded.server_seq, rev = rev + 1",
                params![
                    id,
                    vault_id,
                    sealed.nonce,
                    sealed.blob,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                    seq,
                ],
            )?;
        }
        EntityKind::PortForward | EntityKind::TerminalProfile => return Ok(skipped()),
    }
    Ok(one(ApplyReport {
        applied: 1,
        ..Default::default()
    }))
}

fn from_payload<T: serde::de::DeserializeOwned>(payload: &[u8]) -> Result<T> {
    serde_json::from_slice(payload).map_err(|_| StoreError::Invalid {
        field: "record",
        problem: "unreadable",
    })
}

/// A workspace this build does not know shows up as private rather than
/// making the whole record unusable.
fn workspace_of(value: &str) -> &str {
    match value {
        "business" => "business",
        _ => "private",
    }
}

fn auth_type_of(value: &str) -> &str {
    match value {
        "key" | "agent" | "keyboard-interactive" | "cert" => value,
        _ => "password",
    }
}

/// Placeholders for records that have not arrived yet. A placeholder is live
/// but empty, with the oldest possible clock, so the real record replaces it
/// the moment it turns up — and it is never pushed, since it is not a record.
fn stub_group(tx: &Transaction, vault_id: &str, id: Uuid) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO host_groups
            (id, vault_id, workspace, name, position, hlc_wall_ms, hlc_counter, hlc_device,
             rev, deleted, dirty, server_seq)
         VALUES (?1, ?2, 'private', '', 0, 0, 0, 0, 0, 0, 0, 0)",
        params![id.to_string(), vault_id],
    )?;
    Ok(())
}

fn stub_identity(tx: &Transaction, vault_id: &str, id: Uuid) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO identities
            (id, vault_id, label, username, auth_type, hlc_wall_ms, hlc_counter, hlc_device,
             rev, deleted, dirty, server_seq)
         VALUES (?1, ?2, '', '', 'password', 0, 0, 0, 0, 0, 0, 0)",
        params![id.to_string(), vault_id],
    )?;
    Ok(())
}

fn stub_key(tx: &Transaction, vault_id: &str, id: Uuid) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO keys
            (id, vault_id, label, key_type, public_key, hlc_wall_ms, hlc_counter, hlc_device,
             rev, deleted, dirty, server_seq)
         VALUES (?1, ?2, '', '', '', 0, 0, 0, 0, 0, 0, 0)",
        params![id.to_string(), vault_id],
    )?;
    Ok(())
}

fn stub_secret(tx: &Transaction, vault_id: &str, id: Uuid) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO secrets
            (id, vault_id, nonce, blob, hlc_wall_ms, hlc_counter, hlc_device,
             rev, deleted, dirty, server_seq)
         VALUES (?1, ?2, x'', x'', 0, 0, 0, 0, 0, 0, 0)",
        params![id.to_string(), vault_id],
    )?;
    Ok(())
}
