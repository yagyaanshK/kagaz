//! Print and Copy for the device page. Printing goes either straight to the
//! printer without a driver (IPP Everywhere) or through one of the system's
//! print queues, and so through whatever driver is installed. A copy is a
//! scan whose pages are kept here, shown as thumbnails, then printed.

use super::blocking;
use base64::Engine as _;
use kagaz_core::ipp::status::PrinterStatus;
use kagaz_core::print::system::{self, Queue};
use kagaz_core::print::{Fit, PrintRequest};
use kagaz_core::scan::{Page, ScanRequest};
use kagaz_core::Device;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

/// Pages of the last copy scan, waiting to be printed.
#[derive(Default)]
pub struct CopyState {
    pages: Mutex<Vec<Page>>,
}

/// Where a job goes.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "via", rename_all = "snake_case")]
pub enum Target {
    /// No driver: Kagaz renders and sends over IPP.
    Driverless { device: Device },
    /// A system print queue, so the installed driver.
    Queue { name: String },
    /// The phone's own print dialog (Android).
    System,
}

#[derive(Debug, Clone, Serialize)]
pub struct Queues {
    queues: Vec<Queue>,
    /// The queue that seems to be this device, if any.
    matched: Option<String>,
    /// Whether this computer prints through system queues at all.
    available: bool,
    /// Whether the platform's own print dialog is the way to the driver.
    system_dialog: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Sent {
    /// "job 12", "request Office-31", or "dry run".
    label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Thumb {
    index: usize,
    /// A small JPEG as a data: URL.
    src: String,
    width: u32,
    height: u32,
    dpi: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
enum Progress {
    Print(kagaz_core::print::Event),
    Scan(kagaz_core::scan::Event),
    Note { text: String },
}

fn emit(app: &AppHandle, channel: &str, p: Progress) {
    let _ = app.emit(channel, &p);
}

#[tauri::command]
pub async fn printer_status(device: Device) -> Result<PrinterStatus, String> {
    blocking(move || kagaz_core::ipp::status::fetch(&device).map_err(|e| e.to_string())).await
}

#[tauri::command]
pub async fn print_queues(device: Option<Device>) -> Result<Queues, String> {
    blocking(move || {
        let queues = system::queues().map_err(|e| e.to_string())?;
        let matched = device
            .as_ref()
            .and_then(|d| system::queue_for(&queues, d))
            .map(|q| q.name.clone());
        Ok(Queues {
            queues,
            matched,
            available: system::available(),
            system_dialog: cfg!(mobile),
        })
    })
    .await
}

/// Ask for a document to print; `None` when the person cancels.
#[tauri::command]
pub async fn choose_print_file(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    blocking(move || {
        let picked = app
            .dialog()
            .file()
            .add_filter("Documents and pictures", &["pdf", "jpg", "jpeg", "png"])
            .blocking_pick_file();
        Ok(picked.map(|p| p.to_string()))
    })
    .await
}

#[tauri::command]
pub async fn print_file(
    app: AppHandle,
    target: Target,
    path: String,
    request: PrintRequest,
    dry_run: bool,
) -> Result<Sent, String> {
    blocking(move || {
        let path = PathBuf::from(path);
        match target {
            Target::Driverless { device } => {
                let result =
                    kagaz_core::print::print(&device, &path, &request, dry_run, &mut |e| {
                        emit(&app, "print-progress", Progress::Print(e))
                    })
                    .map_err(|e| e.to_string())?;
                Ok(Sent {
                    label: result
                        .map(|(id, state)| format!("job {id}, {state}"))
                        .unwrap_or_else(|| "dry run: the printer accepts these settings".into()),
                })
            }
            Target::Queue { name } => {
                if dry_run {
                    return Ok(Sent {
                        label: "dry run: nothing sent to the queue".into(),
                    });
                }
                emit(
                    &app,
                    "print-progress",
                    Progress::Note {
                        text: format!("Handing it to the \"{name}\" queue…"),
                    },
                );
                let job = system::print_file(&name, &path, &request).map_err(|e| e.to_string())?;
                Ok(Sent {
                    label: format!("{name}, request {job}"),
                })
            }
            Target::System => mobile::print_document(&app, &path, &request),
        }
    })
    .await
}

/// A thumbnail of a scanned page, as a data: URL.
fn thumbnail(page: &Page) -> Result<(String, u32, u32), String> {
    let img = image::load_from_memory(&page.data).map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    let small = img.thumbnail(360, 360).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75)
        .encode_image(&small)
        .map_err(|e| e.to_string())?;
    Ok((
        format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(jpeg)
        ),
        w,
        h,
    ))
}

/// Scan the originals for a copy; the pages stay here until printed.
#[tauri::command]
pub async fn copy_scan(
    app: AppHandle,
    state: State<'_, CopyState>,
    device: Device,
    request: ScanRequest,
    keep: bool,
) -> Result<Vec<Thumb>, String> {
    let scanned = blocking(move || {
        kagaz_core::scan::scan(&device, &request, &mut |e| {
            emit(&app, "copy-progress", Progress::Scan(e))
        })
        .map_err(|e| e.to_string())
    })
    .await?;
    let mut pages = state.pages.lock().unwrap();
    if !keep {
        pages.clear();
    }
    pages.extend(scanned);
    pages
        .iter()
        .enumerate()
        .map(|(index, p)| {
            let (src, width, height) = thumbnail(p)?;
            Ok(Thumb {
                index,
                src,
                width,
                height,
                dpi: p.dpi,
            })
        })
        .collect()
}

/// Print the chosen scanned pages, in the order given.
#[tauri::command]
pub async fn copy_print(
    app: AppHandle,
    state: State<'_, CopyState>,
    target: Target,
    pages: Vec<usize>,
    request: PrintRequest,
    fit: Fit,
) -> Result<Sent, String> {
    let chosen: Vec<Page> = {
        let all = state.pages.lock().unwrap();
        pages
            .iter()
            .map(|&i| {
                all.get(i)
                    .cloned()
                    .ok_or_else(|| format!("page {} is no longer here; scan again", i + 1))
            })
            .collect::<Result<_, _>>()?
    };
    blocking(move || match target {
        Target::Driverless { device } => {
            let result =
                kagaz_core::print::print_scans(&device, &chosen, &request, fit, false, &mut |e| {
                    emit(&app, "copy-progress", Progress::Print(e))
                })
                .map_err(|e| e.to_string())?;
            Ok(Sent {
                label: result
                    .map(|(id, state)| format!("job {id}, {state}"))
                    .unwrap_or_default(),
            })
        }
        Target::Queue { name } => {
            emit(
                &app,
                "copy-progress",
                Progress::Note {
                    text: format!("Handing the pages to the \"{name}\" queue…"),
                },
            );
            let job =
                system::print_scans(&name, &chosen, &request, fit).map_err(|e| e.to_string())?;
            Ok(Sent {
                label: format!("{name}, request {job}"),
            })
        }
        Target::System => mobile::print_scans(&app, &chosen, &request, fit),
    })
    .await
}

#[tauri::command]
pub fn copy_discard(state: State<'_, CopyState>) {
    state.pages.lock().unwrap().clear();
}

/// The phone's print dialog: the document becomes a PDF that Android's
/// print system takes, with whatever print service (vendor plugin) the
/// phone has installed.
#[cfg(not(mobile))]
mod mobile {
    use super::*;

    fn none() -> Result<Sent, String> {
        Err("this computer prints through its own queues, not a print dialog".into())
    }

    pub fn print_document(
        _: &AppHandle,
        _: &std::path::Path,
        _: &PrintRequest,
    ) -> Result<Sent, String> {
        none()
    }

    pub fn print_scans(
        _: &AppHandle,
        _: &[Page],
        _: &PrintRequest,
        _: Fit,
    ) -> Result<Sent, String> {
        none()
    }
}

#[cfg(mobile)]
mod mobile {
    pub use crate::mobile::{print_document, print_scans};
}
