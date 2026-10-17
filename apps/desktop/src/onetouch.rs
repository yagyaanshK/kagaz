//! OneTouch account commands. All mutations run under one lock so logout
//! cannot race a late login, token refresh, or stream start.
use kagaz_core::extras::go2rtc::{self, Engine};
use kagaz_core::intercom::onetouch::{
    self, Account, Device, Provider, Recording, Recordings, Session,
};
#[path = "onetouch_credentials.rs"]
mod credentials;
use credentials::{SavedAccount, SavedLogin};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct OneTouchState(Mutex<Inner>);
#[derive(Default)]
struct Inner {
    loaded: bool,
    saved: Option<SavedAccount>,
    notice: String,
    session: Option<Session>,
    account: Option<Account>,
    recordings: Vec<Recording>,
    engine: Option<Engine>,
    bridge: Option<onetouch::rtsp::Bridge>,
}
#[derive(Serialize)]
pub struct Status {
    logged_in: bool,
    remembered: Option<SavedAccount>,
    remember_supported: bool,
    notice: String,
    username: String,
    provider: Provider,
    service_name: String,
    devices: Vec<Device>,
}
impl Inner {
    fn stop(&mut self) {
        if let Some(engine) = &self.engine {
            engine.remove_stream("onetouch_live");
        }
        self.bridge = None;
    }
    fn status(&self) -> Status {
        Status {
            logged_in: self.session.is_some(),
            remembered: self.saved.clone(),
            remember_supported: credentials::SUPPORTED,
            notice: self.notice.clone(),
            provider: self
                .session
                .as_ref()
                .map(|s| s.provider)
                .unwrap_or_default(),
            service_name: self
                .session
                .as_ref()
                .map(|s| s.provider.label())
                .unwrap_or("OneTouch")
                .into(),
            username: self
                .session
                .as_ref()
                .map(|s| s.username.clone())
                .unwrap_or_default(),
            devices: self
                .account
                .as_ref()
                .map(|a| a.devices.clone())
                .unwrap_or_default(),
        }
    }
    fn load(&mut self) -> Result<(), String> {
        if !self.loaded {
            self.session = Session::load().map_err(|e| e.to_string())?;
            self.read_saved();
            self.loaded = true;
        }
        Ok(())
    }
    fn read_saved(&mut self) {
        match credentials::read() {
            Ok(saved) => self.saved = saved.map(|s| s.account),
            Err(e) => self.notice = e,
        }
    }
    fn accept(&mut self, session: Session, account: Account) -> Result<(), String> {
        session.save().map_err(|e| e.to_string())?;
        self.stop();
        self.engine = None;
        self.recordings.clear();
        self.session = Some(session);
        self.account = Some(account);
        self.loaded = true;
        Ok(())
    }
    fn cloud_error(&mut self, e: onetouch::Error) -> String {
        if matches!(e, onetouch::Error::Expired) {
            self.stop();
            self.engine = None;
            self.session = None;
            self.account = None;
            self.recordings.clear();
            let _ = Session::logout();
        }
        e.to_string()
    }
}
#[tauri::command]
pub async fn onetouch_status(app: AppHandle) -> Result<Status, String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let mut s = state.0.lock().unwrap();
        s.load()?;
        Ok(s.status())
    })
    .await
}
#[tauri::command]
pub async fn onetouch_login(
    app: AppHandle,
    username: String,
    password: String,
    provider: Option<Provider>,
    legacy_device: Option<String>,
    remember: Option<bool>,
) -> Result<Status, String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let mut s = state.0.lock().unwrap();
        let (session, account) = Session::login_with_provider(
            provider.unwrap_or_default(),
            &username,
            &password,
            legacy_device.as_deref().unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?;
        s.accept(session, account)?;
        s.notice.clear();
        let stored = if remember.unwrap_or(false) {
            credentials::save(&SavedLogin {
                account: SavedAccount {
                    username: username.trim().into(),
                    provider: provider.unwrap_or_default(),
                    legacy_device: legacy_device.unwrap_or_default(),
                },
                password,
            })
        } else {
            credentials::forget()
        };
        if let Err(e) = stored {
            s.notice = format!("Signed in, but the saved sign-in could not be updated. {e}");
        }
        s.read_saved();
        Ok(s.status())
    })
    .await
}
/// Replays saved credentials only when the user explicitly clicks Reconnect.
#[tauri::command]
pub async fn onetouch_reconnect(app: AppHandle) -> Result<Status, String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let mut s = state.0.lock().unwrap();
        s.load()?;
        if s.session.is_some() {
            return Ok(s.status());
        }
        let saved =
            credentials::read()?.ok_or("No saved sign-in. Enter your credentials once first.")?;
        let (session, account) = Session::login_with_provider(
            saved.account.provider,
            &saved.account.username,
            &saved.password,
            &saved.account.legacy_device,
        )
        .map_err(|e| e.to_string())?;
        s.accept(session, account)?;
        s.notice.clear();
        s.saved = Some(saved.account);
        Ok(s.status())
    })
    .await
}

#[tauri::command]
pub async fn onetouch_forget(app: AppHandle) -> Result<Status, String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let mut s = state.0.lock().unwrap();
        s.load()?;
        credentials::forget()?;
        s.saved = None;
        s.notice.clear();
        Ok(s.status())
    })
    .await
}

#[tauri::command]
pub async fn onetouch_logout(app: AppHandle) -> Result<(), String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let mut s = state.0.lock().unwrap();
        s.stop();
        s.engine = None;
        s.session = None;
        s.account = None;
        s.recordings.clear();
        s.loaded = true;
        Session::logout().map_err(|e| e.to_string())
    })
    .await
}
#[tauri::command]
pub async fn onetouch_refresh(app: AppHandle) -> Result<Status, String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let mut s = state.0.lock().unwrap();
        s.load()?;
        let Some(session) = s.session.as_mut() else {
            return Ok(s.status());
        };
        match session.account() {
            Ok(account) => s.account = Some(account),
            Err(e) => return Err(s.cloud_error(e)),
        }
        Ok(s.status())
    })
    .await
}
#[tauri::command]
pub async fn onetouch_live(app: AppHandle, device_id: String) -> Result<String, String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let mut s = state.0.lock().unwrap();
        // Refresh short-lived RTSP credentials immediately before opening video.
        let session = s
            .session
            .as_mut()
            .ok_or("Sign in to your OneTouch account first.")?;
        let account = match session.account() {
            Ok(a) => a,
            Err(e) => return Err(s.cloud_error(e)),
        };
        let device = account
            .devices
            .iter()
            .find(|d| d.id == device_id)
            .ok_or("This doorbell is no longer on your account.")?;
        let target = s
            .session
            .as_ref()
            .unwrap()
            .live_target(&account, device)
            .map_err(|e| e.to_string())?;
        s.account = Some(account);
        s.stop();
        if s.engine.as_mut().is_some_and(|e| !e.alive()) {
            s.engine = None;
        }
        if s.engine.is_none() {
            let binary = go2rtc::ensure(&mut |_| {})
                .map_err(|_| "Could not prepare Kagaz's video engine.")?;
            s.engine = Some(
                Engine::start(&binary, 0, &[])
                    .map_err(|_| "Could not start Kagaz's video engine.")?,
            );
        }
        let bridge = onetouch::rtsp::Bridge::start(target)
            .map_err(|_| "Could not start the doorbell video connection.")?;
        let engine = s.engine.as_ref().unwrap();
        engine
            .add_stream("onetouch_live", &bridge.url)
            .map_err(|_| "Could not open the doorbell video stream.")?;
        let url = engine.ws_url("onetouch_live");
        s.bridge = Some(bridge);
        Ok(url)
    })
    .await
}
#[tauri::command]
pub async fn onetouch_stop(app: AppHandle) -> Result<(), String> {
    super::blocking(move || {
        app.state::<OneTouchState>().0.lock().unwrap().stop();
        Ok(())
    })
    .await
}
#[tauri::command]
pub async fn onetouch_recordings(app: AppHandle, page: u32) -> Result<Recordings, String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let mut s = state.0.lock().unwrap();
        let role = s
            .account
            .as_ref()
            .ok_or("Refresh your OneTouch devices first.")?
            .role
            .clone();
        let session = s
            .session
            .as_mut()
            .ok_or("Sign in to your OneTouch account first.")?;
        let list = match session.recordings(&role, page) {
            Ok(r) => r,
            Err(e) => return Err(s.cloud_error(e)),
        };
        // Only the current page is playable; requests never accept arbitrary URLs.
        s.recordings = list.entries.clone();
        Ok(list)
    })
    .await
}
#[tauri::command]
pub async fn onetouch_recording_media(
    app: AppHandle,
    id: String,
    picture: bool,
) -> Result<String, String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let s = state.0.lock().unwrap();
        if s.session.is_none() {
            return Err("Sign in to your OneTouch account first.".into());
        }
        let recording = s
            .recordings
            .iter()
            .find(|r| r.id == id)
            .ok_or("Refresh the recordings list and select this entry again.")?;
        let url = if picture {
            &recording.picture
        } else {
            &recording.video
        };
        if url.is_empty() {
            return Err("OneTouch has no media for this entry.".into());
        }
        // Short-lived cloud media URL. No account token/header is exposed or forwarded.
        Ok(url.clone())
    })
    .await
}


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_keeps_remembered_metadata_separate_from_secrets() {
        let mut inner = Inner::default();
        inner.saved = Some(SavedAccount {
            username: "resident@example.test".into(),
            provider: Provider::Otipvdp,
            legacy_device: "001122334455".into(),
        });
        let status = serde_json::to_value(inner.status()).unwrap();
        assert_eq!(status["logged_in"], false);
        assert_eq!(status["remembered"]["provider"], "otipvdp");
        assert_eq!(status["remembered"]["username"], "resident@example.test");
        assert!(status["remembered"].get("password").is_none());
        assert!(status.get("token").is_none());
    }
}
