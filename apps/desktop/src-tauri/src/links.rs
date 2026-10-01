//! `uwussh://connect/<host id>`: a link that opens a terminal to one host.
//!
//! UwULock's web vault and apps offer it as "In UwUSSH öffnen". Only the
//! host's record id travels in the link — never an address, a user name or a
//! secret — so a link from anywhere can at most open a host that is already
//! in this device's list (or arrives with the next sync), and every question
//! a connection asks (host key, vault, password) is still asked.
//!
//! The system starts UwUSSH with the link as its argument (Windows, Linux) or
//! hands it to the running app (macOS). The single-instance plugin forwards a
//! second start's link to the first instance and the deep-link plugin hands it
//! here; the window comes to the front and the page is told to fetch the id.
//! The id waits in [`Pending`] until the page takes it, so a link that starts
//! the app is not lost while the page is still loading.
//!
//! Registering the scheme is the installers' job: `plugins.deep-link` in
//! `tauri.conf.json` puts it into the macOS `Info.plist` and the `MimeType`
//! of the .deb and .rpm desktop entry, the Windows setup writes
//! `HKCU\Software\Classes\uwussh`, and the Linux setup the desktop entry's
//! `MimeType` (`apps/setup`).

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;
use uuid::Uuid;

/// The link's scheme, as registered by every installer.
pub(crate) const SCHEME: &str = "uwussh";

/// Told to the page when a link arrived; it then calls [`take_link`].
const EVENT: &str = "link:connect";

/// The host id of the newest link the page hasn't taken yet.
#[derive(Default)]
pub(crate) struct Pending(Mutex<Option<Uuid>>);

/// The host id in `uwussh://connect/<id>`, and nothing else: the id must be a
/// hyphenated UUID, with at most one trailing slash (Windows adds one), and
/// there is no room for a query, a fragment, a user or a port.
pub(crate) fn parse(url: &str) -> Option<Uuid> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let id = rest.strip_prefix("connect/")?;
    let id = id.strip_suffix('/').unwrap_or(id);
    // 36 characters is the hyphenated form only; `try_parse` would also take
    // the simple, braced and urn forms.
    if id.len() != 36 {
        return None;
    }
    Uuid::try_parse(id).ok()
}

/// Wires the deep-link plugin up. Runs in `setup`, after the plugins.
pub(crate) fn setup(app: &tauri::App) {
    app.manage(Pending::default());
    let handle = app.handle().clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            receive(&handle, url.as_str());
        }
    });
    // The link this instance was started with, if it was.
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        for url in urls {
            receive(app.handle(), url.as_str());
        }
    }
}

/// A second start of UwUSSH: this window comes to the front. A link it was
/// started with arrives through the deep-link plugin on its own.
pub(crate) fn second_instance(app: &AppHandle) {
    show(app);
}

fn receive(app: &AppHandle, url: &str) {
    show(app);
    match parse(url) {
        Some(id) => {
            *app.state::<Pending>().0.lock() = Some(id);
            let _ = app.emit(EVENT, ());
        }
        // Not logged: a link is whatever some page put in it.
        None => tracing::warn!("ignored a link that is not uwussh://connect/<host id>"),
    }
}

fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// The host id a link asked for, once: the page connects to it.
#[tauri::command]
pub(crate) fn take_link(pending: tauri::State<'_, Pending>) -> Option<Uuid> {
    pending.0.lock().take()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0b5f5d3e-8a4c-4a7e-9b1f-2c3d4e5f6a7b";

    #[test]
    fn takes_the_host_id() {
        let id = Uuid::parse_str(ID).unwrap();
        assert_eq!(parse(&format!("uwussh://connect/{ID}")), Some(id));
        assert_eq!(parse(&format!("uwussh://connect/{ID}/")), Some(id));
        assert_eq!(parse(&format!("UWUSSH://connect/{ID}")), Some(id));
        assert_eq!(
            parse(&format!("uwussh://connect/{}", ID.to_uppercase())),
            Some(id)
        );
    }

    #[test]
    fn refuses_everything_else() {
        let simple = ID.replace('-', "");
        for url in [
            String::new(),
            "uwussh://".into(),
            "uwussh://connect/".into(),
            "uwussh://connect/not-a-uuid".into(),
            format!("uwussh://connect/{simple}"),
            format!("uwussh://connect/{{{ID}}}"),
            format!("uwussh://connect/urn:uuid:{ID}"),
            format!("uwussh://connect/{ID}//"),
            format!("uwussh://connect/{ID}?host=203.0.113.5"),
            format!("uwussh://connect/{ID}#x"),
            format!("uwussh://connect/{ID}/extra"),
            format!("uwussh://open/{ID}"),
            format!("uwussh://Connect/{ID}"),
            format!("uwussh://user@connect/{ID}"),
            format!("uwussh://connect:22/{ID}"),
            format!("uwurdp://connect/{ID}"),
            format!("https://connect/{ID}"),
            format!("uwussh:connect/{ID}"),
            format!("uwussh://connect/{ID}%00"),
            format!("uwussh://connect/ {ID}"),
        ] {
            assert_eq!(parse(&url), None, "{url}");
        }
    }
}
