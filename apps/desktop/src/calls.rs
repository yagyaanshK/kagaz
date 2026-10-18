//! Doorbell calls through the OneTouch cloud, in the window: the account's
//! app line rings here, a call shows the caller's camera and plays their
//! voice, and the page's microphone goes back. The line runs on its own
//! thread (`intercom::phone`); the page gets its events as `call-event`
//! and the media through a channel it opens with `calls_media`.

use super::blocking;
use base64::Engine as _;
use kagaz_core::intercom::onetouch;
use kagaz_core::intercom::onetouch::calls::{CallDevice, CallSetup};
use kagaz_core::intercom::phone::{Event, MediaOut, Phone};
use kagaz_core::intercom::SipAccount;
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
    {
        // The ring notification plays the person's chosen tone.
        let tone = if snapshot.phase == "ringing" {
            ring_file(&snapshot)
        } else {
            String::new()
        };
        super::mobile::call_changed(app, &snapshot.phase, &snapshot.peer, &tone);
    }
}

/// The tone file for the current ring: the peer's own, else its kind's.
#[cfg_attr(not(mobile), allow(dead_code))]
fn ring_file(status: &Status) -> String {
    use super::ringtones;
    let kind = status
        .devices
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(status.peer.trim()))
        .map(|d| ringtones::kind_of(&d.kind))
        .unwrap_or("doorbell");
    let id = ringtones::tone_for(&ringtones::load(), &status.peer, kind);
    ringtones::file_for(&id)
        .map(|p| p.display().to_string())
        .unwrap_or_default()
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
        Event::Others { .. } => {}
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

/// Whether the person wants this device to ring ("Ring me here").
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn wanted() -> bool {
    load_settings().enabled
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
                s.devices.clear();
            });
            #[cfg(mobile)]
            super::mobile::keep_listening(&app, false);
            return;
        }
        update(&app, |s| {
            s.line = "connecting".into();
            s.detail = String::new();
        });
        // A fresh setup from the window's own session (renewed, or signed
        // in again from the saved login), kept for next time; when that
        // fails, the line kept from last time: the call server is an
        // address, so ringing needs neither DNS nor a live sign-in.
        let (line, devices) = match fresh_setup(&app) {
            Ok(setup) => {
                if setup.rtp_scrambled {
                    update(&app, |s| {
                        s.detail =
                            "This account scrambles call media; calls may ring but stay silent."
                                .into()
                    });
                }
                save_line(&SavedLine {
                    line: setup.line.clone(),
                    devices: setup.devices.clone(),
                });
                (setup.line, setup.devices)
            }
            Err((line_state, detail)) => match load_line() {
                Some(saved) => {
                    eprintln!("calls: {detail}; using the saved call line");
                    (saved.line, saved.devices)
                }
                None => {
                    eprintln!("calls: {detail}");
                    // Nothing to call with: the list goes too, so the
                    // page never offers devices it cannot reach.
                    update(&app, |s| {
                        s.line = line_state.into();
                        s.detail = detail;
                        s.devices.clear();
                    });
                    return;
                }
            },
        };
        update(&app, |s| s.devices = devices.clone());
        let (a1, a2) = (app.clone(), app.clone());
        let phone = Phone::start(line, move |e| on_event(&a1, e), move |m| on_media(&a2, m));
        *state.phone.lock().unwrap() = Some(phone);
        #[cfg(mobile)]
        super::mobile::keep_listening(&app, true);
    });
}

/// The call setup from a live OneTouch session, or why there is none
/// (the line state to show, and the detail).
fn fresh_setup(app: &AppHandle) -> Result<CallSetup, (&'static str, String)> {
    let session = match super::onetouch::session_for_calls(app) {
        Ok(Some(session)) => session,
        Ok(None) => {
            return Err((
                "signed_out",
                "Sign in to OneTouch below to take its calls here.".into(),
            ))
        }
        Err(e) => return Err(("down", e)),
    };
    session.call_setup().map_err(|e| match e {
        onetouch::Error::Expired => (
            "expired",
            "The OneTouch sign-in has expired. Reconnect it below.".into(),
        ),
        e => ("down", e.to_string()),
    })
}

/// The account's call line as last set up, so the bell rings without a
/// live sign-in. It holds the line's own password (not the account's), so
/// it lives in a file only this user can read, and goes on sign-out.
#[derive(Serialize, Deserialize)]
struct SavedLine {
    line: SipAccount,
    devices: Vec<CallDevice>,
}

fn line_path() -> std::path::PathBuf {
    kagaz_core::paths::config_dir().join("onetouch-line.json")
}

fn save_line(saved: &SavedLine) {
    let path = line_path();
    let Some(dir) = path.parent() else {
        return;
    };
    let _ = std::fs::create_dir_all(dir);
    let Ok(text) = serde_json::to_vec(saved) else {
        return;
    };
    let temp = dir.join(".onetouch-line.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&temp).and_then(|mut f| {
        use std::io::Write;
        f.write_all(&text)?;
        f.sync_all()
    });
    if written.is_ok() {
        let _ = std::fs::rename(&temp, &path);
    } else {
        let _ = std::fs::remove_file(&temp);
    }
}

fn load_line() -> Option<SavedLine> {
    serde_json::from_slice(&std::fs::read(line_path()).ok()?).ok()
}

/// Signing out of OneTouch also forgets the call line and stops it.
pub fn forget_line(app: &AppHandle) {
    let _ = std::fs::remove_file(line_path());
    start(app);
}

/// After a OneTouch sign-in: bring the line up if it was waiting for one.
pub fn signed_in(app: &AppHandle) {
    let state = app.state::<CallsState>();
    let waiting = {
        let s = state.status.lock().unwrap();
        s.enabled && matches!(s.line.as_str(), "signed_out" | "expired" | "down")
    };
    if waiting {
        start(app);
    }
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
