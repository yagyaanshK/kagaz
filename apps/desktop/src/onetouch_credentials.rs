//! One remembered doorbell account, held only in the operating system vault.
//! Secrets never enter Status, command errors, logs, or Kagaz's config files.
use kagaz_core::intercom::onetouch::Provider;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct SavedAccount {
    pub username: String,
    pub provider: Provider,
    pub legacy_device: String,
}

// Deliberately no Debug implementation.
#[derive(Serialize, Deserialize)]
pub(super) struct SavedLogin {
    pub account: SavedAccount,
    pub password: String,
}

pub(super) const SUPPORTED: bool = cfg!(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows"
));
const UNAVAILABLE: &str = "Could not access your system's password store. Unlock it and try again.";

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("life.omniverse.kagaz.onetouch", "remembered-account")
        .map_err(|_| UNAVAILABLE.to_string())
}

pub(super) fn read() -> Result<Option<SavedLogin>, String> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        match entry()?.get_password() {
            Ok(secret) => decode(&secret).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(UNAVAILABLE.into()),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Ok(None)
    }
}

fn decode(secret: &str) -> Result<SavedLogin, String> {
    let saved: SavedLogin = serde_json::from_str(secret).map_err(|_| {
        "The saved sign-in could not be read. Forget it and sign in again.".to_string()
    })?;
    if saved.account.username.is_empty() || saved.password.is_empty() {
        return Err("The saved sign-in is incomplete. Forget it and sign in again.".into());
    }
    Ok(saved)
}

pub(super) fn save(saved: &SavedLogin) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        let secret =
            serde_json::to_string(saved).map_err(|_| "Could not prepare the saved sign-in.")?;
        entry()?
            .set_password(&secret)
            .map_err(|_| UNAVAILABLE.to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = saved;
        Err("Remembering credentials is not available on this platform yet.".into())
    }
}

pub(super) fn forget() -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(UNAVAILABLE.into()),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_incomplete_or_invalid_records_without_echoing_secrets() {
        let bad = "private-value-not-json";
        let error = decode(bad).err().unwrap();
        assert!(!error.contains(bad));
        assert!(decode(r#"{"account":{"username":"test@example.test","provider":"smart_cloud","legacy_device":""},"password":""}"#).is_err());
    }
    // Explicitly run with --ignored to check a real desktop vault. A separate,
    // temporary entry holds synthetic data and is removed even after failure.
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    #[ignore = "requires an unlocked OS credential store"]
    fn system_vault_roundtrip() {
        let name = format!(
            "test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let entry = keyring::Entry::new("life.omniverse.kagaz.onetouch.test", &name).unwrap();
        struct Cleanup(keyring::Entry);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.delete_credential();
            }
        }
        let cleanup = Cleanup(entry);
        let saved = SavedLogin {
            account: SavedAccount {
                username: "resident@example.test".into(),
                provider: Provider::SmartCloud,
                legacy_device: String::new(),
            },
            password: "synthetic-test-secret".into(),
        };
        cleanup
            .0
            .set_password(&serde_json::to_string(&saved).unwrap())
            .unwrap();
        let restored = decode(&cleanup.0.get_password().unwrap()).unwrap();
        assert_eq!(restored.password, saved.password);
        assert_eq!(restored.account.username, saved.account.username);
        cleanup.0.delete_credential().unwrap();
        assert!(matches!(
            cleanup.0.get_password(),
            Err(keyring::Error::NoEntry)
        ));
    }
}
