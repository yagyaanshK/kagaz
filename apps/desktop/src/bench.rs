//! The workbench: the pictures a tool works on, wherever they came from (a
//! scan, an ID page, a file on this device, later a camera's photo), kept
//! per place ("scan", "id", "tools", ...) so each page keeps its own. The
//! tools themselves are kagaz_core::tools; the window's one panel for them
//! is ui/bench.js. A new source only puts its pictures here ([`put`]).
//!
//! Each tool changes the current pictures; Undo goes back one step, Start
//! over to the pictures as they came. Compress makes the files (and says
//! their size) without changing the pictures; Save writes them.

use super::blocking;
use base64::Engine as _;
use kagaz_core::output::{self, Encoded, Format};
use kagaz_core::scan::id::{Shape, Tidy};
use kagaz_core::scan::{ColorMode, Paper};
use kagaz_core::tools::{self, CropMode, Picture};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

struct Bench {
    /// What it is called (a file's name, "scan"), for the saved file's name.
    name: String,
    original: Vec<Picture>,
    current: Vec<Picture>,
    /// Before the last tool, for Undo; and for frames adjusted after Tidy.
    before: Option<Vec<Picture>>,
    /// Files Compress made, with their format.
    packed: Option<(Format, Vec<Encoded>)>,
}

#[derive(Default)]
pub struct Benches(Mutex<HashMap<String, Bench>>);

/// Put pictures on a bench, in place of what was there.
pub fn put(app: &AppHandle, bench: &str, name: &str, pictures: Vec<Picture>) {
    app.state::<Benches>().0.lock().unwrap().insert(
        bench.to_string(),
        Bench { name: name.to_string(), original: pictures.clone(), current: pictures, before: None, packed: None },
    );
}

/// The current pictures as pages (to print a copy, say).
pub fn pages(app: &AppHandle, bench: &str) -> Result<Vec<kagaz_core::scan::Page>, String> {
    let current = with(app.state::<Benches>().inner(), bench, |b| Ok(b.current.clone()))?;
    current.iter().map(tools::to_page).collect()
}

fn with<T>(benches: &Benches, bench: &str, f: impl FnOnce(&mut Bench) -> Result<T, String>) -> Result<T, String> {
    let mut all = benches.0.lock().unwrap();
    let b = all.get_mut(bench).ok_or_else(|| "nothing to work on yet".to_string())?;
    f(b)
}

/// One picture shown: a page, small and full size (data: URLs).
#[derive(Debug, Clone, Serialize)]
pub struct View {
    name: String,
    pages: usize,
    page: usize,
    thumb: String,
    view: String,
    /// The page as it was before the last tool (for frames after Tidy).
    before: Option<String>,
    /// A card's rounded corner, as a fraction of the page's width.
    radius: f32,
    /// Frames to start on: Tidy's cards (Find the card), Crop's area.
    frames: Vec<Frame>,
    /// After Compress: the files' size, and why it is not under the limit.
    size: Option<u64>,
    warning: Option<String>,
    notes: Vec<String>,
    can_undo: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Frame {
    quad: Vec<[f32; 2]>,
    found: bool,
}

fn url(img: &image::RgbImage, max: u32, quality: u8) -> Result<String, String> {
    let small = image::DynamicImage::ImageRgb8(img.clone()).thumbnail(max, max).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
        .encode_image(&small)
        .map_err(|e| e.to_string())?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpeg)))
}

fn view(b: &Bench, page: usize, frames: Vec<Frame>, notes: Vec<String>, with_before: bool) -> Result<View, String> {
    let page = page.min(b.current.len().saturating_sub(1));
    let p = b.current.get(page).ok_or("no pages")?;
    let before = match (with_before, &b.before) {
        (true, Some(before)) => before.get(page).map(|q| url(&q.img, 2000, 85)).transpose()?,
        _ => None,
    };
    let (size, warning) = match &b.packed {
        Some((_, files)) => (
            Some(files.iter().map(|f| f.bytes.len() as u64).sum()),
            files.iter().find_map(|f| f.warning.clone()),
        ),
        None => (None, None),
    };
    Ok(View {
        name: b.name.clone(),
        pages: b.current.len(),
        page,
        thumb: url(&p.img, 360, 75)?,
        view: url(&p.img, 2000, 85)?,
        before,
        radius: kagaz_core::scan::id::corner_radius(p.dpi) / p.img.width().max(1) as f32,
        frames,
        size,
        warning,
        notes,
        can_undo: b.before.is_some(),
    })
}

/// Run a tool on the bench's pictures (off the window's thread).
async fn work<T: Send + 'static>(
    app: AppHandle,
    bench: String,
    f: impl FnOnce(&mut Bench) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    blocking(move || with(app.state::<Benches>().inner(), &bench, f)).await
}

/// Open a file (PDF, JPEG, PNG) on a bench.
#[tauri::command]
pub async fn bench_open_file(app: AppHandle, bench: String, path: String, name: String) -> Result<View, String> {
    let app2 = app.clone();
    let pictures = blocking(move || tools::open(std::path::Path::new(&path))).await?;
    if pictures.is_empty() {
        return Err("the file has no pages".into());
    }
    put(&app2, &bench, &name, pictures);
    work(app, bench, |b| view(b, 0, Vec::new(), Vec::new(), false)).await
}

/// A page of what is on the bench.
#[tauri::command]
pub async fn bench_view(app: AppHandle, bench: String, page: usize) -> Result<View, String> {
    work(app, bench, move |b| view(b, page, Vec::new(), Vec::new(), false)).await
}

fn step(b: &mut Bench) {
    b.before = Some(b.current.clone());
    b.packed = None;
}

/// Tidy every page (Clean background, Find the card). With Find the card,
/// the frames it found on `page` come back to adjust (see bench_frames).
#[tauri::command]
pub async fn bench_tidy(app: AppHandle, bench: String, page: usize, tidy: Tidy) -> Result<View, String> {
    work(app, bench, move |b| {
        step(b);
        let frames = if tidy.card {
            let p = b.current.get(page).ok_or("no such page")?;
            tools::find_frames(p).into_iter().map(|(q, found)| Frame { quad: q.to_vec(), found }).collect()
        } else {
            Vec::new()
        };
        let mut notes = Vec::new();
        for p in &mut b.current {
            notes.extend(tools::tidy(p, tidy));
        }
        view(b, page, frames, notes, tidy.card)
    })
    .await
}

/// The person's frames on `page`, applied to it as it was before Tidy.
#[tauri::command]
pub async fn bench_frames(
    app: AppHandle,
    bench: String,
    page: usize,
    shapes: Vec<Shape>,
    background: bool,
) -> Result<View, String> {
    work(app, bench, move |b| {
        let mut p = match &b.before {
            Some(before) => before.get(page).cloned(),
            None => b.current.get(page).cloned(),
        }
        .ok_or("no such page")?;
        tools::frames(&mut p, &shapes, background);
        b.current[page] = p;
        b.packed = None;
        view(b, page, Vec::new(), Vec::new(), false)
    })
    .await
}

/// Crop's frame to start on: around what is on `page`.
#[tauri::command]
pub async fn bench_crop_start(app: AppHandle, bench: String, page: usize) -> Result<View, String> {
    work(app, bench, move |b| {
        let p = b.current.get(page).ok_or("no such page")?;
        let (quad, found) = tools::find_area(p);
        view(b, page, vec![Frame { quad, found }], Vec::new(), false)
    })
    .await
}

/// Crop `page` to the frame; `size_mm` makes it exactly that size (a
/// passport photo's 35 x 45 mm).
#[tauri::command]
pub async fn bench_crop(
    app: AppHandle,
    bench: String,
    page: usize,
    shape: Shape,
    mode: CropMode,
    paper: Option<Paper>,
    size_mm: Option<[f32; 2]>,
) -> Result<View, String> {
    work(app, bench, move |b| {
        step(b);
        let p = b.current.get(page).ok_or("no such page")?;
        b.current[page] = tools::crop(p, &shape, mode, paper.unwrap_or(Paper::A4), size_mm);
        view(b, page, Vec::new(), Vec::new(), false)
    })
    .await
}

/// Make the files of `format` under `max_bytes`; says their size.
#[tauri::command]
pub async fn bench_compress(app: AppHandle, bench: String, page: usize, format: Format, max_bytes: u64) -> Result<View, String> {
    work(app, bench, move |b| {
        let files = tools::compress(&b.current, format, max_bytes)?;
        b.packed = Some((format, files));
        view(b, page, Vec::new(), Vec::new(), false)
    })
    .await
}

/// Back one tool.
#[tauri::command]
pub async fn bench_undo(app: AppHandle, bench: String, page: usize) -> Result<View, String> {
    work(app, bench, move |b| {
        if let Some(before) = b.before.take() {
            b.current = before;
        }
        b.packed = None;
        view(b, page, Vec::new(), Vec::new(), false)
    })
    .await
}

/// Back to the pictures as they came.
#[tauri::command]
pub async fn bench_reset(app: AppHandle, bench: String, page: usize) -> Result<View, String> {
    work(app, bench, move |b| {
        b.current = b.original.clone();
        b.before = None;
        b.packed = None;
        view(b, page, Vec::new(), Vec::new(), false)
    })
    .await
}

/// Save at `out`, in its format: Compress's files when they are of that
/// format, else the pictures, kept under `max_bytes` if given.
#[tauri::command]
pub async fn bench_save(
    app: AppHandle,
    bench: String,
    out: String,
    max_bytes: Option<u64>,
) -> Result<Vec<super::WrittenFile>, String> {
    let app2 = app.clone();
    let format = std::path::Path::new(&out)
        .extension()
        .and_then(|e| Format::from_extension(&e.to_string_lossy()))
        .unwrap_or(Format::Pdf);
    let files = work(app, bench, move |b| match &b.packed {
        Some((f, files)) if *f == format && max_bytes.is_none() => Ok(files.clone()),
        _ => {
            let pages = b.current.iter().map(tools::to_page).collect::<Result<Vec<_>, _>>()?;
            output::encode_files(
                &pages,
                &output::OutputOptions { format, color: ColorMode::Color, quality: Some(92), max_bytes },
            )
            .map_err(|e| e.to_string())
        }
    })
    .await?;
    blocking(move || super::write_encoded(&app2, files, out)).await
}

/// The bench's pages become the copy's pages (Copy tab), ready to print.
#[tauri::command]
pub async fn bench_to_copy(app: AppHandle, bench: String) -> Result<(), String> {
    let app2 = app.clone();
    let pages = blocking(move || pages(&app2, &bench)).await?;
    super::printing::set_copy_pages(&app, pages);
    Ok(())
}

/// Scan straight onto the "scan" bench: look, use the tools, then save.
#[tauri::command]
pub async fn scan_to_bench(
    app: AppHandle,
    device: kagaz_core::Device,
    mut request: kagaz_core::scan::ScanRequest,
) -> Result<View, String> {
    super::printing::cancellable(&mut request);
    let app2 = app.clone();
    let pictures = blocking(move || {
        let pages = kagaz_core::scan::scan(&device, &request, &mut |event| {
            use tauri::Emitter;
            let _ = app2.emit("scan-progress", &event);
        })
        .map_err(|e| e.to_string())?;
        pages.iter().map(tools::from_page).collect::<Result<Vec<_>, _>>()
    })
    .await?;
    put(&app, "scan", "scan", pictures);
    work(app, "scan".into(), |b| view(b, 0, Vec::new(), Vec::new(), false)).await
}
