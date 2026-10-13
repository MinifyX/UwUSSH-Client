//! The user's own keyboard shortcuts, the texts a key can type, and what the
//! shortcut settings need to know about this machine: its keyboard layout and
//! its monospace fonts.
//!
//! The keymap lives in the store (`uwussh_store::keymap`) and travels with
//! the sync. What a text types never goes back to the page: it is written
//! once, and typing it into a session happens here.

use crate::hosts::SaveFailure;
use crate::AppState;
use std::collections::BTreeSet;
use std::process::Command;
use tauri::State;
use uuid::Uuid;
use uwussh_core::SessionId;
use uwussh_store::{KeymapDraft, KeymapView};

/// As many font names as the settings will list.
const MAX_FONTS: usize = 400;

#[tauri::command]
pub(crate) fn keymap_get(
    state: State<'_, AppState>,
    family: String,
) -> Result<KeymapView, SaveFailure> {
    Ok(state.store().keymap(&family)?)
}

#[tauri::command]
pub(crate) fn keymap_save(
    state: State<'_, AppState>,
    sync: State<'_, crate::sync::Syncs>,
    family: String,
    draft: KeymapDraft,
) -> Result<KeymapView, SaveFailure> {
    let saved = state.store().save_keymap(&family, draft)?;
    sync.poke();
    Ok(saved)
}

/// What a text sends to the terminal: line breaks become the Enter key, tabs
/// stay, and every other control character is dropped — a text types, it does
/// not press Ctrl+C or send escape sequences.
fn typed(text: &str, enter: bool) -> Vec<u8> {
    let mut out = String::with_capacity(text.len() + 1);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\r');
            }
            '\n' => out.push('\r'),
            '\t' => out.push('\t'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    if enter && !out.ends_with('\r') {
        out.push('\r');
    }
    out.into_bytes()
}

/// Type one of the user's texts into a session. Nothing happens when the text
/// is gone or has not arrived from the device that wrote it.
#[tauri::command]
pub(crate) fn keymap_send_text(
    state: State<'_, AppState>,
    id: SessionId,
    family: String,
    text_id: Uuid,
) -> Result<(), SaveFailure> {
    let Some((text, enter)) = state.store().keymap_text(&family, text_id)? else {
        return Ok(());
    };
    let mut bytes = typed(&text, enter);
    let written = state.sessions.write(id, &bytes);
    zeroize::Zeroize::zeroize(&mut bytes);
    written.map_err(|error| SaveFailure::Error {
        message: error.to_string(),
    })
}

/// The monospace fonts installed on this machine, by family name.
#[tauri::command]
pub(crate) async fn terminal_fonts() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let names: BTreeSet<String> = db
            .faces()
            .filter(|face| face.monospaced)
            .filter_map(|face| {
                face.families
                    .first()
                    .map(|(name, _)| name.trim().to_owned())
            })
            // A leading dot marks a system-internal font on macOS.
            .filter(|name| !name.is_empty() && !name.starts_with('.') && name.len() <= 80)
            .collect();
        let mut names: Vec<String> = names.into_iter().take(MAX_FONTS).collect();
        names.sort_by_key(|name| name.to_lowercase());
        names
    })
    .await
    .unwrap_or_default()
}

/// The keyboard layout in use, in xkb's words (`de`, `ch-fr`, `us-dvorak`),
/// or nothing when the system does not say. On Windows the webview knows it
/// itself (`navigator.keyboard`), so nothing is asked here.
#[tauri::command]
pub(crate) async fn keyboard_layout() -> Option<String> {
    tauri::async_runtime::spawn_blocking(system_layout)
        .await
        .ok()
        .flatten()
}

/// What a helper program prints, when it ends well, soon and briefly.
fn output_of(program: &str, args: &[&str]) -> Option<String> {
    output_within(program, args, std::time::Duration::from_secs(2))
}

/// Run a program and hand back what it printed. Nothing when it cannot be
/// started, fails, is not done in `timeout` or prints more than a layout's
/// name could ever take: it is killed then, and reaped either way.
#[cfg_attr(windows, allow(dead_code))]
fn output_within(program: &str, args: &[&str], timeout: std::time::Duration) -> Option<String> {
    use std::io::Read;
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// As much output as is read.
    const LIMIT: usize = 64 * 1024;

    let deadline = Instant::now() + timeout;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Read on a thread of its own, so that a program that says nothing and
    // never ends costs the timeout and no more. One byte past the limit is
    // asked for, which tells too much from just enough.
    let output = child
        .stdout
        .take()
        .and_then(|stdout| {
            let (sender, receiver) = mpsc::channel();
            std::thread::Builder::new()
                .name("keymap-helper".into())
                .spawn(move || {
                    let mut out = Vec::new();
                    let read = stdout.take(LIMIT as u64 + 1).read_to_end(&mut out);
                    let _ = sender.send(read.ok().map(|_| out));
                })
                .ok()?;
            receiver.recv_timeout(timeout).ok().flatten()
        })
        .filter(|out| out.len() <= LIMIT);

    // Its output ended, so it is ending: the end of the pipe and the exit
    // are a moment apart at most. One that closed its output and stays is
    // given until the deadline, not longer.
    let status = output.as_ref().and_then(|_| loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => break None,
        }
    });
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }

    match (output, status) {
        (Some(out), Some(status)) if status.success() => {
            Some(String::from_utf8_lossy(&out).into_owned())
        }
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn system_layout() -> Option<String> {
    let sources = output_of(
        "/usr/bin/defaults",
        &["read", "com.apple.HIToolbox", "AppleSelectedInputSources"],
    )?;
    mac_layout(&sources).map(str::to_owned)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_layout() -> Option<String> {
    for key in ["mru-sources", "sources"] {
        let found = output_of(
            "gsettings",
            &["get", "org.gnome.desktop.input-sources", key],
        )
        .and_then(|sources| gnome_layout(&sources));
        if found.is_some() {
            return found;
        }
    }
    if let Some(found) = output_of("setxkbmap", &["-query"]).and_then(|text| xkb_layout(&text, ':'))
    {
        return Some(found);
    }
    ["/etc/default/keyboard", "/etc/vconsole.conf"]
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .find_map(|text| xkb_layout(&text, '='))
}

#[cfg(windows)]
fn system_layout() -> Option<String> {
    None
}

/// `de` and `nodeadkeys` as one name; only letters, digits and dashes get out.
fn xkb_name(layout: &str, variant: &str) -> Option<String> {
    let clean = |part: &str| -> String {
        part.trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(24)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let layout = clean(layout);
    let variant = clean(variant);
    match (layout.is_empty(), variant.is_empty()) {
        (true, _) => None,
        (false, true) => Some(layout),
        (false, false) => Some(format!("{layout}-{variant}")),
    }
}

/// The first xkb source in GNOME's list: `[('xkb', 'de+nodeadkeys'), …]`.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn gnome_layout(sources: &str) -> Option<String> {
    let rest = &sources[sources.find("'xkb'")? + 5..];
    let start = rest.find('\'')? + 1;
    let name = &rest[start..start + rest[start..].find('\'')?];
    let (layout, variant) = name.split_once('+').unwrap_or((name, ""));
    xkb_name(layout, variant)
}

/// The first layout and its variant out of `setxkbmap -query` (`layout: de,us`)
/// or a config file (`XKBLAYOUT="de,us"`).
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn xkb_layout(text: &str, separator: char) -> Option<String> {
    let value = |names: &[&str]| -> Option<&str> {
        text.lines().find_map(|line| {
            let (key, value) = line.split_once(separator)?;
            names
                .contains(&key.trim().to_ascii_lowercase().as_str())
                .then(|| value.trim().trim_matches('"'))
        })
    };
    let first = |list: &str| list.split(',').next().unwrap_or("").to_owned();
    let layout = first(value(&["layout", "xkblayout"])?);
    let variant = value(&["variant", "xkbvariant"])
        .map(first)
        .unwrap_or_default();
    xkb_name(&layout, &variant)
}

/// macOS names its layouts in English (`"KeyboardLayout Name" = German;`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_layout(sources: &str) -> Option<&'static str> {
    const NAMES: &[(&str, &str)] = &[
        ("swiss german", "ch"),
        ("swiss french", "ch-fr"),
        ("german", "de"),
        ("austrian", "de"),
        ("british", "gb"),
        ("irish", "gb"),
        ("canadian", "us"),
        ("dvorak", "us-dvorak"),
        ("colemak", "us-colemak"),
        ("azerty", "fr"),
        ("french", "fr"),
        ("belgian", "be"),
        ("latin american", "latam"),
        ("spanish", "es"),
        ("italian", "it"),
        ("brazilian", "br"),
        ("portuguese", "pt"),
        ("dutch", "nl"),
        ("swedish", "se"),
        ("finnish", "fi"),
        ("norwegian", "no"),
        ("danish", "dk"),
        ("icelandic", "is"),
        ("estonian", "ee"),
        ("polish", "pl"),
        ("czech", "cz"),
        ("slovak", "sk"),
        ("hungarian", "hu"),
        ("croatian", "hr"),
        ("slovenian", "si"),
        ("romanian", "ro"),
        ("turkish", "tr"),
        ("greek", "gr"),
        ("ukrainian", "ua"),
        ("russian", "ru"),
        ("hebrew", "il"),
        ("u.s.", "us"),
        ("us", "us"),
        ("abc", "us"),
        ("australian", "us"),
    ];
    let name = sources.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        key.contains("KeyboardLayout Name").then(|| {
            value
                .trim()
                .trim_end_matches(';')
                .trim_matches('"')
                .to_lowercase()
        })
    })?;
    NAMES
        .iter()
        .find(|(known, _)| name.starts_with(known) || name.contains(known))
        .map(|(_, id)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_types_and_never_presses_control_keys() {
        assert_eq!(typed("ls -la", false), b"ls -la");
        assert_eq!(typed("ls -la", true), b"ls -la\r");
        assert_eq!(typed("a\r\nb\nc\n", true), b"a\rb\rc\r");
        assert_eq!(typed("a\tb\x03\x1b[2J", false), b"a\tb[2J");
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_program_is_read_when_it_ends_well() {
        let long = std::time::Duration::from_secs(30);
        assert_eq!(
            output_within("sh", &["-c", "printf 'layout: de'"], long).as_deref(),
            Some("layout: de")
        );
        assert_eq!(output_of("sh", &["-c", "printf de"]).as_deref(), Some("de"));
        assert_eq!(output_within("sh", &["-c", "echo de; exit 3"], long), None);
        assert_eq!(output_within("uwussh-no-such-program", &[], long), None);
        // Its input is closed and what it says on the side goes nowhere.
        assert_eq!(
            output_within("sh", &["-c", "cat; echo oops >&2"], long).as_deref(),
            Some("")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_program_that_hangs_or_floods_is_cut_off() {
        let started = std::time::Instant::now();
        let short = std::time::Duration::from_millis(100);
        // Silent and never done.
        assert_eq!(output_within("sh", &["-c", "exec sleep 600"], short), None);
        // Done talking, but not done.
        assert_eq!(
            output_within("sh", &["-c", "echo de; exec >&-; exec sleep 600"], short),
            None
        );
        // More than is ever read.
        let long = std::time::Duration::from_secs(30);
        assert_eq!(
            output_within("sh", &["-c", "exec head -c 70000 /dev/zero"], long),
            None
        );
        assert_eq!(
            output_within("sh", &["-c", "exec head -c 65536 /dev/zero"], long).map(|out| out.len()),
            Some(65536)
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "nothing waited for a program that would not end"
        );
    }

    #[test]
    fn gnome_names_its_first_source() {
        assert_eq!(
            gnome_layout("[('xkb', 'de+nodeadkeys'), ('xkb', 'us')]").as_deref(),
            Some("de-nodeadkeys")
        );
        assert_eq!(
            gnome_layout("[('ibus', 'anthy'), ('xkb', 'jp')]").as_deref(),
            Some("jp")
        );
        assert_eq!(gnome_layout("@a(ss) []"), None);
    }

    #[test]
    fn xkb_settings_are_read_from_a_query_and_from_a_file() {
        let query = "rules:      evdev\nmodel:      pc105\nlayout:     ch,us\nvariant:    fr,\n";
        assert_eq!(xkb_layout(query, ':').as_deref(), Some("ch-fr"));
        let file = "XKBMODEL=\"pc105\"\nXKBLAYOUT=\"de\"\nXKBVARIANT=\"\"\n";
        assert_eq!(xkb_layout(file, '=').as_deref(), Some("de"));
        assert_eq!(xkb_layout("KEYMAP=de-latin1\n", '='), None);
        assert_eq!(
            xkb_layout("layout: $(reboot)\n", ':').as_deref(),
            Some("reboot")
        );
    }

    #[test]
    fn mac_layouts_are_known_by_name() {
        let sources = "(\n    {\n        InputSourceKind = \"Keyboard Layout\";\n        \"KeyboardLayout ID\" = 3;\n        \"KeyboardLayout Name\" = German;\n    }\n)\n";
        assert_eq!(mac_layout(sources), Some("de"));
        let swiss = "\"KeyboardLayout Name\" = \"Swiss German\";";
        assert_eq!(mac_layout(swiss), Some("ch"));
        assert_eq!(
            mac_layout("\"KeyboardLayout Name\" = \"U.S.\";"),
            Some("us")
        );
        assert_eq!(mac_layout("\"KeyboardLayout Name\" = Klingon;"), None);
        assert_eq!(mac_layout("()"), None);
    }
}
