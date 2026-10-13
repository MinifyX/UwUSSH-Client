//! Two servers: which there are, which is the main one, where new entries
//! go, and taking one away.
//!
//! The work is `uwussh_store::Spaces`'; this layer adds what belongs to the
//! running app — each server's sync side (see [`crate::sync::Syncs`]) and the
//! events the page listens to. A server is named by its space's id; the
//! second one, while it is being set up, by `new`.

use crate::lock::Running;
use crate::sync::{self, SyncFailure, SyncResult, SyncStatus, Syncs};
use crate::AppState;
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};
use uwussh_store::{
    SpaceDefaults, SpaceHandle, SpaceKind, Store, VaultStatus, Workspace, NEW_SPACE,
};
use zeroize::Zeroizing;

/// The list of servers, a label, a lock state or a default changed.
pub(crate) fn changed(app: &AppHandle) {
    let _ = app.emit("spaces:changed", ());
    let _ = app.emit("sync:status", ());
}

/// A vault was opened, or its key changed: open what the main vault's key
/// opens, and seal every other open vault's key under it again.
pub(crate) fn rechain(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.spaces.unlock_chain();
    state.spaces.reseal_chain();
    changed(app);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpaceInfo {
    id: String,
    main: bool,
    kind: SpaceKind,
    /// The server's host name; empty while it is connected to none.
    label: String,
    /// Its vault did not open with the main vault's key.
    locked: bool,
    read_only: bool,
    status: SyncStatus,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpacesStatus {
    spaces: Vec<SpaceInfo>,
    defaults: SpaceDefaults,
    can_add: bool,
}

/// The host name of a server address, as the list of servers names it.
fn host_of(url: &str) -> String {
    let rest = url.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, rest)| rest);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or("");
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    let host = match rest.strip_prefix('[') {
        // An IPv6 address keeps its brackets and loses the port after them.
        Some(inside) => match inside.split_once(']') {
            Some((address, _)) => return format!("[{address}]"),
            None => rest,
        },
        None => rest.split(':').next().unwrap_or(""),
    };
    host.to_string()
}

fn label(store: &Store) -> String {
    let lock = store.lock_state().ok().filter(|state| state.active);
    let url = match lock {
        Some(lock) => lock.server_url,
        None => store.sync_state().ok().and_then(|state| state.server_url),
    };
    url.as_deref().map(host_of).unwrap_or_default()
}

fn info(state: &AppState, syncs: &Syncs, space: SpaceHandle) -> SyncResult<SpaceInfo> {
    Ok(SpaceInfo {
        label: label(&space.store),
        locked: state.spaces.is_locked(&space.id),
        status: SyncStatus::of(&space.store, &syncs.of(&space.id))?,
        id: space.id,
        main: space.main,
        kind: space.kind,
        read_only: space.read_only,
    })
}

#[tauri::command]
pub(crate) fn spaces_status(
    state: State<'_, AppState>,
    syncs: State<'_, Syncs>,
) -> SyncResult<SpacesStatus> {
    let spaces = state
        .spaces
        .all()
        .into_iter()
        .map(|space| info(&state, &syncs, space))
        .collect::<SyncResult<Vec<_>>>()?;
    Ok(SpacesStatus {
        spaces,
        defaults: state.spaces.defaults(),
        can_add: sync::can_add(&state.spaces),
    })
}

/// Where new entries of a workspace go.
#[tauri::command]
pub(crate) fn space_set_default(
    app: AppHandle,
    state: State<'_, AppState>,
    workspace: Workspace,
    space: String,
) -> SyncResult<()> {
    state.spaces.set_default(workspace, &space)?;
    changed(&app);
    Ok(())
}

async fn blocking<T: Send + 'static>(
    app: &AppHandle,
    work: impl FnOnce(&AppHandle) -> SyncResult<T> + Send + 'static,
) -> SyncResult<T> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || work(&app))
        .await
        .map_err(SyncFailure::error)?
}

fn open(store: &Store) -> SyncResult<bool> {
    Ok(store.vault_status()? == VaultStatus::Unlocked)
}

/// Make the other server the main one: shortcuts, snippets and the
/// assistant's settings go there, and its password opens the app from now
/// on. No password is asked: both vaults are open, or nothing happens.
#[tauri::command]
pub(crate) async fn space_make_main(app: AppHandle, space: String) -> SyncResult<()> {
    blocking(&app, move |app| {
        let state = app.state::<AppState>();
        let syncs = app.state::<Syncs>();
        let target = sync::target(app, Some(&space), false)?;
        if target.new {
            return Err(SyncFailure::error("this server is not connected yet"));
        }
        let main = state.spaces.main_id();
        if !open(&state.store())? || !open(&target.store)? {
            return Err(SyncFailure::VaultLocked);
        }
        // Neither server syncs while the personal records change hands: a
        // pass in between would send the old main's emptied ones first.
        let (old, new) = (syncs.of(&main), syncs.of(&target.id));
        let made = if target.main {
            Ok(())
        } else {
            let _old = Running::take(&old)?;
            let _new = Running::take(&new)?;
            state.spaces.make_main(&target.id, &crate::device::protect)
        };
        made?;
        changed(app);
        syncs.poke();
        Ok(())
    })
    .await
}

/// Open a server's vault with that server's own password, when the main
/// vault's key did not open it, and keep its key under the main vault's
/// again.
#[tauri::command]
pub(crate) async fn space_unlock(
    app: AppHandle,
    space: String,
    password: String,
) -> SyncResult<()> {
    let password = Zeroizing::new(password);
    blocking(&app, move |app| {
        let target = sync::target(app, Some(&space), false)?;
        if !open(&target.store)? {
            unlock(&target.store, &password)?;
        }
        drop(target);
        rechain(app);
        app.state::<Syncs>().poke();
        Ok(())
    })
    .await
}

fn unlock(store: &Store, password: &Zeroizing<String>) -> SyncResult<()> {
    let account_key = if store.vault_needs_account_key()? {
        sync::kept_account_key(store)
    } else {
        None
    };
    store.unlock_vault_with(password.as_bytes(), account_key.as_ref())?;
    Ok(())
}

/// Take a server away from this device, with `keep` after its entries moved
/// to the one that stays. `password` is that server's own master password.
/// Taking the main server away makes the other one main first.
#[tauri::command]
pub(crate) async fn space_remove(
    app: AppHandle,
    space: String,
    password: String,
    keep: bool,
) -> SyncResult<()> {
    let password = Zeroizing::new(password);
    if space == NEW_SPACE {
        sync::discard_new(&app);
        return Ok(());
    }
    blocking(&app, move |app| {
        let state = app.state::<AppState>();
        let target = sync::target(app, Some(&space), false)?;
        let Some(rest) = state.spaces.other(&target.id) else {
            return Err(SyncFailure::error(
                "this is the only server: disconnect it instead",
            ));
        };

        // The password first, against the vault as it is: wrong means
        // nothing changes.
        let store = &target.store;
        if let Some(header) = store.vault_header()? {
            let account_key = if store.vault_needs_account_key()? {
                sync::kept_account_key(store)
            } else {
                None
            };
            header
                .unlock_with(password.as_bytes(), account_key.as_ref())
                .map_err(|_| SyncFailure::PasswordWrong)?;
            if !open(store)? {
                store.unlock_vault_with(password.as_bytes(), account_key.as_ref())?;
                state.spaces.unlock_chain();
            }
        }
        // Entries that move are sealed again for the other vault, and a new
        // main vault takes the personal records: both are open for that.
        if (keep || target.main) && !(open(store)? && open(&rest.store)?) {
            return Err(SyncFailure::VaultLocked);
        }

        // What the server is told afterwards, taken while its store is here.
        let paired = store.sync_state()?;
        let leaving = match paired.device_id.filter(|_| paired.paired()) {
            Some(me) => sync::server(store, &target.sync)
                .ok()
                .map(|server| (server, me)),
            None => None,
        };

        // Nothing of this server's runs or holds its store from here on. No
        // pass in particular: what moves away is deleted on this device only,
        // and a pass would delete it for the server's other devices too.
        let sync::Target {
            id, sync, store, ..
        } = target;
        drop(store);
        drop(rest);
        sync.held.store(true, Ordering::SeqCst);
        let removed = Running::take(&sync).and_then(|running| {
            let removed = state
                .spaces
                .remove(&id, keep, &crate::device::protect)
                .map_err(SyncFailure::from);
            if removed.is_ok() {
                // Before the pass flag is let go: no pass ever runs again.
                sync::retire(app, &id, &sync);
            }
            drop(running);
            removed
        });
        if let Err(failure) = removed {
            sync.held.store(false, Ordering::SeqCst);
            // A part of the entries may have moved already.
            changed(app);
            return Err(failure);
        }

        if let Some((server, me)) = leaving {
            if let Err(error) = server.revoke(me, None) {
                tracing::info!(%error, "the server could not be told this device left");
            }
        }
        rechain(app);
        app.state::<Syncs>().poke();
        Ok(())
    })
    .await
}

/// Give up on the second server being set up. Nothing when none is.
#[tauri::command]
pub(crate) async fn space_discard_new(app: AppHandle) -> SyncResult<()> {
    blocking(&app, |app| {
        sync::discard_new(app);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::host_of;

    #[test]
    fn a_server_is_named_by_its_host() {
        assert_eq!(host_of("https://sync.example.com"), "sync.example.com");
        assert_eq!(host_of("https://nas.example:8443/base/"), "nas.example");
        assert_eq!(host_of(" http://192.0.2.7:8080 "), "192.0.2.7");
        assert_eq!(host_of("https://[2001:db8::1]:8443/x"), "[2001:db8::1]");
        assert_eq!(
            host_of("https://user@lock.example.org?x=1"),
            "lock.example.org"
        );
        assert_eq!(host_of("nas.test"), "nas.test");
        assert_eq!(host_of(""), "");
    }
}
