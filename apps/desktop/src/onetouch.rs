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
    /// The saved password was refused on an automatic sign-in: no more
    /// automatic attempts until the person signs in by hand.
    auto_blocked: bool,
    last_auto: Option<std::time::Instant>,
    /// The person signed out: no automatic sign-in until they sign in again.
    signed_out: bool,
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
            self.signed_out = signed_out_marker().exists();
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
    /// Sign in again from the saved login when there is no session, so the
    /// person is not asked to press Reconnect. At most once a minute, and
    /// never again after the password was refused. Ok(true) when signed in.
    fn auto_reconnect(&mut self) -> Result<bool, String> {
        if self.session.is_some() {
            return Ok(true);
        }
        if self.auto_blocked
            || self.signed_out
            || self
                .last_auto
                .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(60))
        {
            return Ok(false);
        }
        let saved = credentials::read().inspect_err(|e| {
            eprintln!("onetouch: cannot read the saved sign-in: {e}");
        })?;
        let Some(saved) = saved else {
            eprintln!("onetouch: no saved sign-in to reconnect with");
            return Ok(false);
        };
        self.last_auto = Some(std::time::Instant::now());
        eprintln!("onetouch: signing in again from the saved sign-in");
        match Session::login_with_provider(
            saved.account.provider,
            &saved.account.username,
            &saved.password,
            &saved.account.legacy_device,
        ) {
            Ok((session, account)) => {
                self.accept(session, account)?;
                self.notice.clear();
                self.saved = Some(saved.account);
                Ok(true)
            }
            Err(onetouch::Error::Credentials) => {
                eprintln!("onetouch: the saved password was refused");
                self.auto_blocked = true;
                self.notice = "The saved OneTouch password was not accepted. Sign in again.".into();
                Ok(false)
            }
            Err(e) => {
                eprintln!("onetouch: signing in again failed: {e}");
                Err(e.to_string())
            }
        }
    }
    /// The session failed for a reason other than the network (an expired
    /// token can come back as other errors once its refresh is refused):
    /// drop it and sign in again from the saved login.
    fn sign_in_again(&mut self) -> bool {
        self.stop();
        self.session = None;
        self.account = None;
        self.auto_reconnect().unwrap_or(false)
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
        let was_out = s.session.is_none();
        if let Err(e) = s.auto_reconnect() {
            s.notice = format!("Could not reconnect to OneTouch: {e}");
        }
        if was_out && s.session.is_some() {
            super::calls::signed_in(&app);
        }
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
        s.auto_blocked = false;
        s.signed_out = false;
        let _ = std::fs::remove_file(signed_out_marker());
        super::calls::signed_in(&app);
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
/// Replays the saved credentials now. The same happens automatically when
/// the session has expired (see `Inner::auto_reconnect`).
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
        s.auto_blocked = false;
        s.signed_out = false;
        let _ = std::fs::remove_file(signed_out_marker());
        s.saved = Some(saved.account);
        super::calls::signed_in(&app);
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
        s.signed_out = true;
        let _ = std::fs::write(signed_out_marker(), b"");
        let result = Session::logout().map_err(|e| e.to_string());
        super::calls::forget_line(&app);
        result
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
            Err(e) => {
                let network = matches!(e, onetouch::Error::Network);
                let message = s.cloud_error(e);
                if network || !s.sign_in_again() {
                    return Err(message);
                }
            }
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
/// Live view for the window's own player: Kagaz speaks to the relay
/// itself, no helper. Returns the watch id (`media_unwatch` ends it).
#[tauri::command]
pub async fn onetouch_watch(
    app: AppHandle,
    device_id: String,
    channel: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
) -> Result<u64, String> {
    super::blocking(move || {
        let target = {
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
            target
        };
        let live = onetouch::live::Live::start(
            target,
            Box::new(move |m| super::media::send(&channel, &m)),
        );
        Ok(app
            .state::<super::media::Watches>()
            .add(super::media::Watch::Live(live)))
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
/// The account as last known, without signing in (Home asks this: a new
/// sign-in would sign the person's other devices out of OneTouch).
#[tauri::command]
pub async fn onetouch_peek(app: AppHandle) -> Result<Status, String> {
    super::blocking(move || {
        let state = app.state::<OneTouchState>();
        let mut s = state.0.lock().unwrap();
        s.load()?;
        Ok(s.status())
    })
    .await
}

/// The role and a usable session, refreshing the account when needed.
fn with_session<T>(
    app: &AppHandle,
    f: impl Fn(&mut Session, &str) -> Result<T, onetouch::Error>,
) -> Result<T, String> {
    let state = app.state::<OneTouchState>();
    let mut s = state.0.lock().unwrap();
    s.load()?;
    if s.session.is_none() {
        return Err("Sign in to your OneTouch account first.".into());
    }
    if s.account.is_none() {
        let account = match s.session.as_mut().unwrap().account() {
            Ok(a) => a,
            Err(e) => return Err(s.cloud_error(e)),
        };
        s.account = Some(account);
    }
    let role = s.account.as_ref().unwrap().role.clone();
    let result = f(s.session.as_mut().unwrap(), &role);
    match result {
        Ok(v) => {
            if let Some(session) = &s.session {
                let _ = session.save();
            }
            Ok(v)
        }
        Err(e) => Err(s.cloud_error(e)),
    }
}

/// One page of the account's history: "door" (rings with a picture of who
/// was there), "calls" or "captures".
#[tauri::command]
pub async fn onetouch_history(
    app: AppHandle,
    log: String,
    page: u32,
) -> Result<onetouch::history::Page, String> {
    use onetouch::history::Log;
    let log = match log.as_str() {
        "door" => Log::Door,
        "calls" => Log::Calls,
        "captures" => Log::Captures,
        _ => return Err("Unknown history.".into()),
    };
    super::blocking(move || with_session(&app, |session, role| session.history(role, log, page)))
        .await
}

/// The home's family members (each with their own login).
#[tauri::command]
pub async fn onetouch_people(app: AppHandle) -> Result<Vec<onetouch::history::Person>, String> {
    super::blocking(move || with_session(&app, |session, role| session.people(role))).await
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

/// Present while the person has signed out, so a restart does not sign
/// them back in automatically.
fn signed_out_marker() -> std::path::PathBuf {
    kagaz_core::paths::config_dir().join("onetouch-signed-out")
}

/// A live session for the call line: the window's own session, checked
/// with the cloud (renewing its token). Never a new sign-in: OneTouch keeps
/// one session per account, so signing in here would sign the person's
/// phone out each time this app starts. A new sign-in happens only when the
/// person opens the Doorbell pane (`onetouch_status`) or presses Reconnect.
/// Ok(None) when there is no session; the line then uses the setup kept
/// from last time.
pub fn session_for_calls(app: &AppHandle) -> Result<Option<Session>, String> {
    let state = app.state::<OneTouchState>();
    let mut s = state.0.lock().unwrap();
    s.load()?;
    let Some(session) = s.session.as_mut() else {
        return Ok(None);
    };
    match session.account() {
        Ok(account) => {
            let _ = session.save();
            let fresh = session.clone();
            s.account = Some(account);
            Ok(Some(fresh))
        }
        Err(e) => Err(s.cloud_error(e)),
    }
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
