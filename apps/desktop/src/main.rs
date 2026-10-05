//! The Kagaz window: a thin layer over kagaz-core. Every command here has a
//! CLI twin; the window never does anything the command line cannot.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use kagaz_core::output::{self, Format, OutputOptions};
use kagaz_core::scan::{ColorMode, ScanRequest};
use kagaz_core::{explain, open_ports, Device, DiscoverOptions, Explanation, Host};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
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
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            discover,
            explain_device,
            choose_save_path,
            scan,
            default_scan_name,
            button_settings,
            button_settings_set,
            button_settings_reset
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Kagaz window");
}
