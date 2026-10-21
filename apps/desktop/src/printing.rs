//! Print and Copy for the device page. Printing goes either straight to the
//! printer without a driver (IPP Everywhere) or through one of the system's
//! print queues, and so through whatever driver is installed. A copy is a
//! scan whose pages are kept here, shown as thumbnails, then printed.

use super::blocking;
use base64::Engine as _;
use kagaz_core::ipp::status::PrinterStatus;
use kagaz_core::print::system::{self, Queue};
use kagaz_core::print::{Fit, PrintRequest};
use kagaz_core::scan::{Page, Paper, ScanRequest, Source};
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

impl Sent {
    #[cfg_attr(not(mobile), allow(dead_code))]
    pub fn new(label: String) -> Self {
        Sent { label }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Thumb {
    index: usize,
    /// A small JPEG as a data: URL.
    src: String,
    width: u32,
    height: u32,
    dpi: u32,
    /// Anything to tell about it (an ID's card not found).
    #[serde(skip_serializing_if = "String::is_empty")]
    note: String,
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

/// Keep a document the page read from a file picker (a phone has no file
/// paths to hand over); returns where it was put. `data` is the file in
/// base64: plain text crosses into the app the same way on every platform,
/// where raw bytes arrive differently on Android.
#[tauri::command]
pub async fn upload_document(ext: String, data: String) -> Result<String, String> {
    blocking(move || {
        let ext = ext.to_ascii_lowercase();
        if !["pdf", "jpg", "jpeg", "png"].contains(&ext.as_str()) {
            return Err("Kagaz prints PDF, JPEG and PNG files".into());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .map_err(|e| format!("the file arrived damaged ({e})"))?;
        if bytes.is_empty() {
            return Err("the file is empty".into());
        }
        let dir = kagaz_core::paths::cache_dir().join("uploads");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("document.{ext}"));
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path.display().to_string())
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
    shrunk(page, 360, 75)
}

/// The thumbnail and the full-size view of a page, from one decoding: a
/// 1200 dpi scan takes a while to open, so it is opened once.
fn thumb_and_view(page: &Page) -> Result<((String, u32, u32), String), String> {
    let img = image::load_from_memory(&page.data).map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    let view = img.thumbnail(2000, 2000);
    let thumb = view.thumbnail(360, 360);
    let url = |img: &image::DynamicImage, quality| -> Result<String, String> {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
            .encode_image(&img.to_rgb8())
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(jpeg)
        ))
    };
    Ok(((url(&thumb, 75)?, w, h), url(&view, 85)?))
}

/// The scan under way, so Cancel can stop it.
static SCANNING: Mutex<Option<kagaz_core::scan::Cancel>> = Mutex::new(None);

/// Give `request` a way to be stopped, as the scan under way.
pub(crate) fn cancellable(request: &mut ScanRequest) {
    let cancel = kagaz_core::scan::Cancel::default();
    request.cancel = cancel.clone();
    *SCANNING.lock().unwrap() = Some(cancel);
}

/// Stop the scan under way: the scanner is told to stop.
#[tauri::command]
pub fn scan_cancel() {
    if let Some(cancel) = SCANNING.lock().unwrap().take() {
        cancel.cancel();
    }
}

/// The page as a JPEG data: URL at most `max` pixels on its long side, and
/// the page's own size. A scan at 1200 dpi is far too big for the window.
fn shrunk(page: &Page, max: u32, quality: u8) -> Result<(String, u32, u32), String> {
    let img = image::load_from_memory(&page.data).map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    let small = if w.max(h) > max { img.thumbnail(max, max) } else { img }.to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
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
    mut request: ScanRequest,
    keep: bool,
) -> Result<Vec<Thumb>, String> {
    cancellable(&mut request);
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
                note: String::new(),
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

/// Pages made elsewhere (the workbench: an ID page) become the copy's.
pub fn set_copy_pages(app: &AppHandle, pages: Vec<Page>) {
    use tauri::Manager;
    *app.state::<CopyState>().pages.lock().unwrap() = pages;
}

#[tauri::command]
pub fn copy_discard(state: State<'_, CopyState>) {
    state.pages.lock().unwrap().clear();
}

// ---------- ID scan and ID copy ----------
//
// Both sides of an ID card or a passport page on one page, as a printer's
// "2 in 1 ID copy" makes it: the front is a scan of the top half of the
// glass, the back (turned over, same place) another, and the page is the
// front's half above the back's (kagaz_core::scan::id). The Scan tab saves
// that page; the Copy tab prints it.

/// The two sides scanned so far, and the page made from them.
#[derive(Default)]
pub struct IdState {
    sides: Mutex<[Option<Page>; 2]>,
    /// Full-size views of the front and the back, made as each came, so
    /// opening one is instant.
    views: Mutex<[Option<String>; 2]>,
}

/// Scan one side (0 the front, 1 the back): the top half of the glass.
/// Scanning the front again starts over.
#[tauri::command]
pub async fn id_scan_side(
    app: AppHandle,
    state: State<'_, IdState>,
    device: Device,
    mut request: ScanRequest,
    side: usize,
    copy: bool,
) -> Result<Thumb, String> {
    if side > 1 {
        return Err("an ID has two sides".into());
    }
    request.source = Source::Glass;
    request.half_page = true;
    if request.paper == Paper::Max {
        request.paper = Paper::A4;
    }
    let channel = if copy { "copy-progress" } else { "scan-progress" };
    cancellable(&mut request);
    let page = blocking(move || {
        let pages = kagaz_core::scan::scan(&device, &request, &mut |e| {
            if copy {
                emit(&app, channel, Progress::Scan(e))
            } else {
                let _ = app.emit(channel, &e);
            }
        })
        .map_err(|e| e.to_string())?;
        pages
            .into_iter()
            .next()
            .ok_or_else(|| "the scanner sent no picture".to_string())
    })
    .await?;
    let (page, (src, width, height), view) = blocking(move || {
        let (thumb, view) = thumb_and_view(&page)?;
        Ok((page, thumb, view))
    })
    .await?;
    let dpi = page.dpi;
    let mut sides = state.sides.lock().unwrap();
    let mut views = state.views.lock().unwrap();
    if side == 0 {
        sides[1] = None;
        views[1] = None;
    }
    sides[side] = Some(page);
    views[side] = Some(view);
    Ok(Thumb { index: side, src, width, height, dpi, note: String::new() })
}

/// Put the two sides on one page, on the "id" workbench: Tidy, Crop,
/// Compress, then Save or Print, as for any scan (bench.rs).
#[tauri::command]
pub async fn id_page(app: AppHandle, state: State<'_, IdState>, paper: Paper) -> Result<super::bench::View, String> {
    let (front, back) = {
        let sides = state.sides.lock().unwrap();
        match (&sides[0], &sides[1]) {
            (Some(f), Some(b)) => (f.clone(), b.clone()),
            (None, _) => return Err("scan the front first".into()),
            (_, None) => return Err("scan the back first".into()),
        }
    };
    let paper = if paper == Paper::Max { Paper::A4 } else { paper };
    let picture = blocking(move || {
        let sheet = kagaz_core::scan::id::page(&front, &back, paper)?;
        kagaz_core::tools::from_page(&sheet)
    })
    .await?;
    super::bench::put(&app, "id", "id-card", vec![picture]);
    super::bench::bench_view(app, "id".into(), 0).await
}

/// A side (0 the front, 1 the back), big enough to look at closely.
#[tauri::command]
pub async fn id_picture(state: State<'_, IdState>, which: usize) -> Result<String, String> {
    if let Some(view) = state.views.lock().unwrap().get(which).cloned().flatten() {
        return Ok(view);
    }
    let page = state
        .sides
        .lock()
        .unwrap()
        .get(which)
        .cloned()
        .flatten()
        .ok_or_else(|| "nothing scanned there yet".to_string())?;
    blocking(move || shrunk(&page, 2000, 85).map(|(src, _, _)| src)).await
}

/// A copy's scanned page, big enough to look at closely.
#[tauri::command]
pub async fn copy_picture(state: State<'_, CopyState>, index: usize) -> Result<String, String> {
    let page = state
        .pages
        .lock()
        .unwrap()
        .get(index)
        .cloned()
        .ok_or_else(|| "that page is no longer here; scan again".to_string())?;
    blocking(move || shrunk(&page, 2000, 85).map(|(src, _, _)| src)).await
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
use super::mobile;
