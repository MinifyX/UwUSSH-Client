// No console window next to the setup in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg_attr(windows, path = "install.rs")]
#[cfg_attr(unix, path = "install_unix.rs")]
mod install;
#[cfg_attr(windows, path = "system.rs")]
#[cfg_attr(unix, path = "system_unix.rs")]
mod system;
mod versions;

fn main() {
    // On Windows, DLLs loaded later at runtime come from System32 only; this
    // must happen before anything else loads one. Nothing to do elsewhere.
    system::restrict_dll_search();
    app::run();
}
