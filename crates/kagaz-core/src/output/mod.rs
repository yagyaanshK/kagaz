//! Turning scanned pages into files: PDF, JPEG or PNG, in the colour mode the
//! user asked for, optionally under a size limit. A JPEG page that already
//! matches goes into the PDF byte for byte; everything else is decoded and
//! re-encoded with the `image` crate.

pub mod jpeg;
pub mod pdf;

use crate::scan::{ColorMode, Page};
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Pdf,
    Jpeg,
    Png,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Jpeg => "jpg",
            Format::Png => "png",
        }
    }
    pub fn from_extension(ext: &str) -> Option<Format> {
        match ext.to_ascii_lowercase().as_str() {
            "pdf" => Some(Format::Pdf),
            "jpg" | "jpeg" => Some(Format::Jpeg),
            "png" => Some(Format::Png),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutputOptions {
    pub format: Format,
    pub color: ColorMode,
    /// JPEG quality 1-100 when a page has to be re-encoded (default 85).
    pub quality: Option<u8>,
    /// Keep each file at or under this many bytes, re-encoding and shrinking as needed.
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Written {
    pub path: PathBuf,
    pub bytes: u64,
    pub pages: usize,
    /// Set when the size limit could not be met.
    pub warning: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("cannot write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot read the scanned image: {0}")]
    Image(String),
    #[error("nothing was scanned")]
    NoPages,
}

/// Write `pages` to `path` (PDF: one file; JPEG/PNG: one file per page, with
/// `-1`, `-2`, ... before the extension when there are several).
pub fn write(
    pages: &[Page],
    opts: &OutputOptions,
    path: &Path,
) -> Result<Vec<Written>, OutputError> {
    if pages.is_empty() {
        return Err(OutputError::NoPages);
    }
    let mut written = Vec::new();
    match opts.format {
        Format::Pdf => {
            let (bytes, warning) = fit(opts, |q, s| {
                let rendered = pages
                    .iter()
                    .map(|p| render(p, opts.color, Format::Pdf, q, s))
                    .collect::<Result<Vec<_>, _>>()?;
                let pdf_pages: Vec<pdf::PdfPage> = rendered
                    .iter()
                    .map(|r| pdf::PdfPage {
                        width_px: r.width,
                        height_px: r.height,
                        dpi: r.dpi,
                        image: match &r.kind {
                            Kind::Jpeg { data, components } => pdf::Image::Jpeg {
                                data,
                                components: *components,
                            },
                            Kind::Bilevel { packed } => pdf::Image::Bilevel {
                                packed: packed.clone(),
                            },
                            Kind::Png { .. } => unreachable!("PDF pages are never rendered as PNG"),
                        },
                    })
                    .collect();
                Ok(pdf::write(&pdf_pages))
            })?;
            save(path, &bytes)?;
            written.push(Written {
                path: path.to_path_buf(),
                bytes: bytes.len() as u64,
                pages: pages.len(),
                warning,
            });
        }
        Format::Jpeg | Format::Png => {
            for (i, page) in pages.iter().enumerate() {
                let file = if pages.len() == 1 {
                    path.to_path_buf()
                } else {
                    numbered(path, i + 1)
                };
                let (bytes, warning) = fit(opts, |q, s| {
                    let r = render(page, opts.color, opts.format, q, s)?;
                    Ok(match r.kind {
                        Kind::Jpeg { data, .. } | Kind::Png { data } => data,
                        Kind::Bilevel { .. } => {
                            unreachable!("image files are never bilevel-packed")
                        }
                    })
                })?;
                save(&file, &bytes)?;
                written.push(Written {
                    path: file,
                    bytes: bytes.len() as u64,
                    pages: 1,
                    warning,
                });
            }
        }
    }
    Ok(written)
}

/// Render with the requested quality, then if a size limit is set and
/// missed, with lower quality and then smaller pages until it fits.
fn fit(
    opts: &OutputOptions,
    mut render_all: impl FnMut(Option<u8>, f32) -> Result<Vec<u8>, OutputError>,
) -> Result<(Vec<u8>, Option<String>), OutputError> {
    let first = render_all(opts.quality, 1.0)?;
    let Some(max) = opts.max_bytes else {
        return Ok((first, None));
    };
    if first.len() as u64 <= max {
        return Ok((first, None));
    }
    let steps: &[(u8, f32)] = &[
        (75, 1.0),
        (60, 1.0),
        (45, 1.0),
        (45, 0.8),
        (40, 0.65),
        (35, 0.5),
        (30, 0.4),
        (25, 0.3),
    ];
    let mut best = first;
    for (q, s) in steps {
        let q = match opts.quality {
            Some(user) => Some(user.min(*q)),
            None => Some(*q),
        };
        let attempt = render_all(q, *s)?;
        if attempt.len() < best.len() {
            best = attempt;
        }
        if best.len() as u64 <= max {
            return Ok((best, None));
        }
    }
    let warning = format!(
        "could not get under {}; the smallest version is {}",
        human_size(max),
        human_size(best.len() as u64)
    );
    Ok((best, Some(warning)))
}

enum Kind {
    Jpeg { data: Vec<u8>, components: u8 },
    Png { data: Vec<u8> },
    Bilevel { packed: Vec<u8> },
}

struct Rendered {
    kind: Kind,
    width: u32,
    height: u32,
    dpi: u32,
}

/// Does a page scanned as `have` already satisfy a request for `want`?
fn color_satisfied(have: ColorMode, want: ColorMode) -> bool {
    match want {
        ColorMode::Color => true,
        ColorMode::Gray => matches!(have, ColorMode::Gray | ColorMode::BlackWhite),
        ColorMode::BlackWhite => have == ColorMode::BlackWhite,
    }
}

fn render(
    page: &Page,
    color: ColorMode,
    format: Format,
    quality: Option<u8>,
    scale: f32,
) -> Result<Rendered, OutputError> {
    let untouched = page.mime == "image/jpeg"
        && format != Format::Png
        && quality.is_none()
        && scale >= 1.0
        && color_satisfied(page.color, color);
    if untouched {
        if let Some((width, height, components)) = jpeg::dimensions(&page.data) {
            if components == 1 || components == 3 {
                return Ok(Rendered {
                    kind: Kind::Jpeg {
                        data: page.data.clone(),
                        components,
                    },
                    width,
                    height,
                    dpi: page.dpi,
                });
            }
        }
    }

    let mut img =
        image::load_from_memory(&page.data).map_err(|e| OutputError::Image(e.to_string()))?;
    let mut dpi = page.dpi;
    if scale < 1.0 {
        let w = ((img.width() as f32) * scale).round().max(1.0) as u32;
        let h = ((img.height() as f32) * scale).round().max(1.0) as u32;
        img = img.resize_exact(w, h, FilterType::Triangle);
        dpi = ((page.dpi as f32) * scale).round().max(1.0) as u32;
    }
    let keep_gray = matches!(page.color, ColorMode::Gray | ColorMode::BlackWhite);
    let (width, height) = (img.width(), img.height());
    let quality = quality.unwrap_or(85).clamp(1, 100);

    let kind = match (color, format) {
        (ColorMode::BlackWhite, Format::Pdf) => Kind::Bilevel {
            packed: pack_bilevel(&img.to_luma8()),
        },
        (ColorMode::BlackWhite, f) => {
            let mut luma = img.to_luma8();
            let t = otsu_threshold(&luma);
            for p in luma.pixels_mut() {
                p.0[0] = if p.0[0] > t { 255 } else { 0 };
            }
            encode(DynamicImage::ImageLuma8(luma), f, quality)?
        }
        (ColorMode::Gray, f) => encode(DynamicImage::ImageLuma8(img.to_luma8()), f, quality)?,
        (ColorMode::Color, f) => {
            let img = if keep_gray {
                DynamicImage::ImageLuma8(img.to_luma8())
            } else {
                DynamicImage::ImageRgb8(img.to_rgb8())
            };
            encode(img, f, quality)?
        }
    };
    Ok(Rendered {
        kind,
        width,
        height,
        dpi,
    })
}

fn encode(img: DynamicImage, format: Format, quality: u8) -> Result<Kind, OutputError> {
    let mut buf = Cursor::new(Vec::new());
    match format {
        Format::Png => {
            img.write_to(&mut buf, ImageFormat::Png)
                .map_err(|e| OutputError::Image(e.to_string()))?;
            Ok(Kind::Png {
                data: buf.into_inner(),
            })
        }
        Format::Jpeg | Format::Pdf => {
            // Encode the concrete buffer: a `DynamicImage` view is seen as
            // RGBA by the encoder and would come out with three components.
            let mut enc = JpegEncoder::new_with_quality(&mut buf, quality);
            let result = match &img {
                DynamicImage::ImageLuma8(grey) => enc.encode_image(grey).map(|()| 1),
                DynamicImage::ImageRgb8(rgb) => enc.encode_image(rgb).map(|()| 3),
                other => enc.encode_image(&other.to_rgb8()).map(|()| 3),
            };
            let components = result.map_err(|e| OutputError::Image(e.to_string()))?;
            Ok(Kind::Jpeg {
                data: buf.into_inner(),
                components,
            })
        }
    }
}

/// Otsu's threshold: the grey level that best separates ink from paper.
fn otsu_threshold(luma: &image::GrayImage) -> u8 {
    let mut hist = [0u64; 256];
    for p in luma.pixels() {
        hist[p.0[0] as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return 128;
    }
    let sum_all: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, c)| i as f64 * *c as f64)
        .sum();
    let (mut sum_b, mut weight_b, mut best, mut best_t) = (0.0f64, 0u64, 0.0f64, 128u8);
    for (t, &count) in hist.iter().enumerate() {
        weight_b += count;
        if weight_b == 0 {
            continue;
        }
        let weight_f = total - weight_b;
        if weight_f == 0 {
            break;
        }
        sum_b += t as f64 * count as f64;
        let mean_b = sum_b / weight_b as f64;
        let mean_f = (sum_all - sum_b) / weight_f as f64;
        let between = weight_b as f64 * weight_f as f64 * (mean_b - mean_f).powi(2);
        if between > best {
            best = between;
            best_t = t as u8;
        }
    }
    best_t
}

/// 1 bit per pixel, 1 = white, each row padded to a whole byte.
fn pack_bilevel(luma: &image::GrayImage) -> Vec<u8> {
    let t = otsu_threshold(luma);
    let (w, h) = luma.dimensions();
    let stride = (w as usize).div_ceil(8);
    let mut out = vec![0u8; stride * h as usize];
    for (y, row) in luma.rows().enumerate() {
        for (x, p) in row.enumerate() {
            if p.0[0] > t {
                out[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    out
}

fn numbered(path: &Path, n: usize) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scan");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let name = if ext.is_empty() {
        format!("{stem}-{n}")
    } else {
        format!("{stem}-{n}.{ext}")
    };
    path.with_file_name(name)
}

fn save(path: &Path, bytes: &[u8]) -> Result<(), OutputError> {
    std::fs::write(path, bytes).map_err(|source| OutputError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// "1.5 MB", "640 KB", "900 B".
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{} KB", bytes / 1_000)
    } else {
        format!("{bytes} B")
    }
}

/// Parse "2M", "500K", "1.5MB", "800000" into bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_uppercase();
    let s = s.strip_suffix('B').unwrap_or(&s);
    let (num, mult) = if let Some(n) = s.strip_suffix('G') {
        (n, 1_000_000_000.0)
    } else if let Some(n) = s.strip_suffix('M') {
        (n, 1_000_000.0)
    } else if let Some(n) = s.strip_suffix('K') {
        (n, 1_000.0)
    } else {
        (s, 1.0)
    };
    let value: f64 = num.trim().parse().ok()?;
    if value <= 0.0 {
        return None;
    }
    Some((value * mult).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma, Rgb, RgbImage};

    fn jpeg_page(img: DynamicImage, color: ColorMode) -> Page {
        let mut buf = Cursor::new(Vec::new());
        JpegEncoder::new_with_quality(&mut buf, 90)
            .encode_image(&img)
            .unwrap();
        Page {
            data: buf.into_inner(),
            mime: "image/jpeg",
            dpi: 100,
            color,
        }
    }

    fn colour_page() -> Page {
        let mut img = RgbImage::new(80, 40);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 40 {
                Rgb([250, 250, 240])
            } else {
                Rgb([20, 30, 40])
            };
        }
        jpeg_page(DynamicImage::ImageRgb8(img), ColorMode::Color)
    }

    #[test]
    fn colour_jpeg_goes_into_pdf_untouched() {
        let page = colour_page();
        let r = render(&page, ColorMode::Color, Format::Pdf, None, 1.0).unwrap();
        match r.kind {
            Kind::Jpeg { data, components } => {
                assert_eq!(data, page.data);
                assert_eq!(components, 3);
            }
            _ => panic!("expected pass-through JPEG"),
        }
        assert_eq!((r.width, r.height, r.dpi), (80, 40, 100));
    }

    #[test]
    fn grey_and_bw_are_converted() {
        let page = colour_page();
        let r = render(&page, ColorMode::Gray, Format::Jpeg, None, 1.0).unwrap();
        match r.kind {
            Kind::Jpeg { data, components } => {
                assert_eq!(components, 1);
                // What the file really says, not just our bookkeeping.
                assert_eq!(jpeg::dimensions(&data), Some((80, 40, 1)));
            }
            _ => panic!("expected JPEG"),
        }
        let r = render(&page, ColorMode::BlackWhite, Format::Pdf, None, 1.0).unwrap();
        match r.kind {
            Kind::Bilevel { packed } => {
                assert_eq!(packed.len(), 10 * 40);
                assert_eq!(packed[0], 0xff); // left half white
                assert_eq!(packed[9], 0x00); // right half black
            }
            _ => panic!("expected bilevel"),
        }
        let r = render(&page, ColorMode::Color, Format::Png, None, 0.5).unwrap();
        assert!(matches!(r.kind, Kind::Png { .. }));
        assert_eq!((r.width, r.height, r.dpi), (40, 20, 50));
    }

    #[test]
    fn otsu_splits_two_levels() {
        let mut img = GrayImage::new(10, 10);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = Luma([if x < 5 { 30 } else { 220 }]);
        }
        let t = otsu_threshold(&img);
        assert!((30..220).contains(&t), "{t}");
    }

    #[test]
    fn size_limit_shrinks_until_it_fits() {
        let mut img = RgbImage::new(400, 400);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([
                (x * 7 % 256) as u8,
                (y * 13 % 256) as u8,
                ((x ^ y) % 256) as u8,
            ]);
        }
        let page = jpeg_page(DynamicImage::ImageRgb8(img), ColorMode::Color);
        let dir = std::env::temp_dir().join(format!("kagaz-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let full = write(
            std::slice::from_ref(&page),
            &OutputOptions {
                format: Format::Jpeg,
                color: ColorMode::Color,
                quality: None,
                max_bytes: None,
            },
            &dir.join("full.jpg"),
        )
        .unwrap();
        let limit = full[0].bytes / 3;
        let small = write(
            &[page.clone(), page.clone()],
            &OutputOptions {
                format: Format::Pdf,
                color: ColorMode::Color,
                quality: None,
                max_bytes: Some(limit),
            },
            &dir.join("small.pdf"),
        )
        .unwrap();
        assert!(small[0].bytes <= limit, "{} > {limit}", small[0].bytes);
        assert_eq!(small[0].pages, 2);
        assert!(small[0].warning.is_none());

        let two = write(
            &[page.clone(), page],
            &OutputOptions {
                format: Format::Png,
                color: ColorMode::Gray,
                quality: None,
                max_bytes: Some(10),
            },
            &dir.join("pages.png"),
        )
        .unwrap();
        assert_eq!(two.len(), 2);
        assert!(two[0].path.ends_with("pages-1.png"));
        assert!(two[1].warning.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sizes_parse_and_print() {
        assert_eq!(parse_size("2M"), Some(2_000_000));
        assert_eq!(parse_size("1.5MB"), Some(1_500_000));
        assert_eq!(parse_size("500k"), Some(500_000));
        assert_eq!(parse_size("800000"), Some(800_000));
        assert_eq!(parse_size("0"), None);
        assert_eq!(parse_size("big"), None);
        assert_eq!(human_size(1_536_000), "1.5 MB");
        assert_eq!(human_size(640_000), "640 KB");
        assert_eq!(Format::from_extension("JPEG"), Some(Format::Jpeg));
    }
}
