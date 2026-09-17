//! Packs UwUSSH, and UwUKeygen next to it, into the setup executable.
//!
//! `UWUSSH_SETUP_PAYLOAD` points at the built `uwussh-desktop.exe` and
//! `UWUSSH_SETUP_KEYGEN_PAYLOAD` at `uwukeygen.exe` (`pnpm build:setup` sets
//! both). Without them the setup still builds, but can't install anything,
//! which is enough for checks and working on its UI.

use std::path::{Path, PathBuf};

/// Compress the file an environment variable names into `OUT_DIR/<name>`, and
/// tell the code its size through `<size_env>`. Empty when the variable is unset.
fn pack(variable: &str, name: &str, size_env: &str) {
    println!("cargo:rerun-if-env-changed={variable}");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set")).join(name);
    match std::env::var_os(variable).filter(|path| !path.is_empty()) {
        Some(path) => {
            let path = Path::new(&path);
            println!("cargo:rerun-if-changed={}", path.display());
            let app = std::fs::read(path)
                .unwrap_or_else(|e| panic!("can't read {}: {e}", path.display()));
            let packed = zstd::encode_all(app.as_slice(), 19).expect("compressing the app");
            std::fs::write(&out, packed).expect("writing the payload");
            println!("cargo:rustc-env={size_env}={}", app.len());
        }
        None => {
            std::fs::write(&out, []).expect("writing the empty payload");
            println!("cargo:rustc-env={size_env}=0");
        }
    }
}

fn main() {
    pack(
        "UWUSSH_SETUP_PAYLOAD",
        "payload.zst",
        "UWUSSH_SETUP_PAYLOAD_SIZE",
    );
    pack(
        "UWUSSH_SETUP_KEYGEN_PAYLOAD",
        "keygen.zst",
        "UWUSSH_SETUP_KEYGEN_SIZE",
    );
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
