//! The Kagaz window: a thin layer over kagaz-core. Every command here has a
//! CLI twin; the window never does anything the command line cannot.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use kagaz_core::output::{self, Format, OutputOptions};
use kagaz_core::scan::{ColorMode, ScanRequest};
use kagaz_core::{explain, open_ports, Device, DiscoverOptions, Explanation, Host};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

/// Run a blocking core call off the UI thread.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn discover(timeout: u64) -> Result<Vec<Device>, String> {
    blocking(move || {
        kagaz_core::discover(&DiscoverOptions {
            timeout: Duration::from_secs(timeout.clamp(1, 30)),
            ..Default::default()
        })
        .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn explain_device(device: Device) -> Result<Explanation, String> {
    blocking(move || {
        let host = Host::detect();
        let ports = device
            .addresses
            .iter()
            .find(|a| a.is_ipv4())
            .map(|ip| open_ports(*ip, Duration::from_millis(800)))
            .unwrap_or_default();
        Ok(explain(&device, &host, &ports))
    })
    .await
}

#[tauri::command]
async fn scan_capabilities(device: Device) -> Result<kagaz_core::scan::Capabilities, String> {
    blocking(move || kagaz_core::scan::capabilities(&device).map_err(|e| e.to_string())).await
}

#[derive(Debug, Clone, Deserialize)]
struct OutputRequest {
    format: Format,
    color: ColorMode,
    quality: Option<u8>,
    max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct WrittenFile {
    path: String,
    bytes: u64,
    pages: usize,
    warning: Option<String>,
}

/// Ask where to save a scan; `None` when the person cancels.
#[tauri::command]
async fn choose_save_path(
    app: AppHandle,
    suggested_name: String,
    extension: String,
) -> Result<Option<String>, String> {
    blocking(move || {
        let label = match extension.as_str() {
            "pdf" => "PDF document",
            "jpg" => "JPEG image",
            "png" => "PNG image",
            "mp4" => "MP4 video",
            _ => "File",
        };
        let picked = app
            .dialog()
            .file()
            .set_file_name(&suggested_name)
            .add_filter(label, &[extension.as_str()])
            .blocking_save_file();
        Ok(picked.map(|p| p.to_string()))
    })
    .await
}

/// Scan and write the files; progress arrives on the `scan-progress` event.
#[tauri::command]
async fn scan(
    app: AppHandle,
    device: Device,
    request: ScanRequest,
    output: OutputRequest,
    path: String,
) -> Result<Vec<WrittenFile>, String> {
    blocking(move || {
        let pages = kagaz_core::scan::scan(&device, &request, &mut |event| {
            let _ = app.emit("scan-progress", &event);
        })
        .map_err(|e| e.to_string())?;
        let written = output::write(
            &pages,
            &OutputOptions {
                format: output.format,
                color: output.color,
                quality: output.quality,
                max_bytes: output.max_bytes,
            },
            &PathBuf::from(path),
        )
        .map_err(|e| e.to_string())?;
        Ok(written
            .into_iter()
            .map(|w| WrittenFile {
                path: w.path.display().to_string(),
                bytes: w.bytes,
                pages: w.pages,
                warning: w.warning,
            })
            .collect())
    })
    .await
}

#[derive(Debug, Clone, Deserialize)]
struct ButtonChange {
    action: String,
    resolution: u32,
    size: String,
    duplex: bool,
}

/// The Brother Scan-button settings on this computer, when the tool is installed.
#[tauri::command]
fn button_settings() -> Option<Vec<kagaz_core::scan::button_settings::ActionSettings>> {
    use kagaz_core::scan::button_settings as bs;
    bs::available().then(bs::get_all)
}

#[tauri::command]
fn button_settings_set(
    change: ButtonChange,
) -> Result<Vec<kagaz_core::scan::button_settings::ActionSettings>, String> {
    use kagaz_core::scan::button_settings as bs;
    bs::set(
        &change.action,
        change.resolution,
        change.duplex,
        &change.size,
    )
    .map_err(|e| e.to_string())?;
    Ok(bs::get_all())
}

#[tauri::command]
fn button_settings_reset(
    action: String,
) -> Result<Vec<kagaz_core::scan::button_settings::ActionSettings>, String> {
    use kagaz_core::scan::button_settings as bs;
    bs::reset(&action).map_err(|e| e.to_string())?;
    Ok(bs::get_all())
}

/// The camera engine, kept alive for the window's lifetime once started.
struct CameraState {
    engine: std::sync::Mutex<Option<CameraEngine>>,
    /// Cameras with a download in progress (a camera serves one at a time).
    downloading: std::sync::Mutex<std::collections::HashSet<String>>,
    /// A login waiting for its second-factor code: the session and the code type.
    pending_login: std::sync::Mutex<Option<(kagaz_core::cameras::tapo::Session, u32)>>,
}

#[derive(Clone)]
struct CameraEngine {
    _server: std::sync::Arc<kagaz_core::cameras::tapo::TsServer>,
    engine: std::sync::Arc<kagaz_core::extras::go2rtc::Engine>,
    cameras: Vec<kagaz_core::cameras::tapo::Camera>,
}

#[derive(Debug, Clone, Serialize)]
struct CameraView {
    name: String,
    model: String,
    device_id: String,
    /// go2rtc stream name.
    stream: String,
    /// WebSocket URL for go2rtc's player (MSE/WebRTC), HD.
    ws_url: String,
    /// The same for the low-resolution stream, for small tiles.
    ws_url_vga: String,
    /// MPEG-TS over HTTP, for a plain <video> (the route WebKit plays),
    /// each camera on its own loopback host so the webview's per-host
    /// connection limit never applies.
    ts_url: String,
    ts_url_vga: String,
}

#[derive(Debug, Clone, Serialize)]
struct CamerasStatus {
    logged_in: bool,
    running: bool,
    api_port: u16,
    player_script: String,
    cameras: Vec<CameraView>,
    message: String,
}

fn camera_views(e: &CameraEngine) -> Vec<CameraView> {
    e.cameras
        .iter()
        .enumerate()
        .map(|(i, c)| CameraView {
            name: c.name.clone(),
            model: c.model.clone(),
            device_id: c.device_id.clone(),
            stream: kagaz_core::extras::go2rtc::yaml_key(&c.name),
            ws_url: e.engine.ws_url(&c.name),
            ws_url_vga: e.engine.ws_url(&format!("{} vga", c.name)),
            ts_url: e
                ._server
                .tile_url(i, &kagaz_core::extras::go2rtc::yaml_key(&c.name)),
            ts_url_vga: e._server.tile_url(
                i,
                &kagaz_core::extras::go2rtc::yaml_key(&format!("{} vga", c.name)),
            ),
        })
        .collect()
}

/// Where the cameras stand: not logged in, ready, or running with its streams.
#[tauri::command]
fn cameras_status(state: tauri::State<CameraState>) -> CamerasStatus {
    use kagaz_core::cameras::tapo::Session;
    let logged_in = Session::default_path().is_file();
    let guard = state.engine.lock().unwrap();
    match guard.as_ref() {
        Some(e) => CamerasStatus {
            logged_in,
            running: true,
            api_port: e.engine.api_port,
            player_script: format!("http://127.0.0.1:{}/video-stream.js", e.engine.api_port),
            cameras: camera_views(e),
            message: String::new(),
        },
        None => CamerasStatus {
            logged_in,
            running: false,
            api_port: 0,
            player_script: String::new(),
            cameras: Vec::new(),
            message: if logged_in {
                String::new()
            } else {
                "Not logged in to TP-Link: run `kagaz tapo login` in a terminal once.".into()
            },
        },
    }
}

/// Start serving the account's cameras and the video engine (downloaded on first use).
#[tauri::command]
async fn cameras_start(
    app: AppHandle,
    state: tauri::State<'_, CameraState>,
) -> Result<CamerasStatus, String> {
    use kagaz_core::cameras::tapo::{Session, TsServer};
    use kagaz_core::extras::go2rtc;
    if state.engine.lock().unwrap().is_some() {
        return Ok(cameras_status(state));
    }
    let built = blocking(move || {
        let mut session = Session::load(&Session::default_path()).map_err(|_| {
            "Not logged in to TP-Link: use \"Log in\" here or `kagaz tapo login` in a terminal."
                .to_string()
        })?;
        let cameras = match session.cameras() {
            Ok(c) => c,
            // An old token: refresh it once, then ask again.
            Err(kagaz_core::cameras::tapo::CloudError::Unauthorized) => {
                session
                    .refresh()
                    .map_err(|_| "The TP-Link session has expired: log in again.".to_string())?;
                let _ = session.save(&Session::default_path());
                session.cameras().map_err(|e| e.to_string())?
            }
            Err(e) => return Err(e.to_string()),
        };
        let server = TsServer::start(session, cameras.clone(), 0).map_err(|e| e.to_string())?;
        let binary = go2rtc::ensure(&mut |ev| {
            let text = match ev {
                go2rtc::Event::Downloading { bytes } => {
                    format!("downloading the video engine ({} MB)...", bytes / 1_000_000)
                }
                go2rtc::Event::Verified => "video engine verified".to_string(),
                go2rtc::Event::Ready(_) => "video engine ready".to_string(),
            };
            let _ = app.emit("cameras-progress", text);
        })
        .map_err(|e| e.to_string())?;
        let streams: Vec<go2rtc::StreamSource> = cameras
            .iter()
            .flat_map(|c| {
                [
                    go2rtc::StreamSource {
                        name: c.name.clone(),
                        url: server.url_for(&c.device_id),
                    },
                    go2rtc::StreamSource {
                        name: format!("{} vga", c.name),
                        url: server.vga_url_for(&c.device_id),
                    },
                ]
            })
            .collect();
        let engine = go2rtc::Engine::start(&binary, 0, &streams).map_err(|e| e.to_string())?;
        server.set_engine_port(engine.api_port);
        Ok(CameraEngine {
            _server: server,
            engine: std::sync::Arc::new(engine),
            cameras,
        })
    })
    .await?;
    *state.engine.lock().unwrap() = Some(built);
    Ok(cameras_status(state))
}

/// The running camera engine, or why there is none.
fn running(state: &tauri::State<'_, CameraState>) -> Result<CameraEngine, String> {
    state
        .engine
        .lock()
        .map_err(|_| "camera state poisoned".to_string())?
        .as_ref()
        .cloned()
        .ok_or_else(|| "the cameras are not running".to_string())
}

fn camera_of(
    e: &CameraEngine,
    device_id: &str,
) -> Result<(usize, kagaz_core::cameras::tapo::Camera), String> {
    e.cameras
        .iter()
        .position(|c| c.device_id == device_id)
        .map(|i| (i, e.cameras[i].clone()))
        .ok_or_else(|| "no such camera".to_string())
}

#[derive(Debug, Clone, Serialize)]
struct RecordingDays {
    utc_offset_minutes: i32,
    /// "YYYY-MM-DD", camera-local.
    dates: Vec<String>,
    today: String,
}

/// Days with footage on this camera's card over the last `days`.
#[tauri::command]
async fn recording_days(
    state: tauri::State<'_, CameraState>,
    device_id: String,
    days: u32,
) -> Result<RecordingDays, String> {
    use kagaz_core::cameras::tapo::recordings::{self, Recordings};
    let e = running(&state)?;
    let (_, cam) = camera_of(&e, &device_id)?;
    blocking(move || {
        let session = e._server.session();
        let rec = Recordings::new(&session, &cam);
        let offset = rec.utc_offset_minutes().map_err(|e| e.to_string())?;
        let now = unix_now();
        let today = recordings::day_of(now, offset);
        let first = recordings::day_of(now - i64::from(days) * 86_400, offset);
        let dates = rec
            .dates(&first, &today)
            .map_err(|e| e.to_string())?
            .iter()
            .map(|d| recordings::dash_date(d))
            .collect();
        Ok(RecordingDays {
            utc_offset_minutes: offset,
            dates,
            today: recordings::dash_date(&today),
        })
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
struct RecordingClips {
    utc_offset_minutes: i32,
    sd_card: kagaz_core::cameras::tapo::SdCard,
    clips: Vec<kagaz_core::cameras::tapo::Clip>,
    /// Unix time at which the day starts on the camera's clock.
    day_start: i64,
}

/// The clips of one camera-local day ("YYYY-MM-DD").
#[tauri::command]
async fn recording_clips(
    state: tauri::State<'_, CameraState>,
    device_id: String,
    date: String,
) -> Result<RecordingClips, String> {
    use kagaz_core::cameras::tapo::recordings::{self, Recordings};
    let e = running(&state)?;
    let (_, cam) = camera_of(&e, &device_id)?;
    blocking(move || {
        let session = e._server.session();
        let rec = Recordings::new(&session, &cam);
        let offset = rec.utc_offset_minutes().map_err(|e| e.to_string())?;
        let day: String = date.chars().filter(|c| c.is_ascii_digit()).collect();
        let day_start = recordings::day_start(&day, offset).ok_or("bad date")?;
        Ok(RecordingClips {
            utc_offset_minutes: offset,
            sd_card: rec.sd_card().map_err(|e| e.to_string())?,
            clips: rec.clips(&day).map_err(|e| e.to_string())?,
            day_start,
        })
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
struct PlaybackHandle {
    token: String,
    stream: String,
    /// MPEG-TS from the engine on this camera's own loopback host.
    ts_url: String,
}

/// Start playing recorded footage between two unix times through the engine.
#[tauri::command]
async fn playback_start(
    state: tauri::State<'_, CameraState>,
    device_id: String,
    from: i64,
    to: i64,
) -> Result<PlaybackHandle, String> {
    let e = running(&state)?;
    let (index, cam) = camera_of(&e, &device_id)?;
    blocking(move || {
        let (token, url) = e._server.playback_url(&cam.device_id, from, to, false);
        let stream = format!("playback-{token}");
        e.engine
            .add_stream(&stream, &url)
            .map_err(|e| e.to_string())?;
        Ok(PlaybackHandle {
            ts_url: e._server.tile_url(index, &stream),
            token,
            stream,
        })
    })
    .await
}

#[tauri::command]
fn playback_state(
    state: tauri::State<'_, CameraState>,
    token: String,
) -> Result<kagaz_core::cameras::tapo::PlaybackState, String> {
    let e = running(&state)?;
    e._server
        .playback_state(&token)
        .ok_or_else(|| "unknown playback".to_string())
}

/// End a playback: the engine drops its source and the camera frees its slot.
#[tauri::command]
async fn playback_stop(
    state: tauri::State<'_, CameraState>,
    token: String,
    stream: String,
) -> Result<(), String> {
    let e = running(&state)?;
    blocking(move || {
        e.engine.remove_stream(&stream);
        e._server.forget_playback(&token);
        Ok(())
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
struct DownloadProgress {
    device_id: String,
    bytes: u64,
    done: bool,
    error: String,
}

/// Save footage between two unix times as MP4; progress on `download-progress`.
#[tauri::command]
async fn download_recording(
    app: AppHandle,
    state: tauri::State<'_, CameraState>,
    device_id: String,
    from: i64,
    to: i64,
    path: String,
) -> Result<u64, String> {
    use kagaz_core::cameras::tapo::download;
    let e = running(&state)?;
    let (_, cam) = camera_of(&e, &device_id)?;
    {
        let mut busy = state.downloading.lock().map_err(|_| "state poisoned")?;
        if !busy.insert(device_id.clone()) {
            return Err("this camera is already sending a download; wait for it".into());
        }
    }
    let id = device_id.clone();
    let result = blocking(move || {
        let out = PathBuf::from(&path);
        download::save_span(&e._server, &e.engine, &cam, from, to, &out, &mut |p| {
            let download::Progress::Bytes(bytes) = p;
            let _ = app.emit(
                "download-progress",
                DownloadProgress {
                    device_id: id.clone(),
                    bytes,
                    done: false,
                    error: String::new(),
                },
            );
            true
        })
        .map_err(|e| e.to_string())
    })
    .await;
    if let Ok(mut busy) = state.downloading.lock() {
        busy.remove(&device_id);
    }
    result
}

#[derive(Debug, Clone, Serialize)]
struct LoginStep {
    code_needed: bool,
    message: String,
}

/// Log in to the TP-Link account; may ask for the emailed code next.
#[tauri::command]
async fn tapo_login(
    state: tauri::State<'_, CameraState>,
    email: String,
    password: String,
) -> Result<LoginStep, String> {
    use kagaz_core::cameras::tapo::{cloud, CloudError, Session};
    let step = blocking(move || {
        let mut session = Session::begin(&email);
        match session.do_login(&password) {
            Ok(()) => {
                session
                    .save(&Session::default_path())
                    .map_err(|e| e.to_string())?;
                Ok((None, LoginStep { code_needed: false, message: String::new() }))
            }
            Err(CloudError::MfaRequired { types, .. }) => {
                let mfa_type = if types.contains(&cloud::MFA_EMAIL) || types.is_empty() {
                    cloud::MFA_EMAIL
                } else {
                    types[0]
                };
                session
                    .send_mfa_code(mfa_type, &password)
                    .map_err(|e| e.to_string())?;
                let message = if mfa_type == cloud::MFA_EMAIL {
                    "TP-Link is sending a code to the account's email (check spam too); enter it here.".to_string()
                } else {
                    "Check the Tapo app on your phone for the code and enter it here.".to_string()
                };
                Ok((Some((session, mfa_type)), LoginStep { code_needed: true, message }))
            }
            Err(e) => Err(e.to_string()),
        }
    })
    .await?;
    let (pending, step) = step;
    *state.pending_login.lock().map_err(|_| "state poisoned")? = pending;
    Ok(step)
}

/// Finish a login with the second-factor code.
#[tauri::command]
async fn tapo_login_code(state: tauri::State<'_, CameraState>, code: String) -> Result<(), String> {
    use kagaz_core::cameras::tapo::Session;
    let pending = state
        .pending_login
        .lock()
        .map_err(|_| "state poisoned")?
        .clone()
        .ok_or("no login is waiting for a code; start again")?;
    let (mut session, mfa_type) = pending;
    blocking(move || {
        session
            .submit_mfa(&code, mfa_type)
            .map_err(|e| e.to_string())?;
        session
            .save(&Session::default_path())
            .map_err(|e| e.to_string())
    })
    .await?;
    *state.pending_login.lock().map_err(|_| "state poisoned")? = None;
    Ok(())
}

/// Forget the saved session (the cameras must not be running).
#[tauri::command]
fn tapo_logout(state: tauri::State<'_, CameraState>) -> Result<(), String> {
    use kagaz_core::cameras::tapo::Session;
    if running(&state).is_ok() {
        return Err("stop the cameras first (close and reopen the window)".into());
    }
    match std::fs::remove_file(Session::default_path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// The saved groups, with every current camera placed exactly once.
#[tauri::command]
fn camera_layout(
    state: tauri::State<'_, CameraState>,
) -> Result<kagaz_core::cameras::Layout, String> {
    use kagaz_core::cameras::Layout;
    let mut layout = Layout::load(&Layout::default_path()).map_err(|e| e.to_string())?;
    if let Ok(e) = running(&state) {
        let ids: Vec<String> = e.cameras.iter().map(|c| c.device_id.clone()).collect();
        layout.reconcile(&ids);
    }
    Ok(layout)
}

#[tauri::command]
fn save_camera_layout(layout: kagaz_core::cameras::Layout) -> Result<(), String> {
    use kagaz_core::cameras::Layout;
    layout
        .save(&Layout::default_path())
        .map_err(|e| e.to_string())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `KAGAZ_OPEN=cameras` opens the window on the camera grid and starts it.
#[tauri::command]
fn startup_view() -> String {
    std::env::var("KAGAZ_OPEN").unwrap_or_default()
}

#[tauri::command]
fn default_scan_name(extension: String) -> String {
    format!(
        "scan-{}.{}",
        kagaz_core::localtime::now().file_stamp(),
        extension
    )
}

fn main() {
    tauri::Builder::default()
        .manage(CameraState {
            engine: std::sync::Mutex::new(None),
            downloading: std::sync::Mutex::new(std::collections::HashSet::new()),
            pending_login: std::sync::Mutex::new(None),
        })
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            discover,
            explain_device,
            scan_capabilities,
            choose_save_path,
            scan,
            default_scan_name,
            button_settings,
            button_settings_set,
            button_settings_reset,
            cameras_status,
            cameras_start,
            recording_days,
            recording_clips,
            playback_start,
            playback_state,
            playback_stop,
            download_recording,
            camera_layout,
            save_camera_layout,
            tapo_login,
            tapo_login_code,
            tapo_logout,
            startup_view
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Kagaz window")
        .run(|app, event| {
            // Stop the camera engine when the window goes away.
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<CameraState>() {
                    if let Ok(mut guard) = state.engine.lock() {
                        guard.take();
                    }
                }
            }
        });
}
