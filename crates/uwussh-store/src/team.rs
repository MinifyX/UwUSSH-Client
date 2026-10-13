//! A store that is a team's vault.
//!
//! A team's store is a store like any other: its vault id is the team's id
//! and its vault key is the team key. What it has beyond that is one row,
//! `team_state`, with the number of the key it is sealed under (the *epoch*)
//! and what a key rotation needs to survive a crash.
//!
//! There is no password behind such a vault. The header a store has to have
//! wraps the key under a random secret that is thrown away at once; the key
//! comes from the server, wrapped for this account, and is kept on the device
//! the way a second server's is: sealed by the main vault.
//!
//! A team's store also holds who is in the team, as far as that takes a key:
//! for every member somebody confirmed, the public key that was confirmed,
//! as a sealed record of its own that syncs like a password does. A rotation
//! wraps the new key for those keys and no others, so a server that lists a
//! member nobody let in, or another key for one, is not handed the team key.

use crate::sync::{parse_uuid, table_of, SYNCED_KINDS};
use crate::{Result, Store, StoreError};
use rand::RngCore;
use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use uwussh_proto::EntityKind;
use uwussh_vault::{KdfParams, MemberKey, Sealed, UnlockedVault, VaultHeader};
use zeroize::Zeroizing;

/// Sealed under a team key so that a key can be told from a wrong one.
const CHECK: &[u8] = b"uwussh/team-key/v1";

/// The record id the check value is sealed under.
fn check_id(team: Uuid) -> Uuid {
    Uuid::from_u128(team.as_u128() ^ 0x7577_7573_7368_2d74_6561_6d2d_6b65_7921)
}

/// The record id a pending rotation key is sealed under.
fn pending_id(team: Uuid, epoch: u64) -> Uuid {
    Uuid::from_u128(team.as_u128() ^ 0x7577_7573_7368_2d72_6f74_6174_6500_0000 ^ u128::from(epoch))
}

/// The record id a member's confirmed public key is kept under.
fn pin_id(team: Uuid, account: Uuid) -> Uuid {
    let digest = Sha256::new()
        .chain_update(b"uwussh/team/member-pin/v1")
        .chain_update(team.as_bytes())
        .chain_update(account.as_bytes())
        .finalize();
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(id)
}

/// What a team's store knows about its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamState {
    pub team: Uuid,
    /// The number of the key the store is sealed under.
    pub epoch: u64,
    /// After a change of key by somebody else: once everything was pulled,
    /// what the server does not hold goes back to it.
    pub requeue: bool,
    /// The epoch of a rotation this device began.
    pub pending_epoch: Option<u64>,
}

/// What becomes of the records when the key changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rekey {
    /// This device rotated: the server holds nothing, everything goes back.
    Lead,
    /// Somebody else did, and this account may write: pull first, then send
    /// what the server turns out not to have.
    Follow,
    /// Somebody else did, and this account only reads.
    Read,
}

/// The header a team's store carries. Nothing opens it: the secret it wraps
/// the key under is 32 random bytes nobody keeps, which is also why the
/// cheapest derivation there is does not weaken it.
fn header_for(vault: &UnlockedVault) -> Result<VaultHeader> {
    let mut secret = Zeroizing::new([0u8; 32]);
    rand::thread_rng().fill_bytes(secret.as_mut());
    let kdf = KdfParams {
        memory_kib: 8,
        time_cost: 1,
        parallelism: 1,
    };
    Ok(vault.rewrap(secret.as_ref(), None, kdf)?)
}

fn state_row(conn: &rusqlite::Connection) -> Result<Option<TeamState>> {
    let row = conn
        .query_row(
            "SELECT team_id, epoch, requeue, pending_epoch FROM team_state WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(team, epoch, requeue, pending)| {
        Ok(TeamState {
            team: parse_uuid(&team)?,
            epoch: epoch.max(0) as u64,
            requeue,
            pending_epoch: pending.map(|epoch| epoch.max(0) as u64),
        })
    })
    .transpose()
}

fn write_key(tx: &Transaction, vault: &UnlockedVault, epoch: u64, requeue: bool) -> Result<()> {
    let team = vault.vault_id();
    let check = vault.seal(check_id(team), EntityKind::Secret, CHECK)?;
    tx.execute("DELETE FROM vault", [])?;
    crate::vault::store_header(tx, &header_for(vault)?)?;
    tx.execute(
        "INSERT INTO team_state (id, team_id, epoch, check_nonce, check_blob, requeue)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET
            team_id = excluded.team_id, epoch = excluded.epoch,
            check_nonce = excluded.check_nonce, check_blob = excluded.check_blob,
            requeue = excluded.requeue,
            pending_epoch = NULL, pending_nonce = NULL, pending_blob = NULL",
        params![
            team.to_string(),
            epoch as i64,
            check.nonce,
            check.blob,
            requeue
        ],
    )?;
    // The key this device kept opens the old key's vault.
    tx.execute("DELETE FROM device_unlock", [])?;
    Ok(())
}

/// A rotation this device began, as the row holds it.
type PendingRow = (Option<i64>, Option<Vec<u8>>, Option<Vec<u8>>);

impl Store {
    /// What this store knows about its team key; `None` for a store that is
    /// not a team's.
    pub fn team_state(&self) -> Result<Option<TeamState>> {
        state_row(&self.conn.lock())
    }

    /// Make a new, empty store the vault of a team, open under `key`.
    pub fn team_begin(&self, team: Uuid, epoch: u64, key: Zeroizing<[u8; 32]>) -> Result<()> {
        let vault = UnlockedVault::from_key(team, key);
        let mut conn = self.conn.lock();
        let mut guard = self.vault.lock();
        let tx = conn.transaction()?;
        let has_vault: i64 = tx.query_row("SELECT count(*) FROM vault", [], |row| row.get(0))?;
        if has_vault > 0 {
            return Err(StoreError::VaultExists);
        }
        tx.execute(
            "UPDATE meta SET vault_id = ?1 WHERE id = 1",
            [team.to_string()],
        )?;
        write_key(&tx, &vault, epoch, false)?;
        tx.commit()?;
        *guard = Some(vault);
        Ok(())
    }

    /// Open a team's store with the key the server handed out for `epoch`.
    /// `Ok(false)` when that is not the key this store is sealed under.
    pub fn team_unlock(&self, epoch: u64, key: Zeroizing<[u8; 32]>) -> Result<bool> {
        let conn = self.conn.lock();
        let Some(state) = state_row(&conn)? else {
            return Ok(false);
        };
        if state.epoch != epoch {
            return Ok(false);
        }
        let check: Sealed = conn.query_row(
            "SELECT check_nonce, check_blob FROM team_state WHERE id = 1",
            [],
            |row| {
                Ok(Sealed {
                    nonce: row.get(0)?,
                    blob: row.get(1)?,
                })
            },
        )?;
        let vault = UnlockedVault::from_key(state.team, key);
        let fits = vault
            .open(check_id(state.team), EntityKind::Secret, &check)
            .is_ok_and(|plain| plain.as_slice() == CHECK);
        if fits {
            drop(conn);
            *self.vault.lock() = Some(vault);
        }
        Ok(fits)
    }

    /// The team key this store is open under, to wrap it for a member.
    pub fn team_key(&self) -> Result<Zeroizing<[u8; 32]>> {
        let guard = self.vault.lock();
        Ok(guard.as_ref().ok_or(StoreError::VaultLocked)?.export_key())
    }

    /// Whether `key` is the key this store is open under.
    pub fn team_key_is(&self, key: &[u8; 32]) -> Result<bool> {
        let current = self.team_key()?;
        // Not a secret-dependent branch worth hiding: both are ours.
        Ok(current.as_ref() == key)
    }

    /// Change the key: the store is sealed under `key` of `epoch` from here
    /// on. In one transaction the sealed secrets are sealed again, every
    /// record forgets the version the server had of it — a rotation deleted
    /// them all there — and the pull starts over. Edits that were waiting to
    /// be sent stay waiting.
    pub fn team_rekey(&self, epoch: u64, key: Zeroizing<[u8; 32]>, how: Rekey) -> Result<()> {
        let mut conn = self.conn.lock();
        let mut guard = self.vault.lock();
        let old = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let new = UnlockedVault::from_key(old.vault_id(), key);
        let tx = conn.transaction()?;
        if state_row(&tx)?.is_none() {
            return Err(StoreError::NoVault);
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
        for (id, old_sealed) in &sealed {
            let uuid = parse_uuid(id)?;
            let plain = old.open(uuid, EntityKind::Secret, old_sealed)?;
            let new_sealed = new.seal(uuid, EntityKind::Secret, &plain)?;
            tx.execute(
                "UPDATE secrets SET nonce = ?2, blob = ?3 WHERE id = ?1",
                params![id, new_sealed.nonce, new_sealed.blob],
            )?;
        }

        for kind in SYNCED_KINDS {
            let Some(table) = table_of(kind) else {
                continue;
            };
            tx.execute(&format!("UPDATE {table} SET server_seq = 0"), [])?;
            if how == Rekey::Lead {
                // Deletions too: the server has forgotten those as well.
                tx.execute(
                    &format!(
                        "UPDATE {table} SET dirty = 1
                          WHERE hlc_wall_ms > 0 OR hlc_counter > 0 OR hlc_device > 0"
                    ),
                    [],
                )?;
            }
        }
        crate::manifest::forget_manifests(&tx)?;
        tx.execute(
            "UPDATE sync_state SET cursor = 0, last_sync_ms = NULL WHERE id = 1",
            [],
        )?;
        write_key(&tx, &new, epoch, how == Rekey::Follow)?;
        tx.commit()?;
        *guard = Some(new);
        tracing::info!(epoch, "a team's key changed");
        Ok(())
    }

    /// After a change of key by somebody else and a pull to the end: every
    /// record the server did not send back goes to it again. Whoever rotated
    /// sends everything too; the same record twice is the same record.
    /// Returns how many were queued.
    pub fn team_requeue(&self) -> Result<usize> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let due = state_row(&tx)?.is_some_and(|state| state.requeue);
        if !due {
            return Ok(0);
        }
        let mut queued = 0;
        for kind in SYNCED_KINDS {
            let Some(table) = table_of(kind) else {
                continue;
            };
            queued += tx.execute(
                &format!(
                    "UPDATE {table} SET dirty = 1
                      WHERE dirty = 0 AND server_seq = 0
                        AND (hlc_wall_ms > 0 OR hlc_counter > 0 OR hlc_device > 0)"
                ),
                [],
            )?;
        }
        tx.execute("UPDATE team_state SET requeue = 0 WHERE id = 1", [])?;
        tx.commit()?;
        Ok(queued)
    }

    /// Note, before asking the server to rotate, the key the team is about
    /// to get. Sealed under the current key, so it is worth nothing to
    /// anybody who could not read the team anyway.
    pub fn team_note_rotation(&self, epoch: u64, key: &[u8; 32]) -> Result<()> {
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let sealed = vault.seal(pending_id(vault.vault_id(), epoch), EntityKind::Secret, key)?;
        drop(guard);
        let changed = self.conn.lock().execute(
            "UPDATE team_state SET pending_epoch = ?1, pending_nonce = ?2, pending_blob = ?3
              WHERE id = 1",
            params![epoch as i64, sealed.nonce, sealed.blob],
        )?;
        if changed == 0 {
            return Err(StoreError::NoVault);
        }
        Ok(())
    }

    /// The key of a rotation this device began and has not seen the end of.
    pub fn team_pending_rotation(&self) -> Result<Option<(u64, Zeroizing<[u8; 32]>)>> {
        let row: Option<PendingRow> = self
            .conn
            .lock()
            .query_row(
                "SELECT pending_epoch, pending_nonce, pending_blob FROM team_state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((Some(epoch), Some(nonce), Some(blob))) = row else {
            return Ok(None);
        };
        let epoch = epoch.max(0) as u64;
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let opened = vault.open(
            pending_id(vault.vault_id(), epoch),
            EntityKind::Secret,
            &Sealed { nonce, blob },
        );
        // One that does not open was noted under a key that is gone.
        Ok(opened
            .ok()
            .and_then(|plain| <[u8; 32]>::try_from(plain.as_slice()).ok())
            .map(|key| (epoch, Zeroizing::new(key))))
    }

    pub fn team_forget_rotation(&self) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE team_state
                SET pending_epoch = NULL, pending_nonce = NULL, pending_blob = NULL
              WHERE id = 1",
            [],
        )?;
        Ok(())
    }

    /// Write down the public key a member was confirmed with. It goes to the
    /// other members with the next sync.
    pub fn team_pin_member(&self, account: Uuid, public: &[u8; 32]) -> Result<()> {
        if self.team_pinned(account)?.as_ref() == Some(public) {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let id = pin_id(vault.vault_id(), account);
        let sealed = vault.seal(id, EntityKind::Secret, public)?;
        let tx = conn.transaction()?;
        let clock = crate::tick(&tx, self.device)?;
        tx.execute(
            "INSERT INTO secrets
                (id, vault_id, nonce, blob, hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET
                nonce = excluded.nonce, blob = excluded.blob,
                hlc_wall_ms = excluded.hlc_wall_ms, hlc_counter = excluded.hlc_counter,
                hlc_device = excluded.hlc_device, deleted = 0, dirty = 1, rev = rev + 1",
            params![
                id.to_string(),
                vault.vault_id().to_string(),
                sealed.nonce,
                sealed.blob,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The public key a member was confirmed with, if anybody wrote one down.
    pub fn team_pinned(&self, account: Uuid) -> Result<Option<[u8; 32]>> {
        let conn = self.conn.lock();
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let id = pin_id(vault.vault_id(), account);
        let sealed = conn
            .query_row(
                "SELECT nonce, blob FROM secrets WHERE id = ?1 AND deleted = 0",
                [id.to_string()],
                |row| {
                    Ok(Sealed {
                        nonce: row.get(0)?,
                        blob: row.get(1)?,
                    })
                },
            )
            .optional()?;
        let Some(sealed) = sealed else {
            return Ok(None);
        };
        // One that does not open, or is not a key, is as good as none.
        Ok(vault
            .open(id, EntityKind::Secret, &sealed)
            .ok()
            .and_then(|plain| <[u8; 32]>::try_from(plain.as_slice()).ok()))
    }

    /// Forget a member's confirmed key: they left, or were taken out.
    pub fn team_unpin_member(&self, account: Uuid) -> Result<()> {
        let mut conn = self.conn.lock();
        let Some(state) = state_row(&conn)? else {
            return Err(StoreError::NoVault);
        };
        let tx = conn.transaction()?;
        crate::vault::forget_secret(&tx, self.device, &pin_id(state.team, account).to_string())?;
        tx.commit()?;
        Ok(())
    }

    /// An account's member key, sealed under this store's vault for the
    /// server to keep. This is the account's own store on that server.
    pub fn seal_member_key(&self, member: &MemberKey, account: Uuid) -> Result<Vec<u8>> {
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        Ok(member.seal(vault, account)?)
    }

    /// Open what [`Store::seal_member_key`] wrote.
    pub fn open_member_key(&self, account: Uuid, sealed: &[u8]) -> Result<MemberKey> {
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        Ok(MemberKey::open(vault, account, sealed)?)
    }

    /// The hosts here that carry a stored password or a key: the logins
    /// somebody who left the team may still know.
    pub fn team_credential_hosts(&self) -> Result<Vec<(Uuid, String)>> {
        Ok(self
            .list_hosts()?
            .into_iter()
            .filter(|host| host.has_password || host.key_id.is_some())
            .map(|host| (host.id, host.name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::tests::draft;
    use crate::hosts::PasswordChange;
    use crate::secret::SecretText;
    use crate::VaultStatus;

    fn key(byte: u8) -> Zeroizing<[u8; 32]> {
        Zeroizing::new([byte; 32])
    }

    fn team() -> Uuid {
        Uuid::from_u128(0x7ea0)
    }

    fn begun() -> Store {
        let store = Store::open_in_memory().unwrap();
        store.team_begin(team(), 1, key(1)).unwrap();
        store
    }

    fn dirty(store: &Store, table: &str) -> i64 {
        store
            .conn
            .lock()
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE dirty = 1"),
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn all_pushed(store: &Store) {
        let conn = store.conn.lock();
        for kind in SYNCED_KINDS {
            if let Some(table) = table_of(kind) {
                conn.execute(&format!("UPDATE {table} SET dirty = 0, server_seq = 9"), [])
                    .unwrap();
            }
        }
    }

    #[test]
    fn a_team_store_is_the_teams_vault() {
        let store = begun();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Unlocked);
        let state = store.team_state().unwrap().unwrap();
        assert_eq!((state.team, state.epoch), (team(), 1));
        assert!(!state.requeue && state.pending_epoch.is_none());
        assert_eq!(store.vault_header().unwrap().unwrap().vault_id, team());
        assert!(store.team_begin(team(), 1, key(1)).is_err());
        assert!(Store::open_in_memory()
            .unwrap()
            .team_state()
            .unwrap()
            .is_none());
    }

    #[test]
    fn only_the_right_key_of_the_right_epoch_opens_it() {
        let store = begun();
        store.lock_vault();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Locked);
        assert!(!store.team_unlock(1, key(2)).unwrap());
        assert!(!store.team_unlock(2, key(1)).unwrap());
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Locked);
        assert!(store.team_unlock(1, key(1)).unwrap());
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Unlocked);
    }

    #[test]
    fn a_new_key_keeps_the_secrets_and_what_was_waiting() {
        let store = begun();
        let kept = store.save_host(draft("kept", "192.0.2.10")).unwrap();
        store
            .set_host_password(
                kept.id,
                PasswordChange::Set {
                    value: SecretText::new("hunter2"),
                },
            )
            .unwrap();
        all_pushed(&store);
        let waiting = store.save_host(draft("waiting", "192.0.2.11")).unwrap();
        store.remember_vault(|key| Ok(key.to_vec())).unwrap();

        store.team_rekey(2, key(2), Rekey::Read).unwrap();
        let state = store.team_state().unwrap().unwrap();
        assert_eq!(state.epoch, 2);
        assert!(!state.requeue);
        assert!(store.team_key_is(&[2; 32]).unwrap());
        assert_eq!(
            store.reveal_host_password(kept.id).unwrap().as_slice(),
            b"hunter2"
        );
        assert_eq!(dirty(&store, "hosts"), 1);
        assert!(!store.vault_is_remembered().unwrap());
        assert_eq!(store.sync_state().unwrap().cursor, 0);
        let (pending, _) = store.pending_envelopes(10).unwrap();
        assert!(pending.iter().any(|envelope| envelope.id == waiting.id));
        assert!(!pending.iter().any(|envelope| envelope.id == kept.id));
        assert!(pending.iter().all(|envelope| envelope.base_seq == 0));

        // The old key no longer opens it, the new one does.
        store.lock_vault();
        assert!(!store.team_unlock(1, key(1)).unwrap());
        assert!(store.team_unlock(2, key(2)).unwrap());
        assert_eq!(
            store.reveal_host_password(kept.id).unwrap().as_slice(),
            b"hunter2"
        );
    }

    #[test]
    fn whoever_rotated_sends_everything_again_deletions_too() {
        let store = begun();
        store.save_host(draft("one", "192.0.2.10")).unwrap();
        let gone = store.save_host(draft("gone", "192.0.2.11")).unwrap();
        store.delete_host(gone.id).unwrap();
        all_pushed(&store);
        assert_eq!(dirty(&store, "hosts"), 0);

        store.team_rekey(2, key(2), Rekey::Lead).unwrap();
        assert_eq!(dirty(&store, "hosts"), 2);
        assert!(store
            .pending_envelopes(10)
            .unwrap()
            .0
            .iter()
            .all(|envelope| envelope.base_seq == 0));
    }

    #[test]
    fn a_follower_sends_only_what_the_server_did_not_give_back() {
        let store = begun();
        let back = store.save_host(draft("back", "192.0.2.10")).unwrap();
        store.save_host(draft("missing", "192.0.2.11")).unwrap();
        all_pushed(&store);

        store.team_rekey(2, key(2), Rekey::Follow).unwrap();
        assert!(store.team_state().unwrap().unwrap().requeue);
        assert_eq!(dirty(&store, "hosts"), 0);
        // The pull gave one of them back.
        store
            .conn
            .lock()
            .execute(
                "UPDATE hosts SET server_seq = 4 WHERE id = ?1",
                [back.id.to_string()],
            )
            .unwrap();
        assert!(store.team_requeue().unwrap() >= 1);
        assert_eq!(dirty(&store, "hosts"), 1);
        assert_eq!(store.team_requeue().unwrap(), 0);
        assert!(!store.team_state().unwrap().unwrap().requeue);
    }

    #[test]
    fn a_rotation_that_was_begun_is_remembered_until_the_key_changes() {
        let store = begun();
        assert!(store.team_pending_rotation().unwrap().is_none());
        store.team_note_rotation(2, &[7; 32]).unwrap();
        assert_eq!(store.team_state().unwrap().unwrap().pending_epoch, Some(2));
        let (epoch, pending) = store.team_pending_rotation().unwrap().unwrap();
        assert_eq!((epoch, *pending), (2, [7; 32]));

        // The blob is not the key.
        let blob: Vec<u8> = store
            .conn
            .lock()
            .query_row("SELECT pending_blob FROM team_state", [], |row| row.get(0))
            .unwrap();
        assert!(!blob.windows(8).any(|window| window == [7; 8]));

        store.team_rekey(2, key(7), Rekey::Lead).unwrap();
        assert!(store.team_pending_rotation().unwrap().is_none());

        store.team_note_rotation(3, &[8; 32]).unwrap();
        store.team_forget_rotation().unwrap();
        assert!(store.team_pending_rotation().unwrap().is_none());
    }

    #[test]
    fn a_confirmed_key_is_a_record_that_syncs_and_outlives_a_new_team_key() {
        let store = begun();
        let (nia, olive) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(store.team_pinned(nia).unwrap(), None);
        store.team_pin_member(nia, &[7; 32]).unwrap();
        store.team_pin_member(olive, &[8; 32]).unwrap();
        assert_eq!(store.team_pinned(nia).unwrap(), Some([7; 32]));
        assert_eq!(dirty(&store, "secrets"), 2);
        // The same key again writes nothing.
        all_pushed(&store);
        store.team_pin_member(nia, &[7; 32]).unwrap();
        assert_eq!(dirty(&store, "secrets"), 0);

        store.team_rekey(2, key(2), Rekey::Read).unwrap();
        assert_eq!(store.team_pinned(olive).unwrap(), Some([8; 32]));
        store.team_unpin_member(olive).unwrap();
        assert_eq!(store.team_pinned(olive).unwrap(), None);
        assert_eq!(store.team_pinned(nia).unwrap(), Some([7; 32]));
        // Back again, with another key.
        store.team_pin_member(olive, &[9; 32]).unwrap();
        assert_eq!(store.team_pinned(olive).unwrap(), Some([9; 32]));
    }

    #[test]
    fn the_hosts_with_a_login_to_change_are_named() {
        let store = begun();
        store.save_host(draft("plain", "192.0.2.10")).unwrap();
        let secret = store.save_host(draft("secret", "192.0.2.11")).unwrap();
        store
            .set_host_password(
                secret.id,
                PasswordChange::Set {
                    value: SecretText::new("pw"),
                },
            )
            .unwrap();
        assert_eq!(
            store.team_credential_hosts().unwrap(),
            vec![(secret.id, "secret".to_string())]
        );
    }
}
