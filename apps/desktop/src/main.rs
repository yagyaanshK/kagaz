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
}

struct CameraEngine {
    _server: std::sync::Arc<kagaz_core::cameras::tapo::TsServer>,
    engine: kagaz_core::extras::go2rtc::Engine,
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
        let session = Session::load(&Session::default_path()).map_err(|_| {
            "Not logged in to TP-Link: run `kagaz tapo login` in a terminal once.".to_string()
        })?;
        let cameras = session.cameras().map_err(|e| e.to_string())?;
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
            engine,
            cameras,
        })
    })
    .await?;
    *state.engine.lock().unwrap() = Some(built);
    Ok(cameras_status(state))
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
