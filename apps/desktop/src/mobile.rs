//! What the phone does differently. Android has no print queues an app can
//! reach and no folder a scan can simply be written to, so:
//!
//! - printing through "the installed driver" means Android's print dialog,
//!   which uses whatever print service (vendor plugin, Mopria) the phone
//!   has; the document goes to it as a PDF;
//! - a scan is written to the app's cache, then copied into Downloads/Kagaz
//!   where the Files app and other apps see it.
//!
//! The Kotlin half is `KagazPlugin` in the Android project
//! (gen/android/app/src/main/java/life/omniverse/kagaz/KagazPlugin.kt); it
//! also holds Wi-Fi's multicast lock so discovery hears mDNS and WSD.

use kagaz_core::output::{self, Format, OutputOptions};
use kagaz_core::print::{Fit, PrintRequest};
use kagaz_core::scan::{ColorMode, Page};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime, Wry};

struct Phone<R: Runtime>(PluginHandle<R>);

/// The plugin that connects to the Kotlin side.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("kagaz-phone")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let handle = api.register_android_plugin("life.omniverse.kagaz", "KagazPlugin")?;
                app.manage(Phone(handle));
            }
            #[cfg(not(target_os = "android"))]
            let _ = (app, api);
            Ok(())
        })
        .build()
}

fn phone(app: &AppHandle) -> Result<&PluginHandle<Wry>, String> {
    app.try_state::<Phone<Wry>>()
        .map(|s| &s.inner().0)
        .ok_or_else(|| "this phone's print and file services are not available".to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrintArgs<'a> {
    path: &'a str,
    job_name: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveArgs<'a> {
    path: &'a str,
    name: &'a str,
    mime: &'a str,
}

#[derive(Deserialize)]
struct Saved {
    location: String,
}

fn scratch(name: &str) -> Result<PathBuf, String> {
    let dir = kagaz_core::paths::cache_dir().join("phone");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(name))
}

/// Pages as one PDF in the cache.
fn pages_pdf(pages: &[Page], color: bool, name: &str) -> Result<PathBuf, String> {
    let path = scratch(name)?;
    output::write(
        pages,
        &OutputOptions {
            format: Format::Pdf,
            color: if color {
                ColorMode::Color
            } else {
                ColorMode::Gray
            },
            quality: Some(90),
            max_bytes: None,
        },
        &path,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

fn open_dialog(
    app: &AppHandle,
    pdf: &Path,
    job_name: &str,
) -> Result<super::printing::Sent, String> {
    let path = pdf.to_str().ok_or("unusable file name")?;
    phone(app)?
        .run_mobile_plugin::<()>("print", PrintArgs { path, job_name })
        .map_err(|e| e.to_string())?;
    Ok(super::printing::Sent::new(
        "opened the phone's print dialog; copies, sides and paper are chosen there".into(),
    ))
}

/// Print a PDF, JPEG or PNG through the phone's print dialog.
pub fn print_document(
    app: &AppHandle,
    path: &Path,
    req: &PrintRequest,
) -> Result<super::printing::Sent, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let job_name = req.job_name.clone().unwrap_or_else(|| {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Kagaz print")
            .to_string()
    });
    if data.starts_with(b"%PDF") {
        return open_dialog(app, path, &job_name);
    }
    // A picture becomes a one-page PDF sized so it fits A4 or Letter.
    let img = image::load_from_memory(&data).map_err(|e| e.to_string())?;
    let dpi = ((img.width() as f32 / 8.0).max(img.height() as f32 / 10.5)).ceil() as u32;
    let page = Page {
        mime: if data.starts_with(&[0xFF, 0xD8]) {
            "image/jpeg"
        } else {
            "image/png"
        },
        data,
        dpi: dpi.max(72),
        color: ColorMode::Color,
    };
    let pdf = pages_pdf(&[page], req.color, "print.pdf")?;
    open_dialog(app, &pdf, &job_name)
}

/// Print scanned pages through the phone's print dialog.
pub fn print_scans(
    app: &AppHandle,
    pages: &[Page],
    req: &PrintRequest,
    _fit: Fit,
) -> Result<super::printing::Sent, String> {
    let pdf = pages_pdf(pages, req.color, "copy.pdf")?;
    open_dialog(app, &pdf, req.job_name.as_deref().unwrap_or("Kagaz copy"))
}

/// Copy a finished file into Downloads/Kagaz; returns where it went.
pub fn publish(app: &AppHandle, path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("unusable file name")?;
    let mime = match path.extension().and_then(|e| e.to_str()) {
        Some("pdf") => "application/pdf",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("mp4") => "video/mp4",
        _ => "application/octet-stream",
    };
    let saved: Saved = phone(app)?
        .run_mobile_plugin(
            "saveToDownloads",
            SaveArgs {
                path: path.to_str().ok_or("unusable file name")?,
                name,
                mime,
            },
        )
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(path);
    Ok(saved.location)
}

/// Where a file the person will save goes first: the cache, then
/// [`publish`] moves it to Downloads.
pub fn save_path(name: &str) -> Result<String, String> {
    Ok(scratch(name)?.display().to_string())
}

#[derive(Serialize)]
struct ListenArgs {
    on: bool,
}

#[derive(Serialize)]
struct CallArgs<'a> {
    phase: &'a str,
    peer: &'a str,
}

/// Keep the doorbell line alive in the background (a foreground service
/// with its own small notification), or let it go.
pub fn keep_listening(app: &AppHandle, on: bool) {
    if let Ok(p) = phone(app) {
        let _ = p.run_mobile_plugin::<()>("keepListening", ListenArgs { on });
    }
}

/// Ring (a call notification that can light the screen) or stop ringing.
pub fn call_changed(app: &AppHandle, phase: &str, peer: &str) {
    if let Ok(p) = phone(app) {
        let _ = p.run_mobile_plugin::<()>("callChanged", CallArgs { phase, peer });
    }
}
