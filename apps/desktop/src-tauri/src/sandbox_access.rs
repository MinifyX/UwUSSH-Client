//! Folders and files the person picked, kept reachable inside the Mac App
//! Store's sandbox.
//!
//! A sandboxed app may open what the person handed it through an open or save
//! panel, and only for as long as it runs. The next start, a key file a host
//! points at, the `~/.ssh` folder an import read, a folder the file browser
//! offered — each is a path the app is no longer allowed to read. What
//! survives a restart is a *security-scoped bookmark*: an opaque blob the
//! system makes for a path the app may reach right now, and which, resolved on
//! a later run, gives that access back.
//!
//! So every user-picked path the app keeps for later gets a bookmark, in
//! `bookmarks.json` next to the database, and at start-up every bookmark is
//! resolved and its access switched on before the page asks for anything. A
//! file inside a folder that already has one needs none of its own. The
//! folders the file browser lists for this computer ([`places`]) are kept in
//! the same file.
//!
//! Everywhere else — Windows, Linux, the Mac build from GitHub, which is not
//! sandboxed — these functions do nothing at all, and [`places`] is empty: the
//! file browser starts from the home folder and the drives there.

use std::path::{Path, PathBuf};

/// Resolves the stored bookmarks and switches their access on, for the rest of
/// the run. Called once from `setup`, before the window loads the page.
pub(crate) fn restore(config_directory: &Path) {
    #[cfg(all(target_os = "macos", feature = "mas"))]
    scoped::restore(config_directory);
    #[cfg(not(all(target_os = "macos", feature = "mas")))]
    let _ = config_directory;
}

/// Keeps access to `path` across restarts, if it does not have that already.
/// Cheap when it does: a lookup, no system call. Never fails — a path the app
/// cannot bookmark is a path it could not open in the first place.
pub(crate) fn remember(path: &Path) {
    #[cfg(all(target_os = "macos", feature = "mas"))]
    scoped::remember(path, false);
    #[cfg(not(all(target_os = "macos", feature = "mas")))]
    let _ = path;
}

/// [`remember`], and list the folder among this computer's places in the file
/// browser.
pub(crate) fn add_place(path: &Path) {
    #[cfg(all(target_os = "macos", feature = "mas"))]
    scoped::remember(path, true);
    #[cfg(not(all(target_os = "macos", feature = "mas")))]
    let _ = path;
}

/// Takes a folder off the file browser's places, and gives up its bookmark —
/// from the next start on, the app cannot reach it any more. A bookmark of a
/// folder above it, or of `~/.ssh` or a key file inside it, stays.
pub(crate) fn forget_place(path: &Path) {
    #[cfg(all(target_os = "macos", feature = "mas"))]
    scoped::forget_place(path);
    #[cfg(not(all(target_os = "macos", feature = "mas")))]
    let _ = path;
}

/// The folders the person added to the file browser's places, oldest first.
pub(crate) fn places() -> Vec<PathBuf> {
    #[cfg(all(target_os = "macos", feature = "mas"))]
    return scoped::places();
    #[cfg(not(all(target_os = "macos", feature = "mas")))]
    Vec::new()
}

/// The bookkeeping, apart from the system calls, so it can be tested anywhere.
#[cfg(any(test, all(target_os = "macos", feature = "mas")))]
mod store {
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Serialize};

    /// Enough for every key file and folder anybody uses, and a ceiling on a
    /// file that is read at every start.
    pub(super) const LIMIT: usize = 256;

    #[derive(Debug, Default, Serialize, Deserialize)]
    pub(super) struct Bookmarks {
        /// Oldest first, so the ceiling drops what was picked longest ago.
        pub entries: Vec<Entry>,
        /// The file browser's own folders. Each one is reachable through an
        /// entry — its own or a folder's above it.
        #[serde(default)]
        pub places: Vec<PathBuf>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub(super) struct Entry {
        pub path: PathBuf,
        /// The bookmark, hex-encoded so the file stays plain JSON.
        pub bookmark: String,
        /// Whether it gives access in this run. One that did not resolve at
        /// start-up (a disk not plugged in, a share not mounted) is kept for
        /// the next start, but covers nothing now: picking the path again
        /// makes a new one.
        #[serde(skip)]
        pub live: bool,
        /// Made only for a folder of the file browser: taking the folder off
        /// its places gives it up. Anything else the app needs (`~/.ssh`, a
        /// key file) gets a bookmark of its own even inside such a folder.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pub place_only: bool,
    }

    impl Bookmarks {
        pub fn parse(text: &str) -> Self {
            serde_json::from_str(text).unwrap_or_default()
        }

        /// Whether `path` or a folder it is in has a bookmark that gives
        /// access in this run and stays when a place is forgotten. A
        /// place's own bookmark covers only the place itself (and is then
        /// [`claim`](Self::claim)ed).
        pub fn covers(&self, path: &Path) -> bool {
            self.entries.iter().any(|entry| {
                entry.live
                    && path.starts_with(&entry.path)
                    && (!entry.place_only || entry.path == path)
            })
        }

        /// Whether a file browser folder at `path` is reachable already.
        pub fn covers_place(&self, path: &Path) -> bool {
            self.entries
                .iter()
                .any(|entry| entry.live && path.starts_with(&entry.path))
        }

        /// Adds a lasting bookmark for `path`, replacing one it had. Those of
        /// paths inside it stay: they may be needed on their own.
        pub fn add(&mut self, path: PathBuf, bookmark: &[u8]) {
            self.push(Entry {
                path,
                bookmark: hex(bookmark),
                live: true,
                place_only: false,
            });
        }

        /// Adds the bookmark of a file browser folder.
        pub fn add_for_place(&mut self, path: PathBuf, bookmark: &[u8]) {
            self.push(Entry {
                path,
                bookmark: hex(bookmark),
                live: true,
                place_only: true,
            });
        }

        /// A place's own bookmark is needed for more than the place now.
        pub fn claim(&mut self, path: &Path) {
            for entry in &mut self.entries {
                if entry.path == path {
                    entry.place_only = false;
                }
            }
        }

        /// Keeps a bookmark that did not resolve this time, for the next start.
        pub fn keep(&mut self, entry: Entry) {
            self.push(Entry {
                live: false,
                ..entry
            });
        }

        fn push(&mut self, entry: Entry) {
            self.entries.retain(|kept| kept.path != entry.path);
            self.entries.push(entry);
            let excess = self.entries.len().saturating_sub(LIMIT);
            self.entries.drain(..excess);
            self.drop_unreachable_places();
        }

        pub fn add_place(&mut self, path: &Path) {
            if !self.places.iter().any(|place| place == path) {
                self.places.push(path.to_path_buf());
            }
        }

        /// Takes the place off the list, and its own bookmark with it unless
        /// something else needs it. A bookmark of a folder above it serves
        /// other paths too and stays.
        pub fn forget_place(&mut self, path: &Path) {
            self.places.retain(|place| place != path);
            self.entries
                .retain(|entry| !(entry.place_only && entry.path == path));
        }

        /// A place whose bookmark is gone — dropped past the ceiling — is a
        /// folder the app cannot open any more. One whose bookmark only did
        /// not resolve this time stays listed: its disk may be back next time.
        pub fn drop_unreachable_places(&mut self) {
            let places = std::mem::take(&mut self.places);
            self.places = places
                .into_iter()
                .filter(|place| {
                    self.entries
                        .iter()
                        .any(|entry| place.starts_with(&entry.path))
                })
                .collect();
        }
    }

    pub(super) fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    pub(super) fn unhex(text: &str) -> Option<Vec<u8>> {
        if !text.len().is_multiple_of(2) {
            return None;
        }
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
            .collect()
    }
}

#[cfg(all(target_os = "macos", feature = "mas"))]
mod scoped {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use objc2::runtime::Bool;
    use objc2_foundation::{
        NSData, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions, NSURL,
    };

    use super::store::{unhex, Bookmarks};

    const FILE: &str = "bookmarks.json";

    /// The bookmarks and the file they live in. `None` until [`restore`] ran.
    static STATE: Mutex<Option<(PathBuf, Bookmarks)>> = Mutex::new(None);

    pub(super) fn restore(config_directory: &Path) {
        let file = config_directory.join(FILE);
        let stored = std::fs::read_to_string(&file)
            .map(|text| Bookmarks::parse(&text))
            .unwrap_or_default();

        // The places once every bookmark is back, or each `add` would drop
        // the ones whose bookmark comes later in the list.
        let mut restored = Bookmarks::default();
        for entry in stored.entries {
            let Some(bytes) = unhex(&entry.bookmark) else {
                continue;
            };
            match resolve(&bytes) {
                // A stale bookmark still works this once; a fresh one is made
                // now, while the access it gave is switched on.
                Some((path, stale)) => {
                    let fresh = if stale { create(&path) } else { None };
                    let bookmark = fresh.as_deref().unwrap_or(&bytes);
                    if entry.place_only {
                        restored.add_for_place(path, bookmark);
                    } else {
                        restored.add(path, bookmark);
                    }
                }
                // Deleted, on a volume that is not mounted, or no longer ours
                // to open: no access this run, but kept, so an external disk
                // or a share is back on the next start with it. A host that
                // names a key file in there reports it unreadable, and the
                // page offers to pick it again (a new bookmark replaces it).
                None => {
                    tracing::info!(path = %entry.path.display(), "bookmark does not resolve now");
                    restored.keep(entry);
                }
            }
        }
        restored.places = stored.places;
        restored.drop_unreachable_places();
        tracing::info!(
            count = restored.entries.len(),
            places = restored.places.len(),
            "security-scoped bookmarks restored"
        );
        save(&file, &restored);
        *lock() = Some((file, restored));
    }

    pub(super) fn remember(path: &Path, place: bool) {
        let mut state = lock();
        let Some((file, bookmarks)) = state.as_mut() else {
            return;
        };
        let covered = if place {
            bookmarks.covers_place(path)
        } else {
            bookmarks.covers(path)
        };
        if !covered {
            let Some(bookmark) = create(path) else {
                tracing::debug!(path = %path.display(), "no bookmark for this path");
                return;
            };
            if place {
                bookmarks.add_for_place(path.to_path_buf(), &bookmark);
            } else {
                bookmarks.add(path.to_path_buf(), &bookmark);
            }
        } else if !place {
            bookmarks.claim(path);
        }
        if place {
            bookmarks.add_place(path);
        }
        save(file, bookmarks);
    }

    pub(super) fn forget_place(path: &Path) {
        let mut state = lock();
        if let Some((file, bookmarks)) = state.as_mut() {
            bookmarks.forget_place(path);
            save(file, bookmarks);
        }
    }

    pub(super) fn places() -> Vec<PathBuf> {
        lock()
            .as_ref()
            .map(|(_, bookmarks)| bookmarks.places.clone())
            .unwrap_or_default()
    }

    fn lock() -> std::sync::MutexGuard<'static, Option<(PathBuf, Bookmarks)>> {
        STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A security-scoped bookmark for a path this process can open right now.
    fn create(path: &Path) -> Option<Vec<u8>> {
        let url = if path.is_dir() {
            NSURL::from_directory_path(path)?
        } else {
            NSURL::from_file_path(path)?
        };
        url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions::WithSecurityScope,
            None,
            None,
        )
        .ok()
        .map(|data| data.to_vec())
    }

    /// Resolves a bookmark and switches its access on for the rest of the run.
    /// The path it points at now — a file moved in Finder keeps its bookmark —
    /// and whether the bookmark should be made again.
    fn resolve(bytes: &[u8]) -> Option<(PathBuf, bool)> {
        let data = NSData::with_bytes(bytes);
        let mut stale = Bool::NO;
        // Never a dialog and never a network volume mounted at start-up: a
        // share that is not there costs that one folder, not a hung launch.
        let options = NSURLBookmarkResolutionOptions::WithSecurityScope
            | NSURLBookmarkResolutionOptions::WithoutUI
            | NSURLBookmarkResolutionOptions::WithoutMounting;
        // SAFETY: `stale` is a valid, writable `Bool` for the whole call.
        let url = unsafe {
            NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                &data, options, None, &mut stale,
            )
        }
        .ok()?;
        let path = url.to_file_path()?;
        // SAFETY: a plain message to a valid file URL. It is balanced by the
        // end of the process rather than by `stopAccessingSecurityScopedResource`:
        // a key file is read again at every connection, a folder stays open in
        // the file browser, for as long as the app runs.
        if !unsafe { url.startAccessingSecurityScopedResource() } {
            return None;
        }
        // Kept alive on purpose, see above.
        std::mem::forget(url);
        Some((path, stale.as_bool()))
    }

    /// Written next to the database, through a temporary file so a crash
    /// mid-write never leaves half a JSON document behind.
    fn save(file: &Path, bookmarks: &Bookmarks) {
        let Ok(text) = serde_json::to_string(bookmarks) else {
            return;
        };
        let temporary = file.with_extension("json.tmp");
        if let Some(folder) = file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Err(error) =
            std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, file))
        {
            tracing::warn!(%error, "bookmarks could not be saved");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::store::{hex, unhex, Bookmarks, Entry, LIMIT};

    #[test]
    fn a_file_inside_a_bookmarked_folder_needs_no_bookmark_of_its_own() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add(PathBuf::from("/Users/nyu/.ssh"), b"folder");
        assert!(bookmarks.covers(Path::new("/Users/nyu/.ssh/id_ed25519")));
        assert!(bookmarks.covers(Path::new("/Users/nyu/.ssh")));
        // A shared prefix of characters is not a shared folder.
        assert!(!bookmarks.covers(Path::new("/Users/nyu/.ssh-old/id_rsa")));
    }

    #[test]
    fn a_folder_keeps_the_bookmarks_inside_it() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add(PathBuf::from("/Users/nyu/.ssh"), b"ssh");
        bookmarks.add_for_place(PathBuf::from("/Users/nyu"), b"home");
        bookmarks.add_place(Path::new("/Users/nyu"));
        // The same path again replaces its bookmark.
        bookmarks.add_for_place(PathBuf::from("/Users/nyu"), b"home2");
        assert_eq!(bookmarks.entries.len(), 2);

        // Taking the home folder off the places must not cost ~/.ssh.
        bookmarks.forget_place(Path::new("/Users/nyu"));
        assert!(bookmarks.covers(Path::new("/Users/nyu/.ssh/config")));
        assert!(!bookmarks.covers(Path::new("/Users/nyu/Documents")));
    }

    #[test]
    fn what_is_needed_inside_a_place_gets_its_own_bookmark() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add_for_place(PathBuf::from("/Users/nyu"), b"home");
        bookmarks.add_place(Path::new("/Users/nyu"));
        // Reachable through the place, but that one may go: ~/.ssh needs its own.
        assert!(bookmarks.covers_place(Path::new("/Users/nyu/.ssh")));
        assert!(!bookmarks.covers(Path::new("/Users/nyu/.ssh")));
        bookmarks.add(PathBuf::from("/Users/nyu/.ssh"), b"ssh");
        bookmarks.forget_place(Path::new("/Users/nyu"));
        assert!(bookmarks.covers(Path::new("/Users/nyu/.ssh/id_ed25519")));
        assert_eq!(bookmarks.entries.len(), 1);
    }

    #[test]
    fn a_place_needed_for_more_keeps_its_bookmark() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add_for_place(PathBuf::from("/Users/nyu/.ssh"), b"ssh");
        bookmarks.add_place(Path::new("/Users/nyu/.ssh"));
        // The import then asks for the very same folder.
        assert!(bookmarks.covers(Path::new("/Users/nyu/.ssh")));
        bookmarks.claim(Path::new("/Users/nyu/.ssh"));
        bookmarks.forget_place(Path::new("/Users/nyu/.ssh"));
        assert!(bookmarks.places.is_empty());
        assert!(bookmarks.covers(Path::new("/Users/nyu/.ssh/config")));
    }

    #[test]
    fn a_bookmark_that_does_not_resolve_is_kept_but_covers_nothing() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.keep(Entry {
            path: PathBuf::from("/Volumes/Backup"),
            bookmark: hex(b"disk"),
            live: true,
            place_only: true,
        });
        bookmarks.add_place(Path::new("/Volumes/Backup"));
        bookmarks.drop_unreachable_places();
        // Still listed and still in the file for the next start ...
        assert_eq!(bookmarks.places, [PathBuf::from("/Volumes/Backup")]);
        assert_eq!(bookmarks.entries.len(), 1);
        // ... but no access now, so picking a file there makes a new bookmark.
        assert!(!bookmarks.covers_place(Path::new("/Volumes/Backup/key")));
        bookmarks.add_for_place(PathBuf::from("/Volumes/Backup"), b"again");
        assert!(bookmarks.covers_place(Path::new("/Volumes/Backup/key")));
        assert_eq!(bookmarks.entries.len(), 1);
    }

    #[test]
    fn the_oldest_bookmarks_go_first_past_the_ceiling() {
        let mut bookmarks = Bookmarks::default();
        for i in 0..LIMIT + 3 {
            bookmarks.add(PathBuf::from(format!("/f/{i}")), &[1]);
        }
        assert_eq!(bookmarks.entries.len(), LIMIT);
        assert_eq!(bookmarks.entries[0].path, PathBuf::from("/f/3"));
    }

    #[test]
    fn a_place_lives_as_long_as_its_bookmark() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add_for_place(PathBuf::from("/Users/nyu/Projects"), b"p");
        bookmarks.add_place(Path::new("/Users/nyu/Projects"));
        bookmarks.add_place(Path::new("/Users/nyu/Projects"));
        assert_eq!(bookmarks.places, [PathBuf::from("/Users/nyu/Projects")]);

        // Pushed out past the ceiling: the folder is out of reach, so it goes.
        for i in 0..LIMIT {
            bookmarks.add(PathBuf::from(format!("/f/{i}")), &[1]);
        }
        assert!(bookmarks.places.is_empty());
    }

    #[test]
    fn forgetting_a_place_keeps_what_others_need() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add_for_place(PathBuf::from("/Users/nyu"), b"home");
        bookmarks.add_place(Path::new("/Users/nyu"));
        bookmarks.add_place(Path::new("/Users/nyu/Projects"));
        bookmarks.forget_place(Path::new("/Users/nyu/Projects"));
        // Its folder's bookmark above it stays: it was never its own.
        assert_eq!(bookmarks.places, [PathBuf::from("/Users/nyu")]);
        assert_eq!(bookmarks.entries.len(), 1);
        bookmarks.forget_place(Path::new("/Users/nyu"));
        assert!(bookmarks.places.is_empty());
        assert!(bookmarks.entries.is_empty());
    }

    #[test]
    fn bookmarks_survive_the_round_trip_through_their_file() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.add(PathBuf::from("/Users/nyu/.ssh"), &[0, 1, 0xab, 0xff]);
        bookmarks.add_place(Path::new("/Users/nyu/.ssh"));
        let text = serde_json::to_string(&bookmarks).unwrap();
        let back = Bookmarks::parse(&text);
        let saved = |list: &Bookmarks| -> Vec<(PathBuf, String)> {
            list.entries
                .iter()
                .map(|entry| (entry.path.clone(), entry.bookmark.clone()))
                .collect()
        };
        assert_eq!(saved(&back), saved(&bookmarks));
        // Read back, nothing gives access until it is resolved again.
        assert!(!back.covers(Path::new("/Users/nyu/.ssh")));
        assert_eq!(back.places, bookmarks.places);
        assert_eq!(
            unhex(&back.entries[0].bookmark).unwrap(),
            [0, 1, 0xab, 0xff]
        );
        // A file from before places existed still reads.
        assert!(Bookmarks::parse(r#"{"entries":[]}"#).places.is_empty());
        // A damaged file is an empty list, not a failed start.
        assert!(Bookmarks::parse("{ not json").entries.is_empty());
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(hex(&[]), "");
    }
}
