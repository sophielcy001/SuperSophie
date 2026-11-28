// SPDX-License-Identifier: AGPL-3.0-or-later

//! Device tokens live in the system credential store: Windows Credential
//! Manager or the macOS login keychain. Only non-secret settings are written
//! to the data directory.

use zhiye_core::Error;

const SERVICE: &str = "Zhiye Sync";

fn store_error(detail: impl std::fmt::Display) -> Error {
    Error::new(
        "SYNC_CREDENTIAL",
        format!("无法读写系统凭据中的同步令牌。{detail}"),
    )
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{SERVICE, store_error};
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };
    use zhiye_core::Result;

    // errSecItemNotFound
    const NOT_FOUND: i32 = -25300;

    pub fn save(account: &str, secret: &str) -> Result<()> {
        set_generic_password(SERVICE, account, secret.as_bytes()).map_err(store_error)
    }

    pub fn load(account: &str) -> Result<Option<String>> {
        match get_generic_password(SERVICE, account) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| store_error("令牌格式无效。")),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(store_error(error)),
        }
    }

    pub fn delete(account: &str) -> Result<()> {
        match delete_generic_password(SERVICE, account) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(error) => Err(store_error(error)),
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::{SERVICE, store_error};
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };
    use zhiye_core::Result;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn target(account: &str) -> Vec<u16> {
        wide(&format!("{SERVICE}/{account}"))
    }

    fn last_error() -> u32 {
        unsafe { GetLastError() }
    }

    pub fn save(account: &str, secret: &str) -> Result<()> {
        let mut target = target(account);
        let mut user = wide(account);
        let mut blob = secret.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: null_mut(),
            LastWritten: unsafe { std::mem::zeroed() },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: null_mut(),
            TargetAlias: null_mut(),
            UserName: user.as_mut_ptr(),
        };
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(store_error(format!("Win32 错误 {}。", last_error())));
        }
        Ok(())
    }

    pub fn load(account: &str) -> Result<Option<String>> {
        let target = target(account);
        let mut credential: *mut CREDENTIALW = null_mut();
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
            let error = last_error();
            if error == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(store_error(format!("Win32 错误 {error}。")));
        }
        let bytes = unsafe {
            let entry = &*credential;
            std::slice::from_raw_parts(entry.CredentialBlob, entry.CredentialBlobSize as usize)
                .to_vec()
        };
        unsafe { CredFree(credential as *const _) };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| store_error("令牌格式无效。"))
    }

    pub fn delete(account: &str) -> Result<()> {
        let target = target(account);
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let error = last_error();
            if error != ERROR_NOT_FOUND {
                return Err(store_error(format!("Win32 错误 {error}。")));
            }
        }
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::store_error;
    use zhiye_core::Result;

    pub fn save(_: &str, _: &str) -> Result<()> {
        Err(store_error("本平台暂不支持系统凭据。"))
    }

    pub fn load(_: &str) -> Result<Option<String>> {
        Err(store_error("本平台暂不支持系统凭据。"))
    }

    pub fn delete(_: &str) -> Result<()> {
        Ok(())
    }
}

pub use platform::{delete, load, save};

#[cfg(test)]
mod tests {
    /// Writes to the real login keychain or Credential Manager; run on demand:
    /// `cargo test -p zhiye-desktop credentials -- --ignored`
    #[test]
    #[ignore]
    fn round_trips_through_the_system_store() {
        let account = format!("test-{}", uuid::Uuid::new_v4());
        assert_eq!(super::load(&account).unwrap(), None);
        super::save(&account, "zsd_first").unwrap();
        super::save(&account, "zsd_second").unwrap();
        assert_eq!(
            super::load(&account).unwrap().as_deref(),
            Some("zsd_second")
        );
        super::delete(&account).unwrap();
        assert_eq!(super::load(&account).unwrap(), None);
        super::delete(&account).unwrap();
    }
}
