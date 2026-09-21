//! Packs UwUSSH, and UwUKeygen next to it, into the setup executable.
//!
//! On Windows `UWUSSH_SETUP_PAYLOAD` points at the built `uwussh-desktop.exe`
//! and `UWUSSH_SETUP_KEYGEN_PAYLOAD` at `uwukeygen.exe`, each compressed on
//! its own.
//!
//! On macOS and Linux the apps are folders — `UwUSSH.app` and `UwUKeygen.app`,
//! or the AppDirs `UwUSSH` and `UwUKeygen` — and `UWUSSH_SETUP_PAYLOAD` points
//! at a folder holding them. It goes into one tar archive, compressed with a
//! long window: two AppDirs carry the same WebKit libraries, and a window that
//! reaches back past the first one makes the second copy nearly free.
//!
//! `pnpm build:setup` sets the variables. Without them the setup still builds,
//! but can't install anything, which is enough for checks and working on its UI.

use std::path::{Path, PathBuf};

/// How far back the compressor looks: 256 MiB, more than an AppDir. The setup
/// allows the same when it unpacks (`WINDOW_LOG` in `install_unix.rs`).
const WINDOW_LOG: u32 = 28;

fn out(name: &str) -> PathBuf {
    PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set")).join(name)
}

fn compress(raw: &[u8], long: bool) -> Vec<u8> {
    let mut encoder = zstd::Encoder::new(Vec::new(), 19).expect("a compressor");
    if long {
        encoder
            .long_distance_matching(true)
            .expect("long distance matching");
        encoder.window_log(WINDOW_LOG).expect("a long window");
    }
    std::io::Write::write_all(&mut encoder, raw).expect("compressing the app");
    encoder.finish().expect("finishing the compression")
}

/// Compress the file an environment variable names into `OUT_DIR/<name>`,
/// and tell the code its size through `<size_env>`. Empty when unset.
fn pack_file(variable: &str, name: &str, size_env: &str) {
    println!("cargo:rerun-if-env-changed={variable}");
    match std::env::var_os(variable).filter(|path| !path.is_empty()) {
        Some(path) => {
            let path = Path::new(&path);
            println!("cargo:rerun-if-changed={}", path.display());
            let raw = std::fs::read(path)
                .unwrap_or_else(|e| panic!("can't read {}: {e}", path.display()));
            std::fs::write(out(name), compress(&raw, false)).expect("writing the payload");
            println!("cargo:rustc-env={size_env}={}", raw.len());
        }
        None => {
            std::fs::write(out(name), []).expect("writing the empty payload");
            println!("cargo:rustc-env={size_env}=0");
        }
    }
}

/// Everything in `dir` as one tar archive, top-level entries by their own
/// names, symlinks kept as symlinks and modes as they are — an `.app` and an
/// AppDir both need their executable bits and links.
fn archive(dir: &Path) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    builder.follow_symlinks(false);
    builder.mode(tar::HeaderMode::Deterministic);
    let mut children: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("can't read {}: {e}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        let name = child.file_name().expect("a name");
        let meta = std::fs::symlink_metadata(&child).expect("metadata");
        if meta.is_dir() {
            builder
                .append_dir_all(name, &child)
                .unwrap_or_else(|e| panic!("can't pack {}: {e}", child.display()));
        } else {
            builder
                .append_path_with_name(&child, name)
                .unwrap_or_else(|e| panic!("can't pack {}: {e}", child.display()));
        }
    }
    builder.into_inner().expect("finishing the archive")
}

/// The apps folder for macOS and Linux, UwUKeygen inside it or not.
fn pack_folder(variable: &str) {
    println!("cargo:rerun-if-env-changed={variable}");
    let (payload, size, keygen) = match std::env::var_os(variable).filter(|path| !path.is_empty()) {
        Some(path) => {
            let dir = Path::new(&path);
            println!("cargo:rerun-if-changed={}", dir.display());
            let raw = archive(dir);
            let keygen = ["UwUKeygen.app", "UwUKeygen"]
                .iter()
                .any(|name| dir.join(name).is_dir());
            (compress(&raw, true), raw.len(), keygen)
        }
        None => (Vec::new(), 0, false),
    };
    std::fs::write(out("payload.zst"), payload).expect("writing the payload");
    std::fs::write(out("keygen.zst"), []).expect("writing the empty payload");
    println!("cargo:rustc-env=UWUSSH_SETUP_PAYLOAD_SIZE={size}");
    println!("cargo:rustc-env=UWUSSH_SETUP_KEYGEN_SIZE=0");
    println!(
        "cargo:rustc-env=UWUSSH_SETUP_KEYGEN_INSIDE={}",
        if keygen { "1" } else { "0" }
    );
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        println!("cargo:rerun-if-env-changed=UWUSSH_SETUP_KEYGEN_PAYLOAD");
        pack_folder("UWUSSH_SETUP_PAYLOAD");
    } else {
        pack_file(
            "UWUSSH_SETUP_PAYLOAD",
            "payload.zst",
            "UWUSSH_SETUP_PAYLOAD_SIZE",
        );
        pack_file(
            "UWUSSH_SETUP_KEYGEN_PAYLOAD",
            "keygen.zst",
            "UWUSSH_SETUP_KEYGEN_SIZE",
        );
        println!("cargo:rustc-env=UWUSSH_SETUP_KEYGEN_INSIDE=0");
    }
    // The setup usually runs from Downloads, next to whatever else was
    // downloaded: linked DLLs come from System32 only, never from the setup's
    // own folder.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
    tauri_build::build()
}
