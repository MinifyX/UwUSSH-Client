//! The user's home folder, for `~` in a key file path.
//!
//! Usually `HOME` (`USERPROFILE` on Windows). Inside the macOS App Sandbox —
//! the Mac App Store build — `HOME` is the app's container,
//! `~/Library/Containers/<bundle id>/Data`, so `~/.ssh/id_ed25519` from an
//! ssh_config import or another device's host would point into the container,
//! where no key ever is. There `~` means the real home again: reading a file
//! in it still needs the sandbox's permission (a folder the person picked),
//! but the path is the one they meant.

use std::path::{Path, PathBuf};

pub fn home_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os(if cfg!(windows) {
        "USERPROFILE"
    } else {
        "HOME"
    })?);
    if cfg!(target_os = "macos") {
        if let Some(real) = outside_container(&home) {
            return Some(real);
        }
    }
    Some(home)
}

/// `/Users/nyu` for `/Users/nyu/Library/Containers/<id>/Data`, the only shape
/// a sandboxed app's home has; `None` for every other path.
pub fn outside_container(home: &Path) -> Option<PathBuf> {
    let names: Vec<_> = home.components().rev().take(4).collect();
    let [data, _bundle_id, containers, library] = names.as_slice() else {
        return None;
    };
    let is = |part: &std::path::Component, name: &str| part.as_os_str() == name;
    if !(is(data, "Data") && is(containers, "Containers") && is(library, "Library")) {
        return None;
    }
    home.ancestors()
        .nth(4)
        .filter(|real| real.parent().is_some())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_container_leads_back_to_the_real_home() {
        assert_eq!(
            outside_container(Path::new(
                "/Users/nyu/Library/Containers/app.uwussh.desktop/Data"
            )),
            Some(PathBuf::from("/Users/nyu"))
        );
        // Trailing slashes are not a different folder.
        assert_eq!(
            outside_container(Path::new(
                "/Users/nyu/Library/Containers/app.uwussh.desktop/Data/"
            )),
            Some(PathBuf::from("/Users/nyu"))
        );
    }

    #[test]
    fn an_ordinary_home_stays_what_it_is() {
        for home in [
            "/Users/nyu",
            "/home/nyu",
            "/Users/nyu/Library/Containers/app.uwussh.desktop",
            "/Users/nyu/Library/Group Containers/x/Data",
            "/Library/Containers/x/Data",
            "Data",
        ] {
            assert_eq!(outside_container(Path::new(home)), None, "{home}");
        }
    }
}
