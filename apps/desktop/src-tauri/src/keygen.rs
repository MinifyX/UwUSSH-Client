//! UwUKeygen's commands, shared by UwUSSH and the standalone UwUKeygen app
//! (which includes this file by path, together with `dialogs.rs`).
//!
//! A generated key lives in Rust's memory under a token until the user decides
//! what to do with it: show it, save it as a file, put it into the vault (the
//! app's `keys::keygen_store`), or throw it away. Its private half only reaches
//! the page when the user asks to see or copy it — that is what a key
//! generator is for — and otherwise leaves through a save dialog.

use crate::dialogs::{self, Filter};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, State};
use uuid::Uuid;
use uwussh_keygen::{GenerateOptions, KeyInfo, KeyKind, KeyPair, PrivateFormat};
use zeroize::Zeroizing;

/// Keys generated in this run and not yet dealt with. Few, and short-lived.
#[derive(Default)]
pub(crate) struct Generated(Mutex<HashMap<String, Arc<KeyPair>>>);

/// At most this many generated keys wait at once; the oldest goes first.
const MAX_GENERATED: usize = 8;

impl Generated {
    pub(crate) fn with<T>(
        &self,
        token: &str,
        use_key: impl FnOnce(&KeyPair) -> Result<T, KeygenFailure>,
    ) -> Result<T, KeygenFailure> {
        let key = self.get(token)?;
        use_key(&key)
    }

    pub(crate) fn get(&self, token: &str) -> Result<Arc<KeyPair>, KeygenFailure> {
        self.0
            .lock()
            .get(token)
            .cloned()
            .ok_or_else(|| failure("generate the key again"))
    }

    pub(crate) fn forget(&self, token: &str) {
        self.0.lock().remove(token);
    }
}

/// Everything here fails the same way for the page: `{kind: "error", message}`.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum KeygenFailure {
    Error { message: String },
}

pub(crate) fn failure(message: impl std::fmt::Display) -> KeygenFailure {
    KeygenFailure::Error {
        message: message.to_string(),
    }
}

impl From<uwussh_keygen::KeygenError> for KeygenFailure {
    fn from(error: uwussh_keygen::KeygenError) -> Self {
        failure(error)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateRequest {
    kind: KeyKind,
    comment: String,
    /// Bytes the page collected while the user played with Nyu.
    #[serde(default)]
    entropy: Vec<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeneratedInfo {
    token: String,
    info: KeyInfo,
}

#[tauri::command]
pub(crate) async fn keygen_generate(
    generated: State<'_, Generated>,
    request: GenerateRequest,
) -> Result<GeneratedInfo, KeygenFailure> {
    if request.entropy.len() > 64 * 1024 {
        return Err(failure("too much entropy data"));
    }
    let options = GenerateOptions {
        kind: request.kind,
        comment: request.comment,
        extra_entropy: request.entropy,
    };
    // RSA takes a moment; keep it off the IPC threads.
    let key = tauri::async_runtime::spawn_blocking(move || uwussh_keygen::generate(&options))
        .await
        .map_err(failure)??;
    let info = key.info();
    let token = Uuid::now_v7().to_string();
    let mut keys = generated.0.lock();
    if keys.len() >= MAX_GENERATED {
        // Tokens are v7 UUIDs, so the smallest is the oldest.
        if let Some(oldest) = keys.keys().min().cloned() {
            keys.remove(&oldest);
        }
    }
    keys.insert(token.clone(), Arc::new(key));
    Ok(GeneratedInfo { token, info })
}

/// Write a generated key in a format, on a worker thread: with a passphrase,
/// the format's key derivation takes a moment on purpose.
pub(crate) async fn encode(
    generated: &Generated,
    token: &str,
    format: PrivateFormat,
    passphrase: Option<String>,
) -> Result<Zeroizing<String>, KeygenFailure> {
    let key = generated.get(token)?;
    let passphrase = passphrase.filter(|p| !p.is_empty()).map(Zeroizing::new);
    tauri::async_runtime::spawn_blocking(move || {
        Ok(key.encode(format, passphrase.as_ref().map(|p| p.as_str()))?)
    })
    .await
    .map_err(failure)?
}

/// The private key as text, in a format, for showing.
#[tauri::command]
pub(crate) async fn keygen_encode(
    generated: State<'_, Generated>,
    token: String,
    format: PrivateFormat,
    passphrase: Option<String>,
) -> Result<String, KeygenFailure> {
    Ok(encode(&generated, &token, format, passphrase)
        .await?
        .to_string())
}

/// Put the private key on the clipboard, marked so Windows keeps it out of
/// the clipboard history and never syncs it to other devices, and take it off
/// again after a minute if nothing else was copied meanwhile.
#[tauri::command]
pub(crate) async fn keygen_copy_private(
    window: tauri::WebviewWindow,
    generated: State<'_, Generated>,
    token: String,
    format: PrivateFormat,
    passphrase: Option<String>,
) -> Result<(), KeygenFailure> {
    let encoded = encode(&generated, &token, format, passphrase).await?;
    clipboard::copy_private(&window, encoded).await
}

#[tauri::command]
pub(crate) async fn keygen_save(
    app: AppHandle,
    generated: State<'_, Generated>,
    token: String,
    format: PrivateFormat,
    passphrase: Option<String>,
    label: String,
) -> Result<Option<String>, KeygenFailure> {
    let encoded = encode(&generated, &token, format, passphrase).await?;
    save_key_file(&app, &label, format, &encoded).await
}

/// Save the public key line as `<label>.pub`.
#[tauri::command]
pub(crate) async fn keygen_save_public(
    app: AppHandle,
    generated: State<'_, Generated>,
    token: String,
    label: String,
) -> Result<Option<String>, KeygenFailure> {
    let line = generated.with(&token, |key| Ok(key.info().public_openssh))?;
    let name = format!("{}.pub", file_stem(&label));
    let filter = Filter {
        name: "Public Key",
        extensions: &["pub"],
    };
    let Some(path) = dialogs::save(&app, "Public Key speichern", &name, filter).await else {
        return Ok(None);
    };
    std::fs::write(&path, format!("{line}\n")).map_err(failure)?;
    Ok(Some(saved_name(&path, name)))
}

#[tauri::command]
pub(crate) fn keygen_discard(generated: State<'_, Generated>, token: String) {
    generated.forget(&token);
}

/// A label as a file name: letters, digits, `-`, `_` and `.` stay.
pub(crate) fn file_stem(label: &str) -> String {
    let stem: String = label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.trim_matches(['_', '.']).is_empty() {
        "id_uwussh".into()
    } else {
        stem
    }
}

fn saved_name(path: &std::path::Path, fallback: String) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(fallback)
}

/// Save private key text through a save dialog. `None` when cancelled.
pub(crate) async fn save_key_file(
    app: &AppHandle,
    label: &str,
    format: PrivateFormat,
    encoded: &str,
) -> Result<Option<String>, KeygenFailure> {
    let stem = file_stem(label);
    let extension = format.file_extension();
    let name = if extension.is_empty() {
        stem
    } else {
        format!("{stem}.{extension}")
    };
    let filter = Filter {
        name: "Private Key",
        extensions: if extension.is_empty() {
            &[]
        } else {
            std::slice::from_ref(&extension)
        },
    };
    let Some(path) = dialogs::save(app, "Private Key speichern", &name, filter).await else {
        return Ok(None);
    };
    write_private(&path, encoded.as_bytes()).map_err(failure)?;
    Ok(Some(saved_name(&path, name)))
}

/// A private key file, readable by this user only. On Windows the file gets
/// a protected access list — this user, SYSTEM and Administrators, the same
/// Win32-OpenSSH accepts — before anything is written into it, whatever the
/// folder it lands in would otherwise hand down.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::io::Write;
        std::fs::write(path, b"")?;
        private_acl::restrict(path)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)?;
        file.write_all(bytes)
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::write(path, bytes)
    }
}

#[cfg(windows)]
mod private_acl {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(Some(0)).collect()
    }

    /// This user's SID as `S-1-5-21-…`.
    fn user_sid() -> std::io::Result<String> {
        // SAFETY: every out-pointer points at a live local; the token handle
        // is closed and the SID string freed on every path.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut buffer = vec![0u8; 256];
            let mut needed = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut needed,
            );
            CloseHandle(token);
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let user = &*(buffer.as_ptr() as *const TOKEN_USER);
            let mut text = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let length = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text.cast());
            Ok(sid)
        }
    }

    pub(super) fn restrict(path: &std::path::Path) -> std::io::Result<()> {
        let sddl = format!("D:P(A;;FA;;;{})(A;;FA;;;SY)(A;;FA;;;BA)", user_sid()?);
        let sddl = wide(std::ffi::OsStr::new(&sddl));
        let path = wide(path.as_os_str());
        // SAFETY: the strings are NUL-terminated and outlive the calls; the
        // descriptor is freed once the access list has been applied.
        unsafe {
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let mut present = 0;
            let mut defaulted = 0;
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let found =
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted);
            let result = if found == 0 || present == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                match SetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    dacl,
                    std::ptr::null(),
                ) {
                    0 => Ok(()),
                    code => Err(std::io::Error::from_raw_os_error(code as i32)),
                }
            };
            LocalFree(descriptor.cast());
            result
        }
    }
}

mod clipboard {
    use super::{failure, KeygenFailure};
    use zeroize::Zeroizing;

    #[cfg(windows)]
    pub(super) async fn copy_private(
        window: &tauri::WebviewWindow,
        text: Zeroizing<String>,
    ) -> Result<(), KeygenFailure> {
        let hwnd = window.hwnd().map_err(failure)?.0 as isize;
        let (done, result) = std::sync::mpsc::channel();
        window
            .run_on_main_thread(move || {
                let _ = done.send(set(hwnd, &text));
            })
            .map_err(failure)?;
        let sequence = tauri::async_runtime::spawn_blocking(move || result.recv())
            .await
            .map_err(failure)?
            .map_err(failure)?
            .map_err(failure)?;
        // A minute later, unless something else was copied since.
        let window = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(60));
            let _ = window.run_on_main_thread(move || clear_if_unchanged(hwnd, sequence));
        });
        Ok(())
    }

    #[cfg(not(windows))]
    pub(super) async fn copy_private(
        _window: &tauri::WebviewWindow,
        _text: Zeroizing<String>,
    ) -> Result<(), KeygenFailure> {
        Err(failure("copying a private key needs Windows for now"))
    }

    #[cfg(windows)]
    fn format(name: &str) -> u32 {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated UTF-16 string.
        unsafe { windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(wide.as_ptr()) }
    }

    /// Copy `bytes` into movable global memory, as the clipboard wants it.
    #[cfg(windows)]
    unsafe fn global(bytes: &[u8]) -> Option<windows_sys::Win32::Foundation::HGLOBAL> {
        use windows_sys::Win32::System::Memory::{
            GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
        };
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if memory.is_null() {
            return None;
        }
        let target = GlobalLock(memory);
        if target.is_null() {
            windows_sys::Win32::Foundation::GlobalFree(memory);
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target.cast(), bytes.len());
        GlobalUnlock(memory);
        Some(memory)
    }

    /// Returns the clipboard's sequence number after the copy.
    #[cfg(windows)]
    fn set(hwnd: isize, text: &str) -> Result<u32, String> {
        use windows_sys::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard,
            SetClipboardData,
        };
        const CF_UNICODETEXT: u32 = 13;
        let mut utf16: Zeroizing<Vec<u16>> =
            Zeroizing::new(text.encode_utf16().chain(Some(0)).collect());
        // SAFETY: the clipboard is opened for this window and closed on every
        // path; memory handed to SetClipboardData belongs to the system after
        // a successful call and is freed by us otherwise.
        unsafe {
            if OpenClipboard(hwnd as _) == 0 {
                return Err("the clipboard is busy".into());
            }
            let result = (|| {
                if EmptyClipboard() == 0 {
                    return Err("the clipboard could not be emptied".to_string());
                }
                let bytes =
                    std::slice::from_raw_parts(utf16.as_ptr().cast::<u8>(), utf16.len() * 2);
                let text_memory = global(bytes).ok_or("out of memory")?;
                if SetClipboardData(CF_UNICODETEXT, text_memory).is_null() {
                    windows_sys::Win32::Foundation::GlobalFree(text_memory);
                    return Err("the clipboard refused the text".into());
                }
                // Keep it out of Win+V history, cloud sync and clipboard monitors.
                let zero = 0u32.to_ne_bytes();
                for (name, data) in [
                    ("ExcludeClipboardContentFromMonitorProcessing", &zero[..]),
                    ("CanIncludeInClipboardHistory", &zero[..]),
                    ("CanUploadToCloudClipboard", &zero[..]),
                ] {
                    let id = format(name);
                    if id == 0 {
                        continue;
                    }
                    if let Some(memory) = global(data) {
                        if SetClipboardData(id, memory).is_null() {
                            windows_sys::Win32::Foundation::GlobalFree(memory);
                        }
                    }
                }
                Ok(())
            })();
            CloseClipboard();
            utf16.iter_mut().for_each(|unit| *unit = 0);
            result.map(|()| GetClipboardSequenceNumber())
        }
    }

    #[cfg(windows)]
    fn clear_if_unchanged(hwnd: isize, sequence: u32) {
        use windows_sys::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard,
        };
        // SAFETY: opened and closed right here.
        unsafe {
            if GetClipboardSequenceNumber() != sequence || OpenClipboard(hwnd as _) == 0 {
                return;
            }
            EmptyClipboard();
            CloseClipboard();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_become_safe_file_names() {
        assert_eq!(file_stem("lorin@prox-1"), "lorin_prox-1");
        assert_eq!(file_stem("../../evil"), ".._.._evil");
        assert_eq!(file_stem("///"), "id_uwussh");
        assert_eq!(file_stem("id_ed25519"), "id_ed25519");
    }
}
