//! The macOS menu bar.
//!
//! Without a menu, a macOS app has no Edit menu, and without one the webview
//! has nothing that turns ⌘C, ⌘V and ⌘A into copy, paste and select all — not
//! in the terminal, not in any text field. Tauri's own default menu has those,
//! but it also has "Close Window" on ⌘W, which the menu would take before the
//! page could make it "close tab", and a Quit that ends the app without
//! asking about open connections.
//!
//! So this is that menu without the parts that fight the app's shortcuts
//! (`lib/keymap.ts`): no ⌘W, no ⌘T, no ⌘1 … ⌘9 in it, and Quit closes the window
//! the normal way, so the "connections are still open" question comes up.
//!
//! Windows and Linux have no menu bar; the window there has none.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Runtime};

const QUIT: &str = "uwussh-quit";

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let name = app.package_info().name.clone();
    let quit = MenuItem::with_id(app, QUIT, format!("Quit {name}"), true, Some("Cmd+Q"))?;
    let app_menu = Submenu::with_items(
        app,
        &name,
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&app_menu, &edit, &view, &window])
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn on_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    if event.id() != QUIT {
        return;
    }
    // `close()` asks first, like the red button does: the page may hold the
    // window open to ask about running sessions. Closing the last window ends
    // the app.
    match app.get_webview_window("main") {
        Some(window) => {
            if let Err(error) = window.close() {
                tracing::warn!(%error, "could not close the window to quit");
            }
        }
        None => app.exit(0),
    }
}
