//! Driverless printing over IPP: render the document to the printer's own
//! raster format (PWG Raster) at the printer's resolution and paper size,
//! then Print-Job. PDFs are rendered with hayro (pure Rust), images with
//! the image crate.

pub mod pwg;

use crate::ipp::{
    self, call, encode_request, out, out_int, urls_for, IppError, Value, GROUP_JOB, OP_PRINT_JOB,
    OP_VALIDATE_JOB, TAG_INTEGER, TAG_KEYWORD, TAG_MIME, TAG_NAME,
};
use crate::Device;
use image::{DynamicImage, GrayImage, RgbImage};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sides {
    OneSided,
    TwoSidedLongEdge,
    TwoSidedShortEdge,
}

impl Sides {
    pub fn keyword(self) -> &'static str {
        match self {
            Sides::OneSided => "one-sided",
            Sides::TwoSidedLongEdge => "two-sided-long-edge",
            Sides::TwoSidedShortEdge => "two-sided-short-edge",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PrintRequest {
    pub copies: u32,
    pub sides: Sides,
    /// IPP media keyword; the printer's default when None.
    pub media: Option<String>,
    /// Ask for colour when the printer can do it.
    pub color: bool,
    pub job_name: Option<String>,
}

impl Default for PrintRequest {
    fn default() -> Self {
        Self {
            copies: 1,
            sides: Sides::OneSided,
            media: None,
            color: true,
            job_name: None,
        }
    }
}

/// What the printer wants, read from its attributes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub dpi: u32,
    /// Send greyscale (sgray_8) rather than colour (srgb_8).
    pub gray: bool,
    pub media: String,
    pub width_pt: u32,
    pub height_pt: u32,
    pub sides: Sides,
    /// Back sides must be rotated 180 degrees (pwg-raster-document-sheet-back = rotated).
    pub sheet_back_rotated: bool,
    pub copies: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Planned(Plan),
    Rendering { page: usize, of: usize },
    Validated,
    Sending { bytes: usize },
    Submitted { job_id: i32, state: String },
}

#[derive(Debug, thiserror::Error)]
pub enum PrintError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0}")]
    Unsupported(String),
    #[error("cannot render the document: {0}")]
    Render(String),
    #[error(transparent)]
    Ipp(#[from] IppError),
}

/// A rendered page, 8-bit grey or RGB, exactly the paper size in pixels.
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    pub components: u32,
    pub pixels: Vec<u8>,
}

/// Ask the printer what it needs and reconcile that with the request.
pub fn plan(device: &Device, req: &PrintRequest) -> Result<Plan, PrintError> {
    let (http, uri) = urls_for(device)?;
    let request = encode_request(
        ipp::OP_GET_PRINTER_ATTRIBUTES,
        1,
        &uri,
        &[out(
            TAG_KEYWORD,
            "requested-attributes",
            &[
                "document-format-supported",
                "pwg-raster-document-resolution-supported",
                "pwg-raster-document-type-supported",
                "pwg-raster-document-sheet-back",
                "media-default",
                "media-col-default",
                "media-supported",
                "media-col-database",
                "media-size-supported",
                "sides-supported",
                "copies-supported",
                "print-color-mode-supported",
                "color-supported",
            ],
        )],
        &[],
        &[],
    );
    let attrs = call(&http, &request, Duration::from_secs(15))?;
    if !attrs.ok() {
        return Err(IppError::Status(ipp::status_text(attrs.status)).into());
    }
    plan_from_attributes(&attrs, req)
}

/// The planning logic on its own, so it can be tested on recordings.
pub fn plan_from_attributes(attrs: &ipp::Response, req: &PrintRequest) -> Result<Plan, PrintError> {
    let strings = |name: &str| attrs.printer(name).map(|a| a.strings()).unwrap_or_default();
    let formats = strings("document-format-supported");
    if !formats.iter().any(|f| f == "image/pwg-raster") {
        return Err(PrintError::Unsupported(format!(
            "the printer does not accept PWG Raster (it accepts: {}); only IPP Everywhere printers are supported so far",
            formats.join(", ")
        )));
    }
    let dpi = attrs
        .printer("pwg-raster-document-resolution-supported")
        .and_then(|a| {
            a.values
                .iter()
                .filter_map(|v| match v {
                    Value::Resolution { x, .. } => Some(*x as u32),
                    _ => None,
                })
                .filter(|r| *r >= 150)
                .min()
                .or_else(|| {
                    a.values.iter().find_map(|v| match v {
                        Value::Resolution { x, .. } => Some(*x as u32),
                        _ => None,
                    })
                })
        })
        .unwrap_or(300);
    let types = strings("pwg-raster-document-type-supported");
    let color_ok = types.iter().any(|t| t == "srgb_8");
    let gray_ok = types.iter().any(|t| t == "sgray_8");
    let gray = !(req.color && color_ok) && (gray_ok || !color_ok);
    let sheet_back_rotated = strings("pwg-raster-document-sheet-back")
        .first()
        .map(|s| s == "rotated")
        .unwrap_or(false);

    let media = req
        .media
        .clone()
        .or_else(|| strings("media-default").into_iter().next())
        .unwrap_or_else(|| "iso_a4_210x297mm".to_string());
    if let Some(a) = attrs.printer("media-supported") {
        if !a.strings().contains(&media) {
            return Err(PrintError::Unsupported(format!(
                "the printer does not take {} paper; it takes: {}",
                ipp::status::media_name(&media),
                a.strings()
                    .iter()
                    .filter(|m| !m.starts_with("custom_"))
                    .map(|m| ipp::status::media_name(m))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    let (width_pt, height_pt) = media_points(&media).ok_or_else(|| {
        PrintError::Unsupported(format!("cannot work out the size of media \"{media}\""))
    })?;

    let sides_supported = strings("sides-supported");
    let sides =
        if sides_supported.is_empty() || sides_supported.iter().any(|s| s == req.sides.keyword()) {
            req.sides
        } else {
            return Err(PrintError::Unsupported(format!(
                "the printer cannot print {}; it offers: {}",
                req.sides.keyword(),
                sides_supported.join(", ")
            )));
        };
    let copies = match attrs.printer("copies-supported").and_then(|a| a.first()) {
        Some(Value::Range(lo, hi)) => req.copies.clamp(*lo as u32, *hi as u32),
        _ => req.copies.max(1),
    };
    Ok(Plan {
        dpi,
        gray,
        media,
        width_pt,
        height_pt,
        sides,
        sheet_back_rotated,
        copies,
    })
}

/// Paper size in points from a PWG media keyword, which ends in
/// `<width>x<height><mm|in>`.
pub fn media_points(keyword: &str) -> Option<(u32, u32)> {
    let dims = keyword.rsplit('_').next()?;
    let (nums, unit) = if let Some(n) = dims.strip_suffix("mm") {
        (n, 72.0 / 25.4)
    } else {
        (dims.strip_suffix("in")?, 72.0)
    };
    let (w, h) = nums.split_once('x')?;
    let w: f64 = w.parse().ok()?;
    let h: f64 = h.parse().ok()?;
    Some(((w * unit).round() as u32, (h * unit).round() as u32))
}

/// Render `path` (PDF, JPEG or PNG) into pages of the planned paper size.
pub fn render_file(
    path: &Path,
    plan: &Plan,
    on_event: &mut dyn FnMut(Event),
) -> Result<Vec<RenderedPage>, PrintError> {
    let data = std::fs::read(path).map_err(|source| PrintError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let pages = if data.starts_with(b"%PDF") {
        render_pdf(data, plan, on_event)?
    } else if image::guess_format(&data).is_ok() {
        on_event(Event::Rendering { page: 1, of: 1 });
        let img = image::load_from_memory(&data).map_err(|e| PrintError::Render(e.to_string()))?;
        vec![fit_to_page(img, plan)]
    } else {
        return Err(PrintError::Unsupported(format!(
            "{} is not a PDF, JPEG or PNG; those are the formats Kagaz can print today",
            path.display()
        )));
    };
    Ok(pages)
}

fn render_pdf(
    data: Vec<u8>,
    plan: &Plan,
    on_event: &mut dyn FnMut(Event),
) -> Result<Vec<RenderedPage>, PrintError> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings};

    let pdf = Pdf::new(data).map_err(|e| PrintError::Render(format!("{e:?}")))?;
    let pages = pdf.pages();
    let total = pages.len();
    if total == 0 {
        return Err(PrintError::Render("the PDF has no pages".into()));
    }
    let cache = RenderCache::new();
    let settings = InterpreterSettings::default();
    let mut out = Vec::with_capacity(total);
    for (i, page) in pages.iter().enumerate() {
        on_event(Event::Rendering {
            page: i + 1,
            of: total,
        });
        let (w_pt, h_pt) = page.render_dimensions();
        // Landscape pages go on the paper sideways.
        let landscape = w_pt > h_pt && plan.width_pt < plan.height_pt;
        let (target_w, target_h) = if landscape {
            (plan.height_pt as f32, plan.width_pt as f32)
        } else {
            (plan.width_pt as f32, plan.height_pt as f32)
        };
        let fit = (target_w / w_pt).min(target_h / h_pt);
        let scale = fit * plan.dpi as f32 / 72.0;
        let pixmap = hayro::render(
            page,
            &cache,
            &settings,
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            },
        );
        let (pw, ph) = (pixmap.width() as u32, pixmap.height() as u32);
        let rgba = pixmap.data_as_u8_slice();
        let rgb: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let img = RgbImage::from_raw(pw, ph, rgb)
            .map(DynamicImage::ImageRgb8)
            .ok_or_else(|| PrintError::Render("bad pixmap size".into()))?;
        let img = if landscape { img.rotate90() } else { img };
        out.push(place(img, plan));
    }
    Ok(out)
}

/// Scale an image to fit the paper (rotating it when that fits better) and
/// centre it.
fn fit_to_page(img: DynamicImage, plan: &Plan) -> RenderedPage {
    let page_w = plan.width_pt as f32 * plan.dpi as f32 / 72.0;
    let page_h = plan.height_pt as f32 * plan.dpi as f32 / 72.0;
    let (w, h) = (img.width() as f32, img.height() as f32);
    let img = if (w > h) != (page_w > page_h) {
        img.rotate90()
    } else {
        img
    };
    let (w, h) = (img.width() as f32, img.height() as f32);
    let scale = (page_w / w).min(page_h / h);
    let img = img.resize_exact(
        ((w * scale).round() as u32).max(1),
        ((h * scale).round() as u32).max(1),
        image::imageops::FilterType::Triangle,
    );
    place(img, plan)
}

/// Put an already-scaled image in the middle of a white page.
fn place(img: DynamicImage, plan: &Plan) -> RenderedPage {
    let width = (plan.width_pt as f32 * plan.dpi as f32 / 72.0).round() as u32;
    let height = (plan.height_pt as f32 * plan.dpi as f32 / 72.0).round() as u32;
    let x0 = width.saturating_sub(img.width()) / 2;
    let y0 = height.saturating_sub(img.height()) / 2;
    if plan.gray {
        let src = img.to_luma8();
        let mut page = GrayImage::from_pixel(width, height, image::Luma([255]));
        for (x, y, p) in src.enumerate_pixels() {
            if x + x0 < width && y + y0 < height {
                page.put_pixel(x + x0, y + y0, *p);
            }
        }
        RenderedPage {
            width,
            height,
            components: 1,
            pixels: page.into_raw(),
        }
    } else {
        let src = img.to_rgb8();
        let mut page = RgbImage::from_pixel(width, height, image::Rgb([255, 255, 255]));
        for (x, y, p) in src.enumerate_pixels() {
            if x + x0 < width && y + y0 < height {
                page.put_pixel(x + x0, y + y0, *p);
            }
        }
        RenderedPage {
            width,
            height,
            components: 3,
            pixels: page.into_raw(),
        }
    }
}

/// Rotate a page 180 degrees in place (for printers whose duplex back side is "rotated").
fn rotate_180(page: &mut RenderedPage) {
    let bpp = page.components as usize;
    let n = page.pixels.len() / bpp;
    for i in 0..n / 2 {
        let j = n - 1 - i;
        for c in 0..bpp {
            page.pixels.swap(i * bpp + c, j * bpp + c);
        }
    }
}

/// Encode rendered pages as one PWG Raster document.
pub fn to_pwg(pages: &mut [RenderedPage], plan: &Plan) -> Vec<u8> {
    let duplex = plan.sides != Sides::OneSided;
    if duplex && plan.sheet_back_rotated {
        for p in pages.iter_mut().skip(1).step_by(2) {
            rotate_180(p);
        }
    }
    let raster: Vec<pwg::RasterPage> = pages
        .iter()
        .map(|p| pwg::RasterPage {
            width: p.width,
            height: p.height,
            dpi: plan.dpi,
            components: p.components,
            pixels: &p.pixels,
            width_pt: plan.width_pt,
            height_pt: plan.height_pt,
            media: &plan.media,
            duplex,
            tumble: plan.sides == Sides::TwoSidedShortEdge,
        })
        .collect();
    pwg::encode(&raster)
}

fn user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "kagaz".to_string())
}

fn job_attributes(plan: &Plan) -> Vec<ipp::OutAttribute> {
    let mut v = vec![
        out_int(TAG_INTEGER, "copies", plan.copies as i32),
        out(TAG_KEYWORD, "sides", &[plan.sides.keyword()]),
        out(TAG_KEYWORD, "media", &[&plan.media]),
    ];
    v.push(out(
        TAG_KEYWORD,
        "print-color-mode",
        &[if plan.gray { "monochrome" } else { "color" }],
    ));
    v
}

fn operation_attributes(job_name: &str) -> Vec<ipp::OutAttribute> {
    vec![
        out(TAG_NAME, "requesting-user-name", &[&user_name()]),
        out(TAG_NAME, "job-name", &[job_name]),
        out(TAG_MIME, "document-format", &["image/pwg-raster"]),
    ]
}

fn refusal(r: &ipp::Response) -> IppError {
    IppError::Status(
        r.status_message()
            .map(|m| format!("{m} ({})", ipp::status_text(r.status)))
            .unwrap_or_else(|| ipp::status_text(r.status)),
    )
}

/// Validate-Job: would the printer take this job? Sends no document.
pub fn validate(device: &Device, plan: &Plan, job_name: &str) -> Result<(), PrintError> {
    let (http, uri) = urls_for(device)?;
    let request = encode_request(
        OP_VALIDATE_JOB,
        2,
        &uri,
        &operation_attributes(job_name),
        &job_attributes(plan),
        &[],
    );
    let r = call(&http, &request, Duration::from_secs(15))?;
    if r.ok() {
        Ok(())
    } else {
        Err(refusal(&r).into())
    }
}

/// Print-Job with the PWG Raster `document`. Returns the job id and state.
pub fn submit(
    device: &Device,
    plan: &Plan,
    job_name: &str,
    document: &[u8],
) -> Result<(i32, String), PrintError> {
    let (http, uri) = urls_for(device)?;
    let request = encode_request(
        OP_PRINT_JOB,
        3,
        &uri,
        &operation_attributes(job_name),
        &job_attributes(plan),
        document,
    );
    let r = call(&http, &request, Duration::from_secs(600))?;
    if !r.ok() {
        return Err(refusal(&r).into());
    }
    let job = r.group(GROUP_JOB);
    let id = job
        .and_then(|g| g.get("job-id"))
        .and_then(|a| a.first())
        .and_then(Value::as_i32)
        .unwrap_or(0);
    let state = job
        .and_then(|g| g.get("job-state"))
        .and_then(|a| a.first())
        .and_then(Value::as_i32);
    let state = match state {
        Some(3) => "pending",
        Some(4) => "held",
        Some(5) => "printing",
        Some(6) => "stopped",
        Some(7) => "canceled",
        Some(8) => "aborted",
        Some(9) => "completed",
        _ => "accepted",
    }
    .to_string();
    Ok((id, state))
}

/// The whole thing: plan, render, validate, and (unless `dry_run`) print.
pub fn print(
    device: &Device,
    path: &Path,
    req: &PrintRequest,
    dry_run: bool,
    on_event: &mut dyn FnMut(Event),
) -> Result<Option<(i32, String)>, PrintError> {
    let plan = plan(device, req)?;
    on_event(Event::Planned(plan.clone()));
    let job_name = req.job_name.clone().unwrap_or_else(|| {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Kagaz print")
            .to_string()
    });
    let mut pages = render_file(path, &plan, on_event)?;
    validate(device, &plan, &job_name)?;
    on_event(Event::Validated);
    if dry_run {
        return Ok(None);
    }
    let document = to_pwg(&mut pages, &plan);
    on_event(Event::Sending {
        bytes: document.len(),
    });
    let result = submit(device, &plan, &job_name, &document)?;
    on_event(Event::Submitted {
        job_id: result.0,
        state: result.1.clone(),
    });
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipp::decode_response;

    const ATTRS: &[u8] = include_bytes!(
        "../../tests/fixtures/ipp/brother-dcp-l2540dw.get-printer-attributes.response.bin"
    );
    const TEST_PDF: &[u8] = include_bytes!("../../tests/fixtures/print/test-page.pdf");

    fn a4_plan(dpi: u32, gray: bool) -> Plan {
        Plan {
            dpi,
            gray,
            media: "iso_a4_210x297mm".into(),
            width_pt: 595,
            height_pt: 842,
            sides: Sides::OneSided,
            sheet_back_rotated: true,
            copies: 1,
        }
    }

    #[test]
    fn plans_from_the_brother_attributes() {
        let attrs = decode_response(ATTRS).unwrap();
        let p = plan_from_attributes(&attrs, &PrintRequest::default()).unwrap();
        assert_eq!(p.dpi, 300);
        assert!(p.gray); // the Brother only takes sgray_8
        assert_eq!(p.media, "iso_a4_210x297mm");
        assert_eq!((p.width_pt, p.height_pt), (595, 842));
        assert!(p.sheet_back_rotated);
        assert_eq!(p.copies, 1);

        let req = PrintRequest {
            copies: 500,
            sides: Sides::TwoSidedLongEdge,
            media: Some("na_letter_8.5x11in".into()),
            ..Default::default()
        };
        let p = plan_from_attributes(&attrs, &req).unwrap();
        assert_eq!(p.copies, 99);
        assert_eq!((p.width_pt, p.height_pt), (612, 792));
        assert_eq!(p.sides, Sides::TwoSidedLongEdge);

        let req = PrintRequest {
            media: Some("iso_a3_297x420mm".into()),
            ..Default::default()
        };
        assert!(
            matches!(plan_from_attributes(&attrs, &req), Err(PrintError::Unsupported(m)) if m.contains("A3"))
        );
    }

    #[test]
    fn media_keywords_give_points() {
        assert_eq!(media_points("iso_a4_210x297mm"), Some((595, 842)));
        assert_eq!(media_points("na_letter_8.5x11in"), Some((612, 792)));
        assert_eq!(media_points("na_number-10_4.125x9.5in"), Some((297, 684)));
        assert_eq!(media_points("weird"), None);
    }

    #[test]
    fn renders_the_test_pdf_onto_a4() {
        let dir = std::env::temp_dir().join(format!("kagaz-print-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test-page.pdf");
        std::fs::write(&path, TEST_PDF).unwrap();
        let plan = a4_plan(72, true);
        let mut events = Vec::new();
        let pages = render_file(&path, &plan, &mut |e| events.push(e)).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(
            (pages[0].width, pages[0].height, pages[0].components),
            (595, 842, 1)
        );
        assert_eq!(events, vec![Event::Rendering { page: 1, of: 1 }]);
        let px = &pages[0].pixels;
        assert_eq!(px[0], 255, "top-left corner is paper");
        // The page has a black rectangle at 100..300 x 100..200 points from the top-left.
        assert!(px[150 * 595 + 200] < 50, "inside the rectangle is ink");
        let dark = px.iter().filter(|v| **v < 128).count();
        assert!(dark > 20_000 && dark < 60_000, "{dark} dark pixels");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn images_are_fitted_rotated_and_centred() {
        let dir = std::env::temp_dir().join(format!("kagaz-print-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        let img = RgbImage::from_pixel(400, 100, image::Rgb([0, 0, 0]));
        img.save(&path).unwrap();
        let plan = a4_plan(72, false);
        let pages = render_file(&path, &plan, &mut |_| {}).unwrap();
        let p = &pages[0];
        assert_eq!((p.width, p.height, p.components), (595, 842, 3));
        // Rotated to portrait and scaled to the full height: 842 tall, ~210 wide, centred.
        let row = |y: u32| &p.pixels[(y * 595 * 3) as usize..((y + 1) * 595 * 3) as usize];
        assert_eq!(row(0)[0], 255); // left margin is white
        assert_eq!(row(421)[297 * 3], 0); // middle is black
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn duplex_back_sides_are_rotated_when_the_printer_says_so() {
        let plan = Plan {
            sides: Sides::TwoSidedLongEdge,
            ..a4_plan(1, true)
        };
        let mut pages = vec![
            RenderedPage {
                width: 2,
                height: 1,
                components: 1,
                pixels: vec![0, 255],
            },
            RenderedPage {
                width: 2,
                height: 1,
                components: 1,
                pixels: vec![0, 255],
            },
        ];
        let doc = to_pwg(&mut pages, &plan);
        assert_eq!(pages[0].pixels, vec![0, 255]);
        assert_eq!(pages[1].pixels, vec![255, 0]);
        assert!(doc.len() > 2 * pwg::HEADER_LEN);
    }
}
