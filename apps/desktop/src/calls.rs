//! Doorbell calls through the OneTouch cloud, in the window: the account's
//! app line rings here, a call shows the caller's camera and plays their
//! voice, and the page's microphone goes back. The line runs on its own
//! thread (`intercom::phone`); the page gets its events as `call-event`
//! and the media through a channel it opens with `calls_media`.

use super::blocking;
use base64::Engine as _;
use kagaz_core::intercom::onetouch::calls::CallDevice;
use kagaz_core::intercom::onetouch::{self, Session};
use kagaz_core::intercom::phone::{Event, MediaOut, Phone};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager};

/// Whether this app takes the account's calls; kept in `<config>/calls.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: true }
    }
}

fn settings_path() -> std::path::PathBuf {
    kagaz_core::paths::config_dir().join("calls.json")
}

fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_settings(s: &Settings) {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serde_json::to_string_pretty(s).unwrap_or_default());
}

/// What the page shows.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    enabled: bool,
    /// "off", "connecting", "up", "down", "signed_out", "expired"
    line: String,
    detail: String,
    devices: Vec<CallDevice>,
    /// "idle", "ringing", "calling", "talking"
    phase: String,
    /// Who: a device name, or the number.
    peer: String,
    video: bool,
    /// Seconds since the epoch the call connected (0 before).
    since: u64,
    /// The door key of the peer, when it has a door.
    door_dtmf: String,
}

#[derive(Default)]
pub struct CallsState {
    phone: Mutex<Option<Phone>>,
    status: Mutex<Status>,
    media: Mutex<Option<Channel<InvokeResponseBody>>>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn update(app: &AppHandle, f: impl FnOnce(&mut Status)) {
    let state = app.state::<CallsState>();
    let snapshot = {
        let mut s = state.status.lock().unwrap();
        f(&mut s);
        s.clone()
    };
    let _ = app.emit("call-event", &snapshot);
    #[cfg(mobile)]
    super::mobile::call_changed(app, &snapshot.phase, &snapshot.peer);
}

fn device_for<'a>(status: &'a Status, number: &str) -> Option<&'a CallDevice> {
    status.devices.iter().find(|d| d.sip == number)
}

fn on_event(app: &AppHandle, e: Event) {
    match e {
        Event::Line { up, detail } => update(app, |s| {
            s.line = if up { "up" } else { "down" }.into();
            s.detail = detail;
        }),
        Event::Incoming {
            number,
            display,
            video,
            ..
        } => {
            update(app, |s| {
                let dev = device_for(s, &number).cloned();
                s.phase = "ringing".into();
                s.peer = dev
                    .as_ref()
                    .map(|d| d.name.clone())
                    .or_else(|| (!display.is_empty()).then(|| display.clone()))
                    .unwrap_or(number.clone());
                s.door_dtmf = dev.map(|d| d.door_dtmf).unwrap_or_default();
                s.video = video;
                s.since = 0;
            });
            super::doorbell::attention(app);
        }
        Event::Outgoing { number, .. } => update(app, |s| {
            let dev = device_for(s, &number).cloned();
            s.phase = "calling".into();
            s.peer = dev.as_ref().map(|d| d.name.clone()).unwrap_or(number);
            s.door_dtmf = dev.map(|d| d.door_dtmf).unwrap_or_default();
            s.video = false;
            s.since = 0;
        }),
        Event::Ringing { .. } => update(app, |s| s.detail = "ringing there".into()),
        Event::Connected { video, .. } => update(app, |s| {
            s.phase = "talking".into();
            s.video = video;
            s.since = now();
        }),
        Event::Ended { reason, .. } => update(app, |s| {
            s.phase = "idle".into();
            s.detail = reason;
            s.since = 0;
            s.video = false;
        }),
    }
}

/// Media to the page: one byte of kind (1 voice, 2 keyframe, 3 frame),
/// four bytes of RTP timestamp, then 8 kHz 16-bit voice or Annex-B video.
fn on_media(app: &AppHandle, m: MediaOut) {
    let state = app.state::<CallsState>();
    let guard = state.media.lock().unwrap();
    let Some(channel) = guard.as_ref() else {
        return;
    };
    let packet = match m {
        MediaOut::Audio(pcm) => {
            let mut p = Vec::with_capacity(5 + pcm.len() * 2);
            p.push(1);
            p.extend_from_slice(&0u32.to_be_bytes());
            for s in pcm {
                p.extend_from_slice(&s.to_le_bytes());
            }
            p
        }
        MediaOut::Video(unit) => {
            let mut p = Vec::with_capacity(5 + unit.data.len());
            p.push(if unit.key { 2 } else { 3 });
            p.extend_from_slice(&unit.timestamp.to_be_bytes());
            p.extend_from_slice(&unit.data);
            p
        }
    };
    let _ = channel.send(InvokeResponseBody::Raw(packet));
}

/// WebKitGTK keeps the microphone off unless the app turns it on, and asks
/// the app (not the person) whether a page may use it. The page here is
/// Kagaz's own, so the answer is yes.
#[cfg(target_os = "linux")]
fn allow_microphone(app: &AppHandle) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    ONCE.call_once(|| {
        let _ = window.with_webview(|wv| {
            use webkit2gtk::glib::prelude::*;
            use webkit2gtk::{PermissionRequestExt, SettingsExt, WebViewExt};
            let view = wv.inner();
            if let Some(settings) = view.settings() {
                settings.set_enable_media_stream(true);
            }
            view.connect_permission_request(|_, request| {
                if request.is::<webkit2gtk::UserMediaPermissionRequest>() {
                    request.allow();
                    true
                } else {
                    false
                }
            });
        });
    });
}

/// Start or restart the line from the saved OneTouch session; stop it when
/// calls are off or there is no session.
pub fn start(app: &AppHandle) {
    #[cfg(target_os = "linux")]
    allow_microphone(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<CallsState>();
        // Stop any line first (dropping it signs out).
        drop(state.phone.lock().unwrap().take());
        let settings = load_settings();
        update(&app, |s| {
            s.enabled = settings.enabled;
            s.phase = "idle".into();
        });
        if !settings.enabled {
            update(&app, |s| {
                s.line = "off".into();
                s.detail = String::new();
            });
            #[cfg(mobile)]
            super::mobile::keep_listening(&app, false);
            return;
        }
        update(&app, |s| {
            s.line = "connecting".into();
            s.detail = String::new();
        });
        let setup = match Session::load() {
            Ok(Some(session)) => session.call_setup(),
            Ok(None) => {
                update(&app, |s| {
                    s.line = "signed_out".into();
                    s.detail = "Sign in to OneTouch below to take its calls here.".into();
                });
                return;
            }
            Err(e) => Err(e),
        };
        let setup = match setup {
            Ok(s) => s,
            Err(onetouch::Error::Expired) => {
                update(&app, |s| {
                    s.line = "expired".into();
                    s.detail = "The OneTouch sign-in has expired. Reconnect it below, then turn calls on again.".into();
                });
                return;
            }
            Err(e) => {
                update(&app, |s| {
                    s.line = "down".into();
                    s.detail = e.to_string();
                });
                return;
            }
        };
        update(&app, |s| s.devices = setup.devices.clone());
        if setup.rtp_scrambled {
            update(&app, |s| {
                s.detail =
                    "This account scrambles call media; calls may ring but stay silent.".into()
            });
        }
        let (a1, a2) = (app.clone(), app.clone());
        let phone = Phone::start(
            setup.line.clone(),
            move |e| on_event(&a1, e),
            move |m| on_media(&a2, m),
        );
        *state.phone.lock().unwrap() = Some(phone);
        #[cfg(mobile)]
        super::mobile::keep_listening(&app, true);
    });
}

fn with_phone(app: &AppHandle, f: impl FnOnce(&Phone)) -> Result<(), String> {
    let state = app.state::<CallsState>();
    let guard = state.phone.lock().unwrap();
    let phone = guard
        .as_ref()
        .ok_or("calls are not connected; turn them on first")?;
    f(phone);
    Ok(())
}

#[tauri::command]
pub fn calls_status(state: tauri::State<'_, CallsState>) -> Status {
    state.status.lock().unwrap().clone()
}

#[tauri::command]
pub fn calls_enable(app: AppHandle, enabled: bool) {
    save_settings(&Settings { enabled });
    start(&app);
}

/// Reconnect the line, e.g. after signing in to OneTouch again.
#[tauri::command]
pub fn calls_restart(app: AppHandle) {
    start(&app);
}

#[tauri::command]
pub fn calls_answer(app: AppHandle) -> Result<(), String> {
    with_phone(&app, Phone::answer)
}

#[tauri::command]
pub fn calls_decline(app: AppHandle) -> Result<(), String> {
    with_phone(&app, Phone::decline)
}

#[tauri::command]
pub fn calls_hangup(app: AppHandle) -> Result<(), String> {
    with_phone(&app, Phone::hangup)
}

#[tauri::command]
pub fn calls_call(app: AppHandle, number: String) -> Result<(), String> {
    with_phone(&app, |p| p.call(&number))
}

#[tauri::command]
pub fn calls_press(app: AppHandle, key: String) -> Result<(), String> {
    let key = key.chars().next().ok_or("no key")?;
    with_phone(&app, |p| p.dtmf(key))
}

/// Microphone audio from the page: 8 kHz 16-bit little-endian mono, base64.
#[tauri::command]
pub async fn calls_audio(app: AppHandle, data: String) -> Result<(), String> {
    blocking(move || {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())?;
        let pcm: Vec<i16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        with_phone(&app, |p| p.send_audio(&pcm))
    })
    .await
}

/// The page's media channel (voice and pictures of the current call).
#[tauri::command]
pub fn calls_media(state: tauri::State<'_, CallsState>, channel: Channel<InvokeResponseBody>) {
    *state.media.lock().unwrap() = Some(channel);
}
