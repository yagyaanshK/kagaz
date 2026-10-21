//! Tools for pictures of paper, defined once for every part of Kagaz that
//! has one: a scan, an ID page, a file on this device, later a camera's
//! photo. Each part only adds a button; the work is here.
//!
//! - [`open`]: a PDF, JPEG or PNG as pictures, one per page.
//! - Tidy: [`tidy`] (Clean background, Find the card) and [`frames`] (the
//!   person's own outlines around cards; see `scan::id`).
//! - [`crop`]: to an outline, keeping the scale, keeping the page,
//!   filling a page without stretching, or copies on a sheet to cut out
//!   (passport photos).
//! - [`compress`]: files under a size, lowering quality, then resolution.

use crate::output::{self, Encoded, Format, OutputOptions};
use crate::scan::id::{self, Quad, Shape, Tidy};
use crate::scan::{ColorMode, Page, Paper};
use image::{imageops, DynamicImage, ImageFormat, Rgb, RgbImage};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;

/// One page as a picture, with its resolution (so it keeps its real size).
#[derive(Debug, Clone)]
pub struct Picture {
    pub img: RgbImage,
    pub dpi: u32,
}

/// A PDF, JPEG or PNG file as pictures, one per page. A PDF's pages are
/// drawn on A4 at 600 dpi (300 for longer documents, to spare memory).
pub fn open(path: &Path) -> Result<Vec<Picture>, String> {
    let data = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if data.starts_with(b"%PDF") {
        let pages = hayro::hayro_syntax::Pdf::new(data)
            .map(|pdf| pdf.pages().len())
            .unwrap_or(1);
        let dpi = if pages <= 2 { 600 } else { 300 };
        let plan = crate::print::Plan {
            dpi,
            gray: false,
            media: "iso_a4_210x297mm".into(),
            width_pt: 595,
            height_pt: 842,
            sides: crate::print::Sides::OneSided,
            sheet_back_rotated: false,
            copies: 1,
        };
        let mut pictures = Vec::new();
        for p in crate::print::render_file(path, &plan, &mut |_| {}).map_err(|e| e.to_string())? {
            let img = if p.components == 1 {
                DynamicImage::ImageLuma8(
                    image::GrayImage::from_raw(p.width, p.height, p.pixels).ok_or("a page came out damaged")?,
                )
                .to_rgb8()
            } else {
                RgbImage::from_raw(p.width, p.height, p.pixels).ok_or("a page came out damaged")?
            };
            pictures.push(Picture { img, dpi });
        }
        return Ok(pictures);
    }
    let img = image::load_from_memory(&data).map_err(|_| "this is not a PDF, JPEG or PNG file".to_string())?;
    let dpi = output::jpeg::dpi(&data).unwrap_or(300);
    Ok(vec![Picture { img: img.to_rgb8(), dpi }])
}

/// A scanned page as a picture.
pub fn from_page(page: &Page) -> Result<Picture, String> {
    let img = image::load_from_memory(&page.data).map_err(|e| e.to_string())?;
    Ok(Picture { img: img.to_rgb8(), dpi: page.dpi.max(1) })
}

/// A picture as a page, losslessly (PNG), for saving or printing.
pub fn to_page(p: &Picture) -> Result<Page, String> {
    let mut png = Vec::new();
    DynamicImage::ImageRgb8(p.img.clone())
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(Page { data: png, mime: "image/png", dpi: p.dpi, color: ColorMode::Color })
}

/// Clean background and Find the card, as chosen; notes say where no card
/// stood out (that part is left as it was).
pub fn tidy(p: &mut Picture, t: Tidy) -> Vec<String> {
    id::tidy_image(&mut p.img, p.dpi, t)
}

/// Where the cards are, for frames to start on (one per half of a page).
pub fn find_frames(p: &Picture) -> Vec<(Quad, bool)> {
    id::find_cards(&p.img, p.dpi)
}

/// Where whatever is on the page is (a photo, a signature), for Crop's
/// frame to start on; else the whole picture, a little inset. As fractions.
pub fn find_area(p: &Picture) -> (Shape, bool) {
    let (w, h) = (p.img.width().max(1) as f32, p.img.height().max(1) as f32);
    let (found, (l, t, r, b)) = match id::find_card(&p.img, p.dpi) {
        Some((x0, y0, x1, y1)) => (true, (x0 as f32 / w, y0 as f32 / h, (x1 + 1) as f32 / w, (y1 + 1) as f32 / h)),
        None => (false, (0.02, 0.02, 0.98, 0.98)),
    };
    (vec![[l, t], [r, t], [r, b], [l, b]], found)
}

/// The person's frames around cards: white outside them, corners rounded
/// as a card's; then Clean background if asked.
pub fn frames(p: &mut Picture, shapes: &[Shape], background: bool) {
    id::tidy_outlined(&mut p.img, shapes, p.dpi, background);
}

/// What Crop makes of the area inside its outline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum CropMode {
    /// Just the area: a picture the outline's size, at the same scale (a
    /// signature strip, a photo); it prints at its real size.
    #[default]
    Area,
    /// The area made as large as fits the page, without stretching, with a
    /// small margin (a small photo to a full page).
    FillPage,
    /// The page as it was, white outside the area.
    KeepPage,
    /// Copies of the area on a sheet, with thin grey lines to cut along.
    Sheet(Sheet),
}

/// A sheet of copies (passport photos), to print and cut out.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub paper: SheetPaper,
    /// How many; None for as many as fit.
    pub copies: Option<u32>,
}

/// The paper a sheet of copies is printed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SheetPaper {
    A4,
    Letter,
    /// 4 x 6 in photo paper, upright.
    Photo4x6,
}

impl SheetPaper {
    fn size_mm(self) -> (f32, f32) {
        match self {
            SheetPaper::A4 => (210.0, 297.0),
            SheetPaper::Letter => (215.9, 279.4),
            SheetPaper::Photo4x6 => (101.6, 152.4),
        }
    }

    fn name(self) -> &'static str {
        match self {
            SheetPaper::A4 => "A4",
            SheetPaper::Letter => "Letter",
            SheetPaper::Photo4x6 => "4 × 6 in photo paper",
        }
    }
}

/// Crop `p` to `shape` (an outline as fractions of the picture; only what
/// is inside it is kept). The area comes out level: turned so the outline's
/// bottom side is horizontal, never stretched. `paper` is the page
/// FillPage fills. `size_mm` (width, height), for a frame of that shape (a
/// passport photo), makes the area exactly that size, to print true; Fill
/// the page fills it anyway. A note says how a sheet of copies came out.
pub fn crop(
    p: &Picture,
    shape: &Shape,
    mode: CropMode,
    paper: Paper,
    size_mm: Option<[f32; 2]>,
) -> (Picture, Option<String>) {
    let (w, h) = (p.img.width() as f32, p.img.height() as f32);
    let pts: Vec<[f32; 2]> = shape.iter().map(|q| [q[0].clamp(0.0, 1.0) * w, q[1].clamp(0.0, 1.0) * h]).collect();
    let turn = bottom_slope(&pts);
    let size_mm = size_mm.filter(|s| mode != CropMode::FillPage && s[0] > 0.0 && s[1] > 0.0);
    // Under a twentieth of a degree: no turn, the pixels as they are.
    let square = turn.abs() < 0.001;
    let mut area = if square {
        let mut whole = p.img.clone();
        id::whiten_outside(&mut whole, std::slice::from_ref(shape), 0.0);
        if mode == CropMode::KeepPage && size_mm.is_none() {
            return (Picture { img: whole, dpi: p.dpi }, None);
        }
        let xs = pts.iter().map(|q| q[0]);
        let ys = pts.iter().map(|q| q[1]);
        let (x0, x1) = (xs.clone().fold(w, f32::min).floor(), xs.fold(0.0, f32::max).ceil());
        let (y0, y1) = (ys.clone().fold(h, f32::min).floor(), ys.fold(0.0, f32::max).ceil());
        let (cw, ch) = (((x1 - x0) as u32).max(1), ((y1 - y0) as u32).max(1));
        imageops::crop_imm(&whole, x0 as u32, y0 as u32, cw, ch).to_image()
    } else {
        level(&p.img, &pts, turn)
    };
    if let Some([mw, mh]) = size_mm {
        let px = |mm: f32| ((mm / 25.4 * p.dpi as f32).round() as u32).max(1);
        area = imageops::resize(&area, px(mw), px(mh), imageops::FilterType::CatmullRom);
    }
    if mode == CropMode::KeepPage {
        // Level where it was: centred on the outline's middle.
        let xs = pts.iter().map(|q| q[0]);
        let ys = pts.iter().map(|q| q[1]);
        let cx = (xs.clone().fold(w, f32::min) + xs.fold(0.0, f32::max)) / 2.0;
        let cy = (ys.clone().fold(h, f32::min) + ys.fold(0.0, f32::max)) / 2.0;
        let mut page = RgbImage::from_pixel(p.img.width(), p.img.height(), Rgb([255, 255, 255]));
        let at = |c: f32, size: u32| (c - size as f32 / 2.0).round() as i64;
        imageops::replace(&mut page, &area, at(cx, area.width()), at(cy, area.height()));
        return (Picture { img: page, dpi: p.dpi }, None);
    }
    if let CropMode::Sheet(sheet) = mode {
        let (picture, note) = copies(&area, p.dpi, sheet);
        return (picture, Some(note));
    }
    let (cw, ch) = area.dimensions();
    if mode == CropMode::Area {
        return (Picture { img: area, dpi: p.dpi }, None);
    }
    // Fill the page: as large as fits inside 5 mm margins, never stretched.
    let (pw_mils, ph_mils) = paper.size_mils().unwrap_or((8268, 11693));
    let px = |mils: u32| (mils as u64 * p.dpi as u64 / 1000) as u32;
    let (pw, ph) = (px(pw_mils), px(ph_mils));
    let margin = (p.dpi as f32 * 5.0 / 25.4) as u32;
    let (room_w, room_h) = (pw.saturating_sub(2 * margin).max(1), ph.saturating_sub(2 * margin).max(1));
    let scale = (room_w as f32 / cw as f32).min(room_h as f32 / ch as f32);
    let (nw, nh) = (((cw as f32 * scale) as u32).max(1), ((ch as f32 * scale) as u32).max(1));
    let big = imageops::resize(&area, nw, nh, imageops::FilterType::CatmullRom);
    let mut page = RgbImage::from_pixel(pw, ph, Rgb([255, 255, 255]));
    imageops::replace(&mut page, &big, ((pw - nw) / 2) as i64, ((ph - nh) / 2) as i64);
    (Picture { img: page, dpi: p.dpi }, None)
}

/// Copies of `area` (at `dpi`) on a sheet: in rows, 3 mm apart with a thin
/// grey line down the middle of each gap to cut along, inside 5 mm margins.
/// Upright, or turned when only that fits (a 6 x 4 print on 4 x 6 paper);
/// made smaller only when not even one fits. A full sheet sits in the
/// middle; fewer copies start at the top, so the rest of the paper can be
/// used again. Up to 600 dpi, plenty to print a photo.
fn copies(area: &RgbImage, dpi: u32, sheet: Sheet) -> (Picture, String) {
    let out_dpi = dpi.clamp(1, 600);
    let mut one = if out_dpi < dpi {
        let k = out_dpi as f32 / dpi as f32;
        let size = |n: u32| ((n as f32 * k).round() as u32).max(1);
        imageops::resize(area, size(area.width()), size(area.height()), imageops::FilterType::CatmullRom)
    } else {
        area.clone()
    };
    let px = |mm: f32| (mm / 25.4 * out_dpi as f32).round() as u32;
    let (pw, ph) = (px(sheet.paper.size_mm().0), px(sheet.paper.size_mm().1));
    let (margin, gap) = (px(5.0), px(3.0).max(2));
    let (room_w, room_h) = (pw.saturating_sub(2 * margin), ph.saturating_sub(2 * margin));
    let fit = |w: u32, h: u32| ((room_w + gap) / (w + gap).max(1), (room_h + gap) / (h + gap).max(1));
    let mut notes = Vec::new();
    let (mut cols, mut rows) = fit(one.width(), one.height());
    if cols * rows == 0 {
        let (c, r) = fit(one.height(), one.width());
        if c * r > 0 {
            one = imageops::rotate90(&one);
            (cols, rows) = (c, r);
            notes.push("turned on its side to fit".to_string());
        } else {
            let k = (room_w as f32 / one.width() as f32).min(room_h as f32 / one.height() as f32);
            let size = |n: u32| ((n as f32 * k) as u32).max(1);
            one = imageops::resize(&one, size(one.width()), size(one.height()), imageops::FilterType::CatmullRom);
            (cols, rows) = (1, 1);
            notes.push("made smaller to fit, so not at its true size".to_string());
        }
    }
    let room = cols * rows;
    let n = sheet.copies.map_or(room, |c| c.clamp(1, room));
    let (used_cols, used_rows) = (cols.min(n), n.div_ceil(cols));
    let (w, h) = one.dimensions();
    let grid_w = used_cols * w + (used_cols - 1) * gap;
    let grid_h = used_rows * h + (used_rows - 1) * gap;
    let x0 = (pw - grid_w) / 2;
    let y0 = if n == room { (ph - grid_h) / 2 } else { margin };
    let mut page = RgbImage::from_pixel(pw, ph, Rgb([255, 255, 255]));
    let line = Rgb([185, 185, 185]);
    let thick = (out_dpi / 300).max(1);
    for i in 0..n {
        let (x, y) = (x0 + (i % cols) * (w + gap), y0 + (i / cols) * (h + gap));
        imageops::replace(&mut page, &one, x as i64, y as i64);
        // Cut lines around it, halfway into the gap (shared with the next).
        let (l, t) = (x.saturating_sub(gap / 2), y.saturating_sub(gap / 2));
        let (r, b) = ((x + w + gap / 2).min(pw - 1), (y + h + gap / 2).min(ph - 1));
        for k in 0..thick {
            for xx in l..=r {
                page.put_pixel(xx, (t + k).min(ph - 1), line);
                page.put_pixel(xx, b.saturating_sub(k), line);
            }
            for yy in t..=b {
                page.put_pixel((l + k).min(pw - 1), yy, line);
                page.put_pixel(r.saturating_sub(k), yy, line);
            }
        }
    }
    let mut note = format!("{n} {} on {}", if n == 1 { "copy" } else { "copies" }, sheet.paper.name());
    if let Some(asked) = sheet.copies.filter(|&c| c > room) {
        note += &format!(": {asked} asked for, only {room} fit");
    } else if n < room {
        note += &format!(" (room for {room})");
    }
    if !notes.is_empty() {
        note += &format!(", {}", notes.join(", "));
    }
    (Picture { img: page, dpi: out_dpi }, note + ".")
}

/// How far the outline's bottom side (bottom left to bottom right corner)
/// slopes, in radians; within a quarter turn, so the area stays upright.
fn bottom_slope(pts: &[[f32; 2]]) -> f32 {
    let (br, bl) = match pts.len() {
        8 => (pts[4], pts[6]),
        4 => (pts[2], pts[3]),
        _ => return 0.0,
    };
    let a = (br[1] - bl[1]).atan2(br[0] - bl[0]);
    let quarter = std::f32::consts::FRAC_PI_2;
    if a > quarter {
        a - 2.0 * quarter
    } else if a < -quarter {
        a + 2.0 * quarter
    } else {
        a
    }
}

/// What is inside the outline (`pts`, in pixels), turned back by `turn` so
/// its bottom side is level: the box around it, white outside the outline.
fn level(img: &RgbImage, pts: &[[f32; 2]], turn: f32) -> RgbImage {
    let (s, c) = turn.sin_cos();
    // Turned back: u along the bottom side, v across it.
    let turned: Vec<[f32; 2]> = pts.iter().map(|q| [q[0] * c + q[1] * s, q[1] * c - q[0] * s]).collect();
    let u0 = turned.iter().map(|q| q[0]).fold(f32::MAX, f32::min);
    let u1 = turned.iter().map(|q| q[0]).fold(f32::MIN, f32::max);
    let v0 = turned.iter().map(|q| q[1]).fold(f32::MAX, f32::min);
    let v1 = turned.iter().map(|q| q[1]).fold(f32::MIN, f32::max);
    let (ow, oh) = (((u1 - u0).round() as u32).max(1), ((v1 - v0).round() as u32).max(1));
    let (w, h) = (img.width() as f32, img.height() as f32);
    // Each pixel from where it was, between the four pixels around it.
    let sample = |x: f32, y: f32| -> [u8; 3] {
        let (x, y) = (x - 0.5, y - 0.5);
        if x < -0.5 || y < -0.5 || x > w - 0.5 || y > h - 0.5 {
            return [255, 255, 255];
        }
        let (x, y) = (x.clamp(0.0, w - 1.0), y.clamp(0.0, h - 1.0));
        let (xa, ya) = (x.floor() as u32, y.floor() as u32);
        let (xb, yb) = ((xa + 1).min(img.width() - 1), (ya + 1).min(img.height() - 1));
        let (fx, fy) = (x - xa as f32, y - ya as f32);
        let (a, b, cc, d) = (img.get_pixel(xa, ya), img.get_pixel(xb, ya), img.get_pixel(xa, yb), img.get_pixel(xb, yb));
        std::array::from_fn(|k| {
            let top = a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx;
            let bottom = cc[k] as f32 * (1.0 - fx) + d[k] as f32 * fx;
            (top * (1.0 - fy) + bottom * fy).round() as u8
        })
    };
    let mut out = RgbImage::new(ow, oh);
    let stride = ow as usize * 3;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
    let band = (oh as usize).div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (i, rows) in out.chunks_mut(band * stride).enumerate() {
            let sample = &sample;
            scope.spawn(move || {
                for (r, row) in rows.chunks_mut(stride).enumerate() {
                    let v = v0 + (i * band + r) as f32 + 0.5;
                    for (x, px) in row.chunks_mut(3).enumerate() {
                        let u = u0 + x as f32 + 0.5;
                        px.copy_from_slice(&sample(u * c - v * s, u * s + v * c));
                    }
                }
            });
        }
    });
    let outline: Shape = turned.iter().map(|q| [(q[0] - u0) / ow as f32, (q[1] - v0) / oh as f32]).collect();
    id::whiten_outside(&mut out, &[outline], 0.0);
    out
}

/// The pictures as files of `format`, each under `max_bytes` if it can be
/// (quality lowered first, then resolution); a warning says when not.
pub fn compress(pictures: &[Picture], format: Format, max_bytes: u64) -> Result<Vec<Encoded>, String> {
    let pages = pictures.iter().map(to_page).collect::<Result<Vec<_>, _>>()?;
    output::encode_files(
        &pages,
        &OutputOptions { format, color: ColorMode::Color, quality: Some(92), max_bytes: Some(max_bytes) },
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A white A4 page at 20 dpi (165 x 233) with a dark 40 x 20 "signature".
    fn page() -> Picture {
        let mut img = RgbImage::from_pixel(165, 233, Rgb([255, 255, 255]));
        for y in 100..120 {
            for x in 60..100 {
                img.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        Picture { img, dpi: 20 }
    }

    fn around(x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
        let (w, h) = (165.0, 233.0);
        vec![[x0 / w, y0 / h], [x1 / w, y0 / h], [x1 / w, y1 / h], [x0 / w, y1 / h]]
    }

    #[test]
    fn crop_to_the_area_keeps_its_scale() {
        let out = crop(&page(), &around(55.0, 95.0, 105.0, 125.0), CropMode::Area, Paper::A4, None).0;
        assert_eq!((out.img.width(), out.img.height(), out.dpi), (50, 30, 20));
        assert_eq!(out.img.get_pixel(10, 10), &Rgb([20, 20, 20]));
    }

    #[test]
    fn crop_keeping_the_page_whitens_the_rest() {
        let mut p = page();
        p.img.put_pixel(5, 5, Rgb([0, 0, 0]));
        let out = crop(&p, &around(55.0, 95.0, 105.0, 125.0), CropMode::KeepPage, Paper::A4, None).0;
        assert_eq!(out.img.dimensions(), (165, 233));
        assert_eq!(out.img.get_pixel(5, 5), &Rgb([255, 255, 255]));
        assert_eq!(out.img.get_pixel(70, 110), &Rgb([20, 20, 20]));
    }

    #[test]
    fn filling_the_page_enlarges_without_stretching() {
        let out = crop(&page(), &around(60.0, 100.0, 100.0, 120.0), CropMode::FillPage, Paper::A4, None).0;
        assert_eq!(out.img.dimensions(), (165, 233), "an A4 page at the same dpi");
        // 40 x 20 grows to the page's width less margins: 2:1 kept.
        let dark: Vec<(u32, u32)> =
            out.img.enumerate_pixels().filter(|(_, _, p)| p[0] < 100).map(|(x, y, _)| (x, y)).collect();
        let (minx, maxx) = (dark.iter().map(|d| d.0).min().unwrap(), dark.iter().map(|d| d.0).max().unwrap());
        let (miny, maxy) = (dark.iter().map(|d| d.1).min().unwrap(), dark.iter().map(|d| d.1).max().unwrap());
        let (dw, dh) = ((maxx - minx + 1) as f32, (maxy - miny + 1) as f32);
        assert!(dw > 140.0, "{dw}");
        assert!((dw / dh - 2.0).abs() < 0.15, "{dw} x {dh}");
    }

    // A dark 80 x 40 bar on a white page, turned 10 degrees about (100,
    // 100); its corners from the top left, as fractions.
    fn tilted() -> (Picture, Shape) {
        let (s, c) = 10f32.to_radians().sin_cos();
        let mut img = RgbImage::from_pixel(200, 200, Rgb([255, 255, 255]));
        for (x, y, p) in img.enumerate_pixels_mut() {
            let (dx, dy) = (x as f32 + 0.5 - 100.0, y as f32 + 0.5 - 100.0);
            let (u, v) = (dx * c + dy * s, dy * c - dx * s);
            if u.abs() < 40.0 && v.abs() < 20.0 {
                *p = Rgb([20, 20, 20]);
            }
        }
        let corner = |u: f32, v: f32| [(100.0 + u * c - v * s) / 200.0, (100.0 + u * s + v * c) / 200.0];
        (Picture { img, dpi: 20 }, vec![corner(-40.0, -20.0), corner(40.0, -20.0), corner(40.0, 20.0), corner(-40.0, 20.0)])
    }

    #[test]
    fn a_tilted_crop_comes_out_level() {
        let (p, shape) = tilted();
        let out = crop(&p, &shape, CropMode::Area, Paper::A4, None).0;
        let (w, h) = out.img.dimensions();
        assert!((79..=81).contains(&w) && (39..=41).contains(&h), "{w} x {h}");
        // Level and unstretched: dark all over but a pixel's edge.
        for y in 2..h - 2 {
            for x in 2..w - 2 {
                assert!(out.img.get_pixel(x, y)[0] < 60, "({x}, {y})");
            }
        }
        // A middle of a side, moved inward, still turns by the bottom side.
        let mut bent = Vec::new();
        for i in 0..4 {
            let (a, b) = (shape[i], shape[(i + 1) % 4]);
            bent.push(a);
            bent.push([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
        }
        let out8 = crop(&p, &bent, CropMode::Area, Paper::A4, None).0;
        assert_eq!(out8.img.dimensions(), out.img.dimensions());
    }

    #[test]
    fn a_tilted_crop_keeping_the_page_stays_in_place() {
        let (p, shape) = tilted();
        let out = crop(&p, &shape, CropMode::KeepPage, Paper::A4, None).0;
        assert_eq!(out.img.dimensions(), (200, 200));
        // Level: the row through the middle is dark 80 wide, centred.
        let dark: Vec<u32> = (0..200).filter(|&x| out.img.get_pixel(x, 100)[0] < 100).collect();
        assert!((78..=82).contains(&(dark.len() as u32)), "{}", dark.len());
        assert!((59..=61).contains(&dark[0]), "{}", dark[0]);
        // Near the bar's bottom right corner, dark while it was tilted, is not.
        assert_eq!(p.img.get_pixel(134, 124)[0], 20);
        assert_eq!(out.img.get_pixel(134, 124), &Rgb([255, 255, 255]));
    }

    #[test]
    fn a_true_size_crop_comes_out_at_that_size() {
        // 40 x 20 pixels at 20 dpi is 2 x 1 in; asked for 4 x 2 in (80 x 40).
        let shape = around(60.0, 100.0, 100.0, 120.0);
        let out = crop(&page(), &shape, CropMode::Area, Paper::A4, Some([101.6, 50.8])).0;
        assert_eq!((out.img.width(), out.img.height(), out.dpi), (80, 40, 20));
        let kept = crop(&page(), &shape, CropMode::KeepPage, Paper::A4, Some([101.6, 50.8])).0;
        assert_eq!(kept.img.dimensions(), (165, 233));
        assert!(kept.img.get_pixel(45, 92)[0] < 100, "grown in place");
    }

    #[test]
    fn a_sheet_of_passport_photos() {
        // 35 x 45 mm at 100 dpi: 138 x 177 pixels. On A4 (827 x 1169, 20
        // margin, 12 gap): 5 across, 6 down.
        let mut img = RgbImage::from_pixel(827, 1169, Rgb([255, 255, 255]));
        for y in 100..300 {
            for x in 100..250 {
                img.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        let p = Picture { img, dpi: 100 };
        let (w, h) = (827.0, 1169.0);
        let shape = vec![[100.0 / w, 100.0 / h], [250.0 / w, 100.0 / h], [250.0 / w, 300.0 / h], [100.0 / w, 300.0 / h]];
        let full = Sheet { paper: SheetPaper::A4, copies: None };
        let (out, note) = crop(&p, &shape, CropMode::Sheet(full), Paper::A4, Some([35.0, 45.0]));
        assert_eq!(out.img.dimensions(), (827, 1169));
        assert_eq!(note.as_deref(), Some("30 copies on A4."));
        let dark = out.img.pixels().filter(|p| p[0] < 100).count();
        // Every copy whole (but a resized edge's soft pixels).
        assert!((30 * 136 * 175..=30 * 138 * 177).contains(&dark), "{dark}");
        assert!(out.img.pixels().any(|p| p[0] == 185), "cut lines");
        let eight = Sheet { paper: SheetPaper::A4, copies: Some(8) };
        let (_, note) = crop(&p, &shape, CropMode::Sheet(eight), Paper::A4, Some([35.0, 45.0]));
        assert_eq!(note.as_deref(), Some("8 copies on A4 (room for 30)."));
        // A 6 x 4 in print on 4 x 6 in paper: turned to fit.
        let wide = Sheet { paper: SheetPaper::Photo4x6, copies: None };
        let (out, note) = crop(&p, &shape, CropMode::Sheet(wide), Paper::A4, Some([152.4, 101.6]));
        assert_eq!(out.img.dimensions(), (400, 600));
        assert_eq!(note.as_deref(), Some("1 copy on 4 × 6 in photo paper, made smaller to fit, so not at its true size."));
    }

    #[test]
    fn compress_gets_under_the_limit() {
        let mut img = RgbImage::new(600, 800);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([(x * 7 % 256) as u8, (y * 13 % 256) as u8, ((x ^ y) % 256) as u8]);
        }
        let files = compress(&[Picture { img, dpi: 100 }], Format::Jpeg, 60_000).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].bytes.len() <= 60_000 || files[0].warning.is_some());
    }
}
