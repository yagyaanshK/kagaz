//! Tools for pictures of paper, defined once for every part of Kagaz that
//! has one: a scan, an ID page, a file on this device, later a camera's
//! photo. Each part only adds a button; the work is here.
//!
//! - [`open`]: a PDF, JPEG or PNG as pictures, one per page.
//! - Tidy: [`tidy`] (Clean background, Find the card) and [`frames`] (the
//!   person's own outlines around cards; see `scan::id`).
//! - [`crop`]: to an outline, keeping the scale, keeping the page, or
//!   filling a page without stretching.
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// Crop `p` to `shape` (an outline as fractions of the picture; a tilted
/// one keeps only what is inside it). `paper` is the page FillPage fills.
pub fn crop(p: &Picture, shape: &Shape, mode: CropMode, paper: Paper) -> Picture {
    let mut whole = p.img.clone();
    id::whiten_outside(&mut whole, std::slice::from_ref(shape), 0.0);
    if mode == CropMode::KeepPage {
        return Picture { img: whole, dpi: p.dpi };
    }
    let (w, h) = (whole.width() as f32, whole.height() as f32);
    let xs = shape.iter().map(|q| q[0].clamp(0.0, 1.0) * w);
    let ys = shape.iter().map(|q| q[1].clamp(0.0, 1.0) * h);
    let (x0, x1) = (xs.clone().fold(w, f32::min).floor(), xs.fold(0.0, f32::max).ceil());
    let (y0, y1) = (ys.clone().fold(h, f32::min).floor(), ys.fold(0.0, f32::max).ceil());
    let (cw, ch) = (((x1 - x0) as u32).max(1), ((y1 - y0) as u32).max(1));
    let area = imageops::crop_imm(&whole, x0 as u32, y0 as u32, cw, ch).to_image();
    if mode == CropMode::Area {
        return Picture { img: area, dpi: p.dpi };
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
    Picture { img: page, dpi: p.dpi }
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
        let out = crop(&page(), &around(55.0, 95.0, 105.0, 125.0), CropMode::Area, Paper::A4);
        assert_eq!((out.img.width(), out.img.height(), out.dpi), (50, 30, 20));
        assert_eq!(out.img.get_pixel(10, 10), &Rgb([20, 20, 20]));
    }

    #[test]
    fn crop_keeping_the_page_whitens_the_rest() {
        let mut p = page();
        p.img.put_pixel(5, 5, Rgb([0, 0, 0]));
        let out = crop(&p, &around(55.0, 95.0, 105.0, 125.0), CropMode::KeepPage, Paper::A4);
        assert_eq!(out.img.dimensions(), (165, 233));
        assert_eq!(out.img.get_pixel(5, 5), &Rgb([255, 255, 255]));
        assert_eq!(out.img.get_pixel(70, 110), &Rgb([20, 20, 20]));
    }

    #[test]
    fn filling_the_page_enlarges_without_stretching() {
        let out = crop(&page(), &around(60.0, 100.0, 100.0, 120.0), CropMode::FillPage, Paper::A4);
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
