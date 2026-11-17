//! The passwords of computers and phones, kept only in the operating
//! system's password store (on Android: sealed with a key in the Android
//! Keystore), one entry per device, and only when the person asked for it
//! to be remembered. They never enter `remote.toml`, a log line or an
//! error message.

/// Can this build keep a password at all?
pub const SUPPORTED: bool = cfg!(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "android"
));

const UNAVAILABLE: &str = "Could not reach your system's password store. Unlock it and try again.";

/// The store's name for these entries, as the owner named it.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const SERVICE: &str = "app.kagaz.computers";

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn entry(device: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, device).map_err(|_| UNAVAILABLE.to_string())
}

#[cfg(target_os = "android")]
fn name(device: &str) -> String {
    format!("computers-{device}")
}

/// The password remembered for a device (by Kagaz's own id for it).
pub fn get(device: &str) -> Result<Option<String>, String> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        match entry(device)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(UNAVAILABLE.into()),
        }
    }
    #[cfg(target_os = "android")]
    {
        super::mobile::secret_get(&name(device)).map_err(|_| UNAVAILABLE.to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "android")))]
    {
        let _ = device;
        Ok(None)
    }
}

pub fn set(device: &str, password: &str) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        entry(device)?.set_password(password).map_err(|_| UNAVAILABLE.to_string())
    }
    #[cfg(target_os = "android")]
    {
        super::mobile::secret_set(&name(device), password).map_err(|_| UNAVAILABLE.to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "android")))]
    {
        let _ = (device, password);
        Err("Remembering passwords is not available on this system yet.".into())
    }
}

/// Stop remembering; fine when nothing was remembered.
pub fn forget(device: &str) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        match entry(device)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(UNAVAILABLE.into()),
        }
    }
    #[cfg(target_os = "android")]
    {
        super::mobile::secret_delete(&name(device)).map_err(|_| UNAVAILABLE.to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "android")))]
    {
        let _ = device;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    /// A real round trip through this computer's password store, under an
    /// id no device has; skipped where the store cannot be reached (a
    /// build machine without one).
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    fn a_password_is_kept_read_and_forgotten() {
        let device = format!("test-{}", uuid::Uuid::new_v4());
        if super::set(&device, "sesame").is_err() {
            eprintln!("no password store here; skipped");
            return;
        }
        assert_eq!(super::get(&device).unwrap().as_deref(), Some("sesame"));
        super::set(&device, "another").unwrap();
        assert_eq!(super::get(&device).unwrap().as_deref(), Some("another"));
        super::forget(&device).unwrap();
        assert_eq!(super::get(&device).unwrap(), None);
        // Forgetting what is not there is fine.
        super::forget(&device).unwrap();
    }
}
