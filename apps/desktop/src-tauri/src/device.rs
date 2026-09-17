//! Sealing the vault key for this Windows user, so the vault opens on start.
//!
//! DPAPI ties the blob to the signed-in account: another account on this
//! machine can't open it and needs the master password. Anything running as
//! this user can, and so can anyone with the account's password and a copy of
//! the disk — a remembered vault is as strong as the Windows account, which is
//! the trust UwUSSH already places in the user. See `uwussh_store::device`.

use zeroize::Zeroizing;

/// Mixed into DPAPI, so a blob made by UwUSSH is only useful to UwUSSH's call.
#[cfg(windows)]
const ENTROPY: &[u8] = b"uwussh/vault-key/device/v1";

#[cfg(windows)]
pub(crate) fn protect(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = blob(bytes);
    let entropy = blob(ENTROPY);
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: the input blobs point at live slices for the duration of the
    // call; the output is allocated by DPAPI and freed below.
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(take(output).to_vec())
}

#[cfg(windows)]
pub(crate) fn unprotect(bytes: &[u8]) -> std::io::Result<Zeroizing<Vec<u8>>> {
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = blob(bytes);
    let entropy = blob(ENTROPY);
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: as in `protect`.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(take(output))
}

#[cfg(windows)]
fn blob(bytes: &[u8]) -> windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB {
    windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    }
}

/// Copy DPAPI's output into wiping memory, wipe DPAPI's copy and free it.
#[cfg(windows)]
fn take(
    output: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB,
) -> Zeroizing<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    if output.pbData.is_null() {
        return Zeroizing::new(Vec::new());
    }
    // SAFETY: DPAPI returned `cbData` initialised bytes at `pbData`, which stay
    // valid until LocalFree.
    let copy = unsafe {
        let slice = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let copy = Zeroizing::new(slice.to_vec());
        zeroize::Zeroize::zeroize(slice);
        LocalFree(output.pbData.cast());
        copy
    };
    copy
}

#[cfg(not(windows))]
pub(crate) fn protect(_bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::other(
        "keeping the vault key on this device needs Windows for now",
    ))
}

#[cfg(not(windows))]
pub(crate) fn unprotect(_bytes: &[u8]) -> std::io::Result<Zeroizing<Vec<u8>>> {
    Err(std::io::Error::other(
        "keeping the vault key on this device needs Windows for now",
    ))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn what_this_user_protects_this_user_can_open() {
        let key = [42u8; 32];
        let sealed = protect(&key).unwrap();
        assert_ne!(sealed.as_slice(), key.as_slice());
        assert_eq!(unprotect(&sealed).unwrap().as_slice(), key.as_slice());
    }

    #[test]
    fn a_tampered_blob_does_not_open() {
        let mut sealed = protect(&[1u8; 32]).unwrap();
        let middle = sealed.len() / 2;
        sealed[middle] ^= 0xff;
        assert!(unprotect(&sealed).is_err());
    }
}
