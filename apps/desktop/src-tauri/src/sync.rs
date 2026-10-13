//! Settings → Sync: connecting a UwUSync server, adding and removing devices,
//! and the thread that keeps this device in step — through UwUSync, or
//! through UwULock (see [`crate::lock`], which adds signing in, the realtime
//! channel and the move from one to the other).
//!
//! Every step the person takes is one command here, and every command is a
//! thin wrapper around `uwussh_sync::flow`, which puts the pieces in the right
//! order. What this layer adds is the operating system's seal (see
//! [`crate::device`]), the worker thread, and the words the page shows.
//!
//! The worker is a plain thread, because the transport is blocking and must
//! not share a thread with the terminals' runtime. It wakes every few seconds,
//! pushes when something is waiting, pulls once a minute, and at once when the
//! page asks. It never runs while the vault is locked: a pass needs the key to
//! seal and to open, and the lock is not the sync's to lift.
//!
//! With two servers there is all of this twice: each server's store has its
//! own [`Sync`], its own worker and its own listener, kept in [`Syncs`]. A
//! command says which one it means (`space`, see [`target`]); none means the
//! main one.

use crate::{err, AppState, CommandResult};
use parking_lot::{Condvar, Mutex};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use uwussh_store::{Spaces, Store, StoreError, VaultStatus, Withheld, NEW_SPACE};
use uwussh_sync::flow::{self, FlowError, PairingOffer};
use uwussh_sync::lock::{Difference, Lock, LockError};
use uwussh_sync::{pairing, Server, SyncError, SyncReport, TransportError};
use zeroize::Zeroizing;

/// How often the worker looks whether something waits to be pushed.
const TICK: Duration = Duration::from_secs(5);
/// How often it pulls when nothing happens here.
const PULL_EVERY: Duration = Duration::from_secs(60);
/// After a failed pass, how long before the next try.
const BACKOFF: Duration = Duration::from_secs(30);
/// While UwULock has app sync switched off, how long before it looks again —
/// quietly, since nothing is wrong here.
const SWITCHED_OFF_WAIT: Duration = Duration::from_secs(5 * 60);
/// With UwULock's realtime channel open, how much longer a pull waits: the
/// channel says when something changed, and this only covers a message that
/// got lost.
const QUIET_PULL: Duration = Duration::from_secs(14 * 60);

/// What the page is told when something went wrong. `kind` is what it acts
/// on; `message` is for showing.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum SyncFailure {
    /// The vault has to be opened first.
    VaultLocked,
    /// The master password (or the recovery code with it) is wrong.
    PasswordWrong,
    /// The code isn't one, or was misheard.
    BadCode,
    /// The server couldn't be reached.
    Unreachable {
        message: String,
    },
    /// The server said no, and why.
    Refused {
        message: String,
    },
    /// The other device did not answer in time, or the words didn't match.
    PairingFailed {
        message: String,
    },
    Error {
        message: String,
    },
    /// UwULock: the session ended, and only the master password brings it
    /// back.
    SignIn,
    /// UwULock: the server has app sync switched off. The session stays;
    /// syncing waits until an admin switches it on again.
    SwitchedOff,
    /// UwULock refused the email, the master password or the code, in its
    /// own words.
    LoginRefused {
        message: String,
    },
    /// UwULock: the account's key for UwU apps no longer opens (an official
    /// client replaced the key pair). Starting over is the web vault's.
    KeysLost,
    /// UwULock: the account never finished registration.
    NoKeyPair,
    /// UwULock asks for a cheaper key derivation than this account's last
    /// sign-in used. Nothing was sent.
    WeakerKdf {
        message: String,
    },
    /// UwULock lists a space this device left for a newer one, as a server
    /// rolling a rekey back would. Nothing was taken.
    SpaceLeft,
    /// The copy on UwULock is not what UwUSync holds. Nothing switched.
    MoveCheck {
        differences: Vec<Difference>,
    },
}

impl SyncFailure {
    pub(crate) fn error(message: impl std::fmt::Display) -> Self {
        Self::Error {
            message: message.to_string(),
        }
    }
}

impl From<StoreError> for SyncFailure {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::VaultLocked => Self::VaultLocked,
            StoreError::Vault(_) => Self::PasswordWrong,
            other => Self::error(other),
        }
    }
}

impl From<TransportError> for SyncFailure {
    fn from(error: TransportError) -> Self {
        match error {
            TransportError::Unreachable(message) => Self::Unreachable { message },
            TransportError::Refused(message) => Self::Refused { message },
            TransportError::SignIn(_) => Self::SignIn,
            TransportError::SwitchedOff => Self::SwitchedOff,
        }
    }
}

impl From<LockError> for SyncFailure {
    fn from(error: LockError) -> Self {
        match error {
            LockError::Transport(error) => error.into(),
            LockError::WrongPassword(message) => Self::LoginRefused { message },
            LockError::KeysLost => Self::KeysLost,
            LockError::NoKeyPair => Self::NoKeyPair,
            LockError::Store(error) => error.into(),
            LockError::VaultLocked => Self::VaultLocked,
            LockError::MoveCheck(differences) => Self::MoveCheck { differences },
            LockError::Crypto(message) => Self::Error { message },
            LockError::WeakerKdf(message) => Self::WeakerKdf { message },
            LockError::SpaceLeft(_) => Self::SpaceLeft,
        }
    }
}

impl From<FlowError> for SyncFailure {
    fn from(error: FlowError) -> Self {
        match error {
            FlowError::Transport(error) => error.into(),
            FlowError::Store(error) => error.into(),
            FlowError::Vault(_) => Self::PasswordWrong,
            FlowError::BadSetupCode => Self::BadCode,
            FlowError::VaultLocked => Self::VaultLocked,
            FlowError::NotPaired => Self::error("this device is not paired with a server"),
            FlowError::NoAccountKey => Self::PairingFailed {
                message: "the other device did not send the account key".into(),
            },
        }
    }
}

impl From<SyncError> for SyncFailure {
    fn from(error: SyncError) -> Self {
        match error {
            SyncError::VaultLocked => Self::VaultLocked,
            SyncError::Transport(error) => error.into(),
            other => Self::error(other),
        }
    }
}

pub(crate) type SyncResult<T> = Result<T, SyncFailure>;

/// What the last pass did, for the status line.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LastPass {
    at_ms: u64,
    report: Option<SyncReport>,
    /// Why it failed, in the server's or the network's words.
    error: Option<String>,
}

/// One server's worker's side, shared with the commands.
#[derive(Default)]
pub(crate) struct Sync {
    /// The server was removed from this device: no pass runs any more, and
    /// its threads end.
    pub(crate) gone: AtomicBool,
    /// The server is being removed: its threads let go of its store and
    /// wait for how that comes out.
    pub(crate) held: AtomicBool,
    /// A signed-in server, kept between passes. Dropped on any failure, so the
    /// next pass connects afresh.
    pub(crate) server: Mutex<Option<Arc<Server>>>,
    /// The same for UwULock. Kept on failures other than a session that
    /// ended: its refresh token must not be used twice.
    pub(crate) lock: Mutex<Option<Arc<Lock>>>,
    /// The realtime channel to UwULock is open.
    pub(crate) live: AtomicBool,
    /// UwULock refused the realtime channel; polling until the next sign-in.
    pub(crate) live_refused: AtomicBool,
    /// UwULock has app sync switched off: the last pass was told so.
    pub(crate) switched_off: AtomicBool,
    /// The UwUSync server this device just moved away from, and its id
    /// there: to remove it there, if the person wants.
    pub(crate) left_behind: Mutex<Option<(Arc<Server>, Uuid)>>,
    pub(crate) last: Mutex<Option<LastPass>>,
    pub(crate) running: AtomicBool,
    /// The last pass found the server keeping records back, and the window
    /// has been told.
    alarmed: AtomicBool,
    /// A pairing this device offered and is waiting on.
    offer: Mutex<Option<Arc<PairingOffer>>>,
    /// Someone is waiting on the offer. Two waiters on one code would both
    /// speak for side A of the handshake, and the other device would hear
    /// nonsense.
    waiting: AtomicBool,
    /// The page asked for a pass now.
    wake: Mutex<bool>,
    woken: Condvar,
}

impl Sync {
    pub(crate) fn poke(&self) {
        *self.wake.lock() = true;
        self.woken.notify_one();
    }
}

/// Every server's [`Sync`], and the second server while it is being set up.
#[derive(Default)]
pub(crate) struct Syncs {
    of: Mutex<HashMap<String, Arc<Sync>>>,
    /// A store that is no space yet: made when the second server's form
    /// first needs one, registered when the server is connected, thrown away
    /// when the person gives up.
    pending: Mutex<Option<Pending>>,
    /// The spaces whose threads run.
    started: Mutex<HashSet<String>>,
}

struct Pending {
    id: String,
    store: Arc<Store>,
    sync: Arc<Sync>,
}

impl Syncs {
    /// A server's side, made when first asked for.
    pub(crate) fn of(&self, id: &str) -> Arc<Sync> {
        Arc::clone(self.of.lock().entry(id.to_string()).or_default())
    }

    /// A pass now on every server: something changed here.
    pub(crate) fn poke(&self) {
        for sync in self.of.lock().values() {
            sync.poke();
        }
    }

    fn forget(&self, id: &str) {
        self.of.lock().remove(id);
        self.started.lock().remove(id);
    }

    pub(crate) fn setting_up(&self) -> bool {
        self.pending.lock().is_some()
    }
}

/// The server a command means.
pub(crate) struct Target {
    pub(crate) id: String,
    pub(crate) store: Arc<Store>,
    pub(crate) sync: Arc<Sync>,
    /// The main server: its password opens the app.
    pub(crate) main: bool,
    /// The second server being set up, not a space yet.
    pub(crate) new: bool,
}

fn open(store: &Store) -> bool {
    matches!(store.vault_status(), Ok(VaultStatus::Unlocked))
}

/// Syncing with a server at all, signed in or not.
pub(crate) fn connected(store: &Store) -> bool {
    store.sync_state().map(|s| s.paired()).unwrap_or(false)
        || store.lock_state().map(|s| s.active).unwrap_or(false)
}

/// Whether a second server may be set up: the first is connected and open,
/// and there is no second yet.
pub(crate) fn can_add(spaces: &Spaces) -> bool {
    let main = spaces.main();
    spaces.all().len() == 1 && spaces.has_room() && connected(&main) && open(&main)
}

/// Which server `space` means: none is the main one, `new` the second one
/// being set up — begun here with `begin`, when it is not yet.
pub(crate) fn target(app: &AppHandle, space: Option<&str>, begin: bool) -> SyncResult<Target> {
    let state = app.state::<AppState>();
    let syncs = app.state::<Syncs>();
    let id = match space {
        None => state.spaces.main_id(),
        Some(NEW_SPACE) => {
            let mut pending = syncs.pending.lock();
            if pending.is_none() {
                if !begin {
                    return Err(SyncFailure::error("no second server is being set up"));
                }
                if !can_add(&state.spaces) {
                    return Err(SyncFailure::error(
                        "a second server needs the first one connected and its vault open",
                    ));
                }
                let (id, store) = state.spaces.prepare()?;
                *pending = Some(Pending {
                    id,
                    store,
                    sync: Arc::default(),
                });
            }
            let pending = pending.as_ref().expect("just made");
            return Ok(Target {
                id: pending.id.clone(),
                store: Arc::clone(&pending.store),
                sync: Arc::clone(&pending.sync),
                main: false,
                new: true,
            });
        }
        Some(id) => id.to_string(),
    };
    let space = state
        .spaces
        .get(&id)
        .ok_or_else(|| SyncFailure::error("this server is not connected on this device"))?;
    Ok(Target {
        sync: syncs.of(&id),
        id,
        store: space.store,
        main: space.main,
        new: false,
    })
}

/// Two servers that are one account would list every host twice.
fn same_account(a: &Store, b: &Store) -> bool {
    let account = |store: &Store| store.sync_state().ok().and_then(|s| s.account_id);
    let space = |store: &Store| store.lock_state().ok().and_then(|s| s.space_id);
    account(a).is_some_and(|id| account(b) == Some(id))
        || space(a).is_some_and(|id| space(b) == Some(id))
}

/// The second server is connected: its store becomes a space, its key is
/// chained to the main vault's, and its threads start.
fn adopt_new(app: &AppHandle) -> SyncResult<()> {
    let state = app.state::<AppState>();
    let syncs = app.state::<Syncs>();
    let Some(pending) = syncs.pending.lock().take() else {
        return Err(SyncFailure::error("no second server is being set up"));
    };
    let Pending { id, store, sync } = pending;
    let refused = if state
        .spaces
        .all()
        .iter()
        .any(|space| same_account(&space.store, &store))
    {
        // It was enrolled a moment ago; the server hears that it left again.
        let me = store.sync_state().ok().and_then(|s| s.device_id);
        if let (Some(me), Some(server)) = (me, sync.server.lock().clone()) {
            if let Err(error) = server.revoke(me, None) {
                tracing::info!(%error, "the server could not be told this device left");
            }
        }
        Some(SyncFailure::error(
            "this account is already connected on this device",
        ))
    } else {
        state
            .spaces
            .register(&id, Arc::clone(&store))
            .err()
            .map(SyncFailure::from)
    };
    if let Some(failure) = refused {
        throw_away(&state.spaces, Pending { id, store, sync });
        return Err(failure);
    }
    drop(store);
    syncs.of.lock().insert(id.clone(), sync);
    start_space(app, &id);
    crate::spaces::changed(app);
    Ok(())
}

fn throw_away(spaces: &Spaces, pending: Pending) {
    let Pending { id, store, sync } = pending;
    sync.gone.store(true, Ordering::SeqCst);
    // The connections hold the store; the file goes when nobody does.
    *sync.lock.lock() = None;
    *sync.server.lock() = None;
    *sync.left_behind.lock() = None;
    drop(sync);
    drop(store);
    spaces.discard(&id);
}

/// Give up on the second server being set up. Nothing when none is.
pub(crate) fn discard_new(app: &AppHandle) {
    let state = app.state::<AppState>();
    let pending = app.state::<Syncs>().pending.lock().take();
    if let Some(pending) = pending {
        throw_away(&state.spaces, pending);
    }
}

/// A server left this device: its side stops, and nothing holds its store.
pub(crate) fn retire(app: &AppHandle, id: &str, sync: &Sync) {
    sync.gone.store(true, Ordering::SeqCst);
    *sync.lock.lock() = None;
    *sync.server.lock() = None;
    *sync.left_behind.lock() = None;
    *sync.offer.lock() = None;
    *sync.last.lock() = None;
    app.state::<Syncs>().forget(id);
    sync.poke();
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The server this device is paired with, signed in — the cached one, or a
/// fresh connection.
pub(crate) fn server(store: &Store, sync: &Sync) -> SyncResult<Arc<Server>> {
    if let Some(server) = sync.server.lock().clone() {
        return Ok(server);
    }
    let server = Arc::new(flow::reconnect(store, crate::device::unprotect_sync)?);
    *sync.server.lock() = Some(Arc::clone(&server));
    Ok(server)
}

/// One pass, with the outcome written down and told to the page.
pub(crate) fn pass(app: &AppHandle, store: &Arc<Store>, sync: &Sync) -> SyncResult<SyncReport> {
    if sync.running.swap(true, Ordering::SeqCst) {
        return Err(SyncFailure::error("a pass is already running"));
    }
    if sync.gone.load(Ordering::SeqCst) {
        // What moved to the other server was deleted here on this device
        // only; a pass would delete it for this server's other devices.
        sync.running.store(false, Ordering::SeqCst);
        return Err(SyncFailure::error(
            "this server was removed from this device",
        ));
    }
    if sync.held.load(Ordering::SeqCst) {
        // The same while it is being removed, however the pass was asked for.
        sync.running.store(false, Ordering::SeqCst);
        return Err(SyncFailure::error(
            "this server is being removed from this device",
        ));
    }
    let on_lock = store.lock_state().map(|s| s.active).unwrap_or(false);
    let outcome = (|| {
        if on_lock {
            let lock = crate::lock::connection(store, sync)?;
            return uwussh_sync::sync_once(store, lock.as_ref()).map_err(SyncFailure::from);
        }
        let server = server(store, sync)?;
        uwussh_sync::sync_once(store, server.as_ref()).map_err(SyncFailure::from)
    })();
    sync.running.store(false, Ordering::SeqCst);
    if on_lock && matches!(outcome, Err(SyncFailure::SignIn)) {
        crate::lock::session_ended(store, sync);
    }
    let off = on_lock && matches!(outcome, Err(SyncFailure::SwitchedOff));
    let was_off = sync.switched_off.swap(off, Ordering::SeqCst);
    if was_off && outcome.is_ok() {
        // On again: the realtime channel it refused meanwhile is worth a try.
        sync.live_refused.store(false, Ordering::SeqCst);
    }

    let last = match &outcome {
        // No failure to show: the status says it is switched off.
        Err(SyncFailure::SwitchedOff) => LastPass {
            at_ms: now_ms(),
            report: None,
            error: None,
        },
        Ok(report) => LastPass {
            at_ms: now_ms(),
            report: Some(*report),
            error: None,
        },
        Err(failure) => {
            // A fresh connection next time: the token, the pin, the address
            // may all be what went wrong. (UwULock's connection stays: its
            // tokens are only the server's to end.)
            *sync.server.lock() = None;
            LastPass {
                at_ms: now_ms(),
                report: None,
                error: Some(describe(failure)),
            }
        }
    };
    *sync.last.lock() = Some(last);
    if let Ok(report) = &outcome {
        if report.apply.applied > 0 {
            // Another device changed something: the host list loads again.
            let _ = app.emit("sync:changed", report.apply.applied);
        }
        // Not just a line on the settings page: the window hears of it once,
        // when it begins, whatever it is showing.
        let withheld = report.withheld.any();
        if withheld && !sync.alarmed.swap(true, Ordering::SeqCst) {
            let _ = app.emit("sync:withheld", report.withheld);
        } else if !withheld {
            sync.alarmed.store(false, Ordering::SeqCst);
        }
    }
    let _ = app.emit("sync:status", ());
    outcome
}

fn describe(failure: &SyncFailure) -> String {
    match failure {
        SyncFailure::VaultLocked => "the vault is locked".into(),
        SyncFailure::PasswordWrong => "wrong password".into(),
        SyncFailure::BadCode => "not a code".into(),
        SyncFailure::SignIn => "sign in again".into(),
        SyncFailure::SwitchedOff => "this UwULock server has app sync switched off".into(),
        SyncFailure::KeysLost => "the account's key for UwU apps is lost".into(),
        SyncFailure::NoKeyPair => "the account has no key pair".into(),
        SyncFailure::MoveCheck { differences } => {
            format!("the copy differs in {} records", differences.len())
        }
        SyncFailure::WeakerKdf { message } => format!("refused: {message}"),
        SyncFailure::SpaceLeft => "the server lists a space this device left".into(),
        SyncFailure::Unreachable { message }
        | SyncFailure::Refused { message }
        | SyncFailure::PairingFailed { message }
        | SyncFailure::LoginRefused { message }
        | SyncFailure::Error { message } => message.clone(),
    }
}

/// Starts every server's worker, and the thread that listens to UwULock.
pub(crate) fn start(app: &AppHandle) {
    app.manage(Syncs::default());
    for space in app.state::<AppState>().spaces.all() {
        start_space(app, &space.id);
    }
}

/// The worker and the listener of one server. They live as long as the
/// server is on this device.
pub(crate) fn start_space(app: &AppHandle, id: &str) {
    if !app.state::<Syncs>().started.lock().insert(id.to_string()) {
        return;
    }
    // A removal that stopped half way, in this run or an earlier one, keeps
    // the server from syncing until it is asked again and gets through.
    if app.state::<AppState>().spaces.is_leaving(id) {
        app.state::<Syncs>()
            .of(id)
            .held
            .store(true, Ordering::SeqCst);
    }
    let (worker_app, worker_id) = (app.clone(), id.to_string());
    std::thread::Builder::new()
        .name("uwussh-sync".into())
        .spawn(move || worker(worker_app, worker_id))
        .expect("the sync thread starts");
    crate::lock::start_live(app, id);
}

fn worker(app: AppHandle, id: String) {
    let state = app.state::<AppState>();
    let sync = app.state::<Syncs>().of(&id);
    // A first pass shortly after start, so a device that was off for a week
    // catches up without waiting a minute.
    let mut next_pull = Instant::now() + Duration::from_secs(3);
    let mut not_before = Instant::now();
    loop {
        let asked = {
            let mut wake = sync.wake.lock();
            if !*wake {
                sync.woken.wait_for(&mut wake, TICK);
            }
            std::mem::take(&mut *wake)
        };
        // Looked up every time and held for one round only: a server that
        // was removed leaves nothing behind that keeps its file open.
        if sync.gone.load(Ordering::SeqCst) {
            return;
        }
        if sync.held.load(Ordering::SeqCst) {
            continue;
        }
        let Some(space) = state.spaces.get(&id) else {
            return;
        };
        let store = space.store;
        let paired = store.sync_state().map(|s| s.paired()).unwrap_or(false);
        let on_lock = store
            .lock_state()
            .map(|s| s.active && s.signed_in)
            .unwrap_or(false);
        let unlocked = matches!(store.vault_status(), Ok(VaultStatus::Unlocked));
        if !(paired || on_lock) || !unlocked {
            continue;
        }
        let now = Instant::now();
        let waiting = store.pending_count().unwrap_or(0) > 0;
        // With UwULock's realtime channel open, a change elsewhere says so
        // itself; the minute's pull is only for when it is not.
        let listening = on_lock && sync.live.load(Ordering::SeqCst);
        let pull_due = now >= next_pull && !(listening && now < next_pull + QUIET_PULL);
        let due = asked || pull_due || (waiting && now >= not_before);
        if !due {
            continue;
        }
        match pass(&app, &store, &sync) {
            Ok(_) => {
                next_pull = Instant::now() + PULL_EVERY;
                not_before = Instant::now();
            }
            Err(SyncFailure::SwitchedOff) => {
                tracing::info!("UwULock has app sync switched off; looking again later");
                next_pull = Instant::now() + SWITCHED_OFF_WAIT;
                not_before = Instant::now() + SWITCHED_OFF_WAIT;
            }
            Err(failure) => {
                tracing::info!(error = %describe(&failure), "sync pass failed");
                next_pull = Instant::now() + BACKOFF;
                not_before = Instant::now() + BACKOFF;
            }
        }
    }
}

/// Run blocking sync work for one server on a worker thread, so the window
/// keeps drawing — Argon2 and SPAKE2 take a moment, and the network takes
/// longer.
pub(crate) async fn blocking<T: Send + 'static>(
    app: &AppHandle,
    space: Option<String>,
    work: impl FnOnce(&AppHandle, &Arc<Store>, &Sync) -> SyncResult<T> + Send + 'static,
) -> SyncResult<T> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let target = target(&app, space.as_deref(), false)?;
        work(&app, &target.store, &target.sync as &Sync)
    })
    .await
    .map_err(SyncFailure::error)?
}

/// How a step of connecting a server came out.
pub(crate) enum Step<T> {
    /// The server is connected.
    Done(T),
    /// The person is asked something and comes back: nothing is settled.
    More(T),
}

/// A step of connecting a server — the main one, or the second one being set
/// up, whose store is made here when there is none yet.
///
/// When the second server is connected, it becomes a space. When a flow of
/// one step fails, its store is thrown away; one of several steps (a code by
/// mail, then the code) keeps it until the person gives up
/// ([`discard_new`]). Either way a vault key may have changed, so the keys
/// kept under the main vault's are sealed again.
pub(crate) async fn setting_up<T: Send + 'static>(
    app: &AppHandle,
    space: Option<String>,
    one_step: bool,
    work: impl FnOnce(&AppHandle, &Target) -> SyncResult<Step<T>> + Send + 'static,
) -> SyncResult<T> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let target = target(&app, space.as_deref(), true)?;
        let new = target.new;
        let outcome = work(&app, &target);
        // Nothing of ours holds the store while it is registered or deleted.
        drop(target);
        match outcome {
            Ok(Step::Done(value)) => {
                if new {
                    adopt_new(&app)?;
                } else {
                    crate::spaces::rechain(&app);
                }
                Ok(value)
            }
            Ok(Step::More(value)) => Ok(value),
            Err(failure) => {
                if new && one_step {
                    discard_new(&app);
                }
                if !new {
                    // A step that failed late may have changed the key
                    // before it did.
                    crate::spaces::rechain(&app);
                }
                Err(failure)
            }
        }
    })
    .await
    .map_err(SyncFailure::error)?
}

// ── What the page shows ────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncStatus {
    paired: bool,
    server_url: Option<String>,
    /// The pinned certificate, `SHA256:…`, to compare with what the server prints.
    tls_fingerprint: Option<String>,
    device_id: Option<Uuid>,
    paired_ms: Option<u64>,
    last_sync_ms: Option<u64>,
    last: Option<LastPass>,
    /// Changes on this device the server hasn't taken yet.
    pending: usize,
    running: bool,
    vault: VaultStatus,
    /// A name for this device, to suggest when pairing.
    device_name: String,
    /// Waiting for another device to join right now.
    offering: bool,
    /// What the last complete check found the server keeping back — kept
    /// across restarts, so it shows before the first pass has run.
    withheld: Withheld,
    /// `none`, `uwusync` or `uwulock`.
    backend: &'static str,
    lock: crate::lock::LockStatus,
}

impl SyncStatus {
    pub(crate) fn of(store: &Store, sync: &Sync) -> uwussh_store::Result<Self> {
        let s = store.sync_state()?;
        let lock = store.lock_state()?;
        let backend = if lock.active {
            "uwulock"
        } else if s.paired() {
            "uwusync"
        } else {
            "none"
        };
        Ok(SyncStatus {
            paired: s.paired(),
            server_url: s.server_url,
            tls_fingerprint: s.tls_fingerprint,
            device_id: s.device_id,
            paired_ms: s.paired_ms,
            last_sync_ms: s.last_sync_ms,
            last: sync.last.lock().clone(),
            pending: store.pending_count().unwrap_or(0),
            running: sync.running.load(Ordering::SeqCst),
            vault: store.vault_status()?,
            device_name: device_name(),
            offering: sync.offer.lock().is_some(),
            withheld: store.withheld().unwrap_or_default(),
            backend,
            lock: crate::lock::LockStatus::of(lock, sync),
        })
    }

    /// A server nothing is known of yet: the second one, before its form
    /// was sent for the first time.
    fn blank() -> Self {
        let nothing = Sync::default();
        SyncStatus {
            paired: false,
            server_url: None,
            tls_fingerprint: None,
            device_id: None,
            paired_ms: None,
            last_sync_ms: None,
            last: None,
            pending: 0,
            running: false,
            vault: VaultStatus::Absent,
            device_name: device_name(),
            offering: false,
            withheld: Withheld::default(),
            backend: "none",
            lock: crate::lock::LockStatus::of(Default::default(), &nothing),
        }
    }
}

#[tauri::command]
pub(crate) fn sync_status(app: AppHandle, space: Option<String>) -> CommandResult<SyncStatus> {
    if space.as_deref() == Some(NEW_SPACE) && !app.state::<Syncs>().setting_up() {
        return Ok(SyncStatus::blank());
    }
    let target = target(&app, space.as_deref(), false).map_err(|failure| describe(&failure))?;
    SyncStatus::of(&target.store, &target.sync).map_err(err)
}

/// This computer's name, the way the other devices will see it listed.
///
/// Asked once and kept: `scutil` and NSHost can take a moment, and the sync
/// status asks often. `lib.rs` warms it on a thread at start, so no command on
/// the main thread waits for it.
pub(crate) fn device_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(look_up_device_name).clone()
}

fn look_up_device_name() -> String {
    let from_env = ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .filter(|name| !name.trim().is_empty());
    let from_file = || {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
    };
    // Apps started from the Dock or a menu get no HOSTNAME. macOS keeps the
    // name people gave their Mac apart from the network one. The Mac App
    // Store build runs no programs, so it asks Foundation for that name.
    #[cfg(all(target_os = "macos", feature = "mas"))]
    let from_command = || {
        // NSHost is deprecated for resolving names; `localizedName` is the
        // name from System Settings → General → About, all this asks for.
        #[allow(deprecated)]
        let host = objc2_foundation::NSHost::currentHost();
        host.localizedName()
            .map(|name| name.to_string().trim().to_string())
            .filter(|name| !name.is_empty())
    };
    #[cfg(not(all(target_os = "macos", feature = "mas")))]
    let from_command = || {
        let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
            ("/usr/sbin/scutil", &["--get", "ComputerName"])
        } else {
            ("hostname", &[])
        };
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|name| !name.is_empty())
    };
    from_env
        .or_else(from_file)
        .or_else(from_command)
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "Mac".into()
            } else {
                "UwUSSH".into()
            }
        })
        .chars()
        .take(64)
        .collect()
}

fn clean_name(name: &str) -> SyncResult<String> {
    let name: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    if name.is_empty() {
        return Err(SyncFailure::error("the device needs a name"));
    }
    Ok(name)
}

/// The master password is right: checked against the vault that is here,
/// before anything is wrapped again with it.
fn check_password(store: &Store, password: &[u8]) -> SyncResult<()> {
    match store.vault_status()? {
        VaultStatus::Absent => Ok(()),
        _ => store.unlock_vault(password).map_err(SyncFailure::from),
    }
}

/// Whether the vault should be remembered on this device after pairing: only
/// if it was before. A remembered vault opens for anything running as this
/// user, so that is a choice the person makes where they can see it — the
/// vault dialog's checkbox, the next time it asks — never one pairing makes
/// for them.
pub(crate) fn wants_remembered(store: &Store) -> bool {
    store.vault_is_remembered().unwrap_or(false)
}

/// Keep a remembered vault remembered: a vault key that changed (joining
/// another account's vault) needs sealing again.
///
/// The main vault only. What a second server's store keeps is its key under
/// the main vault's, never under the operating system's: that is sealed
/// again when the step is done (see [`setting_up`]).
pub(crate) fn keep_remembered(target: &Target, was_remembered: bool) {
    if was_remembered && target.main {
        if let Err(error) = target.store.remember_vault(crate::device::protect) {
            tracing::warn!(%error, "could not remember the vault again");
        }
    }
}

// ── Connecting ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Connected {
    /// The account key as the recovery kit shows it: right after the account
    /// was made, and again on a paired device for its master password.
    recovery_code: String,
    server_url: String,
    tls_fingerprint: Option<String>,
}

/// First device: a setup code from `uwusync-server invite`, the master
/// password, a name. Makes the account and hands back the recovery kit.
#[tauri::command]
pub(crate) async fn sync_connect(
    app: AppHandle,
    code: String,
    password: String,
    name: String,
    space: Option<String>,
) -> SyncResult<Connected> {
    let password = Zeroizing::new(password);
    let setup = flow::parse_setup(&code).ok_or(SyncFailure::BadCode)?;
    let name = clean_name(&name)?;
    setting_up(&app, space, true, move |app, target| {
        let (store, sync) = (&target.store, &*target.sync);
        if store.sync_state()?.paired() {
            return Err(SyncFailure::error("this device is already paired"));
        }
        if store.lock_state()?.active {
            return Err(SyncFailure::error("this device syncs through UwULock"));
        }
        if password.trim().is_empty() {
            return Err(SyncFailure::error("the master password cannot be empty"));
        }
        check_password(store, password.as_bytes())?;
        let remembered = wants_remembered(store);
        let (server, paired) = flow::create_account(
            store,
            &setup,
            password.as_bytes(),
            &name,
            crate::device::protect_sync,
        )?;
        keep_remembered(target, remembered);
        let recovery_code = paired
            .recovery
            .as_ref()
            .map(|key| key.to_code())
            .unwrap_or_default();
        *sync.server.lock() = Some(Arc::new(server));
        sync.poke();
        let _ = app.emit("sync:status", ());
        Ok(Step::Done(Connected {
            recovery_code,
            server_url: setup.server_url,
            tls_fingerprint: setup.tls_fingerprint,
        }))
    })
    .await
}

/// A code another device showed, as pasted (`uwu2_…`) or as read out
/// (`K7M4Q-tiger-radio-kiwi`, with the server's address and fingerprint).
fn target_of(
    code: &str,
    server_url: Option<&str>,
    fingerprint: Option<&str>,
) -> Option<pairing::Target> {
    if let Some(target) = pairing::parse_offer(code) {
        return Some(target);
    }
    let (id, words) = pairing::parse_spoken(code)?;
    let server_url = server_url?.trim();
    if server_url.is_empty() {
        return None;
    }
    Some(pairing::Target {
        server_url: server_url.to_string(),
        tls_fingerprint: fingerprint
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(str::to_string),
        id,
        words,
    })
}

/// Another device: the code it showed, the account's master password, a name.
#[tauri::command]
pub(crate) async fn sync_join(
    app: AppHandle,
    code: String,
    server_url: Option<String>,
    fingerprint: Option<String>,
    password: String,
    name: String,
    space: Option<String>,
) -> SyncResult<()> {
    let password = Zeroizing::new(password);
    let offered = target_of(&code, server_url.as_deref(), fingerprint.as_deref())
        .ok_or(SyncFailure::BadCode)?;
    let name = clean_name(&name)?;
    setting_up(&app, space, true, move |app, target| {
        let (store, sync) = (&target.store, &*target.sync);
        if store.sync_state()?.paired() {
            return Err(SyncFailure::error("this device is already paired"));
        }
        if store.lock_state()?.active {
            return Err(SyncFailure::error("this device syncs through UwULock"));
        }
        // Hosts and secrets already here come along into the account's vault,
        // which needs them opened first — with this device's own password.
        if store.vault_status()? == VaultStatus::Locked {
            return Err(SyncFailure::VaultLocked);
        }
        let remembered = wants_remembered(store);
        let (server, _) = flow::join(
            store,
            &offered,
            password.as_bytes(),
            &name,
            crate::device::protect_sync,
            Instant::now() + pairing::DEADLINE,
        )?;
        keep_remembered(target, remembered);
        *sync.server.lock() = Some(Arc::new(server));
        sync.poke();
        let _ = app.emit("sync:status", ());
        Ok(Step::Done(()))
    })
    .await
}

// ── Adding a device from here ──────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OfferShown {
    /// What to read out.
    spoken: String,
    /// What to paste, when the two can copy and paste.
    pasteable: String,
    server_url: String,
    tls_fingerprint: Option<String>,
    /// When the code stops working.
    expires_ms: u64,
}

/// The master password first: the other end of the code receives the account
/// key, and a window left open should not hand that to whoever sits down at it.
#[tauri::command]
pub(crate) async fn sync_offer(
    app: AppHandle,
    password: String,
    space: Option<String>,
) -> SyncResult<OfferShown> {
    let password = Zeroizing::new(password);
    blocking(&app, space, move |_, store, sync| {
        let header = store
            .vault_header()?
            .ok_or_else(|| SyncFailure::error("there is no vault"))?;
        let account_key = flow::account_key(store, crate::device::unprotect_sync)?;
        header
            .unlock_with(password.as_bytes(), account_key.as_ref())
            .map_err(|_| SyncFailure::PasswordWrong)?;
        let server = server(store, sync)?;
        let offer = flow::offer_pairing(store, &server)?;
        let state = store.sync_state()?;
        let shown = OfferShown {
            spoken: offer.offer.spoken.clone(),
            pasteable: offer.offer.pasteable.clone(),
            server_url: state.server_url.unwrap_or_default(),
            tls_fingerprint: state.tls_fingerprint,
            expires_ms: now_ms() + pairing::DEADLINE.as_millis() as u64,
        };
        // A new code replaces an old one; the old session closes.
        if let Some(old) = sync.offer.lock().replace(Arc::new(offer)) {
            let _ = server.close_pairing(&old.offer.id);
        }
        Ok(shown)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceJoined {
    name: String,
}

/// Wait for the device the code was read out to. Returns when it joined, the
/// code ran out, or the words didn't match.
#[tauri::command]
pub(crate) async fn sync_wait_for_device(
    app: AppHandle,
    space: Option<String>,
) -> SyncResult<DeviceJoined> {
    blocking(&app, space, |app, store, sync| {
        let offer = sync
            .offer
            .lock()
            .clone()
            .ok_or_else(|| SyncFailure::error("no code is being shown"))?;
        if sync.waiting.swap(true, Ordering::SeqCst) {
            return Err(SyncFailure::error("this code is already being waited on"));
        }
        struct Waiting<'a>(&'a AtomicBool);
        impl Drop for Waiting<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _waiting = Waiting(&sync.waiting);
        let server = server(store, sync)?;
        let joined = flow::wait_for_device(
            store,
            &server,
            &offer,
            crate::device::unprotect_sync,
            Instant::now() + pairing::DEADLINE,
        );
        // This offer is done, whatever came of it — unless a newer one took
        // its place meanwhile.
        {
            let mut current = sync.offer.lock();
            if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &offer)) {
                *current = None;
            }
        }
        let joined = joined?;
        let _ = app.emit("sync:status", ());
        Ok(DeviceJoined {
            name: joined.device_name,
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn sync_cancel_offer(app: AppHandle, space: Option<String>) -> SyncResult<()> {
    blocking(&app, space, |_, store, sync| {
        if let Some(offer) = sync.offer.lock().take() {
            if let Ok(server) = server(store, sync) {
                let _ = server.close_pairing(&offer.offer.id);
            }
        }
        Ok(())
    })
    .await
}

// ── Devices ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceRow {
    id: Uuid,
    name: String,
    created_ms: u64,
    last_seen_ms: Option<u64>,
    revoked_ms: Option<u64>,
    /// This device.
    current: bool,
}

#[tauri::command]
pub(crate) async fn sync_devices(
    app: AppHandle,
    space: Option<String>,
) -> SyncResult<Vec<DeviceRow>> {
    blocking(&app, space, |_, store, sync| {
        let me = store.sync_state()?.device_id;
        let server = server(store, sync)?;
        let devices = server
            .devices()
            .inspect_err(|_| *sync.server.lock() = None)?;
        Ok(devices
            .into_iter()
            .map(|device| DeviceRow {
                current: Some(device.id) == me,
                id: device.id,
                name: device.name,
                created_ms: device.created_ms,
                last_seen_ms: device.last_seen_ms,
                revoked_ms: device.revoked_ms,
            })
            .collect())
    })
    .await
}

/// Shut another device out. Needs the master password: the login key comes
/// from it, so a stolen device can't lock its owner out.
#[tauri::command]
pub(crate) async fn sync_revoke(
    app: AppHandle,
    device_id: Uuid,
    password: String,
    space: Option<String>,
) -> SyncResult<()> {
    let password = Zeroizing::new(password);
    blocking(&app, space, move |_, store, sync| {
        let state = store.sync_state()?;
        if state.device_id == Some(device_id) {
            return Err(SyncFailure::error(
                "this device leaves with “Disconnect”, not by revoking itself",
            ));
        }
        let header = store
            .vault_header()?
            .ok_or_else(|| SyncFailure::error("there is no vault"))?;
        let account_key = flow::account_key(store, crate::device::unprotect_sync)?;
        let login_key = header
            .login_key(password.as_bytes(), account_key.as_ref())
            .map_err(|_| SyncFailure::PasswordWrong)?;
        let server = server(store, sync)?;
        server.revoke(device_id, Some(login_key.as_slice()))?;
        Ok(())
    })
    .await
}

/// A pass now, instead of at the next tick: on one server, or on every one
/// when none is named.
#[tauri::command]
pub(crate) fn sync_now(syncs: State<'_, Syncs>, space: Option<String>) {
    match space {
        None => syncs.poke(),
        Some(id) => {
            if let Some(sync) = syncs.of.lock().get(&id) {
                sync.poke();
            }
        }
    }
}

/// A pass now, waited for: a `uwussh://` link to a host this device doesn't
/// know yet looks again after it. False when there is nothing to sync with.
///
/// Without a server named, the other one is asked too — the host may be on
/// either — but only the main one's trouble is this command's.
#[tauri::command]
pub(crate) async fn sync_pass_now(app: AppHandle, space: Option<String>) -> SyncResult<bool> {
    let everywhere = space.is_none();
    let handle = app.clone();
    let passed = blocking(&app, space, pass_now).await?;
    if !everywhere {
        return Ok(passed);
    }
    let others = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let syncs = handle.state::<Syncs>();
        let mut passed = false;
        for space in state.spaces.all().into_iter().filter(|space| !space.main) {
            passed |= pass_now(&handle, &space.store, &syncs.of(&space.id)).unwrap_or(false);
        }
        passed
    })
    .await
    .unwrap_or(false);
    Ok(passed || others)
}

fn pass_now(app: &AppHandle, store: &Arc<Store>, sync: &Sync) -> SyncResult<bool> {
    let paired = store.sync_state()?.paired();
    let lock = store.lock_state()?;
    if !(paired || (lock.active && lock.signed_in)) {
        return Ok(false);
    }
    // A pass already under way may have started before the record
    // arrived: wait for it, then run one of our own.
    let wait_for_running = || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while sync.running.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    wait_for_running();
    match pass(app, store, sync) {
        Ok(_) => Ok(true),
        // The worker started one in between; that one is as fresh.
        Err(_) if sync.running.load(Ordering::SeqCst) => {
            wait_for_running();
            Ok(true)
        }
        Err(failure) => Err(failure),
    }
}

/// Stop syncing on this device. The hosts stay; the vault is wrapped again
/// without the account key, so the master password alone opens it once more.
/// The server is told this device is gone, as far as it can be reached.
#[tauri::command]
pub(crate) async fn sync_disconnect(
    app: AppHandle,
    password: String,
    space: Option<String>,
) -> SyncResult<()> {
    let password = Zeroizing::new(password);
    blocking(&app, space, move |app, store, sync| {
        let state = store.sync_state()?;
        if !state.paired() {
            return Ok(());
        }
        // The password first, against the vault as it is: wrong means nothing
        // changes.
        let header = store
            .vault_header()?
            .ok_or_else(|| SyncFailure::error("there is no vault"))?;
        let account_key = flow::account_key(store, crate::device::unprotect_sync)?;
        let unlocked = header
            .unlock_with(password.as_bytes(), account_key.as_ref())
            .map_err(|_| SyncFailure::PasswordWrong)?;
        drop(unlocked);
        if store.vault_status()? != VaultStatus::Unlocked {
            store.unlock_vault_with(password.as_bytes(), account_key.as_ref())?;
        }
        if let (Some(me), Ok(server)) = (state.device_id, server(store, sync)) {
            if let Err(error) = server.revoke(me, None) {
                tracing::info!(%error, "the server could not be told this device left");
            }
        }
        store.rewrap_vault(password.as_bytes(), None)?;
        store.forget_enrolment()?;
        *sync.server.lock() = None;
        *sync.last.lock() = None;
        if let Some(offer) = sync.offer.lock().take() {
            drop(offer);
        }
        let _ = app.emit("sync:status", ());
        // A second server's vault was opened with its own password, perhaps;
        // and the server's name is gone from the list.
        crate::spaces::rechain(app);
        Ok(())
    })
    .await
}

/// The recovery kit again, for a device that kept the account key: the kit
/// shown when the account was made is easily lost, and every paired device
/// holds the key anyway. The master password first, so an unattended but
/// unlocked window does not hand it out.
#[tauri::command]
pub(crate) async fn sync_recovery_code(
    app: AppHandle,
    password: String,
    space: Option<String>,
) -> SyncResult<Connected> {
    let password = Zeroizing::new(password);
    blocking(&app, space, move |_, store, _| {
        let state = store.sync_state()?;
        if !state.paired() {
            return Err(SyncFailure::error("this device is not paired"));
        }
        let header = store
            .vault_header()?
            .ok_or_else(|| SyncFailure::error("there is no vault"))?;
        let account_key = flow::account_key(store, crate::device::unprotect_sync)?
            .ok_or_else(|| SyncFailure::error("this device kept no account key"))?;
        header
            .unlock_with(password.as_bytes(), Some(&account_key))
            .map_err(|_| SyncFailure::PasswordWrong)?;
        Ok(Connected {
            recovery_code: account_key.to_code(),
            server_url: state.server_url.unwrap_or_default(),
            tls_fingerprint: state.tls_fingerprint,
        })
    })
    .await
}

/// The account key this device kept, for unlocking a vault that needs one.
/// `None` when it isn't paired, or what it kept no longer opens.
pub(crate) fn kept_account_key(store: &Store) -> Option<uwussh_vault::AccountKey> {
    flow::account_key(store, crate::device::unprotect_sync)
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spoken_code_needs_the_server_it_belongs_to() {
        let words = pairing::words();
        let spoken = format!("K7M4Q-{words}");
        assert!(target_of(&spoken, None, None).is_none());
        assert!(target_of(&spoken, Some("  "), None).is_none());
        let target = target_of(
            &spoken,
            Some("https://nas.example:8443"),
            Some(" SHA256:abc "),
        )
        .unwrap();
        assert_eq!(target.id, "K7M4Q");
        assert_eq!(target.words, words);
        assert_eq!(target.tls_fingerprint.as_deref(), Some("SHA256:abc"));
        assert!(target_of("K7M4Q-not-our-words", Some("https://nas.example"), None).is_none());
    }

    #[test]
    fn a_pasted_code_brings_its_own_server() {
        let offer = pairing::offer("K7M4Q", &pairing::words(), "https://nas.example:8443", None);
        let target = target_of(&offer.pasteable, None, None).unwrap();
        assert_eq!(target.server_url, "https://nas.example:8443");
    }

    #[test]
    fn a_device_name_is_one_line_and_not_empty() {
        assert_eq!(clean_name("  Laptop\n\t ").unwrap(), "Laptop");
        assert!(clean_name(" \n ").is_err());
        assert_eq!(clean_name(&"x".repeat(200)).unwrap().len(), 64);
    }
}
