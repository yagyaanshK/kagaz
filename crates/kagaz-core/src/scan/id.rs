//! Both sides of an ID card (or a passport page) on one page, the way a
//! printer's "2 in 1 ID copy" does it: each side is a scan of the top half
//! of the glass (`ScanRequest::half_page`), and the page is the front's half
//! above the back's, each just as it lay on the glass. Nothing looks for the
//! card: wherever it sat in that half is where it shows on the page.

use super::{ColorMode, Page, Paper};
use image::{imageops, DynamicImage, ImageFormat, Rgb, RgbImage};
use serde::Deserialize;
use std::io::Cursor;

/// Tidying the person may ask for; both off unless chosen.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Tidy {
    /// The lid's off-white around the card becomes pure white; the card is
    /// left as scanned.
    #[serde(default)]
    pub background: bool,
    /// Find the card and make everything outside it white.
    #[serde(default)]
    pub card: bool,
}

/// The page: `front` in the top half, `back` in the bottom half, at the
/// front's resolution. A side bigger than its half (a scanner that sent the
/// whole glass) is cut to the half; a smaller one keeps its place at the
/// top left, the glass's corner.
pub fn page(front: &Page, back: &Page, paper: Paper) -> Result<Page, String> {
    page_tidied(front, back, paper, Tidy::default()).map(|(p, _)| p)
}

/// [`page`], tidied as asked; with a note for each side whose card was not
/// found (that side is left as scanned).
pub fn page_tidied(
    front: &Page,
    back: &Page,
    paper: Paper,
    tidy: Tidy,
) -> Result<(Page, Vec<String>), String> {
    let mut notes = Vec::new();
    let (w_mils, h_mils) = paper.size_mils().unwrap_or((8268, 11693));
    let dpi = front.dpi.max(1);
    let px = |mils: u32| ((mils as u64 * dpi as u64) / 1000) as u32;
    let (width, height) = (px(w_mils), px(h_mils));
    let half = height / 2;

    let mut sheet = RgbImage::from_pixel(width, height, Rgb([255, 255, 255]));
    for (side, top, name) in [(front, 0, "front"), (back, half, "back")] {
        let mut img =
            decode(side).map_err(|e| format!("the {name} scan could not be read: {e}"))?;
        // The same size on paper even if the back came at another resolution.
        if side.dpi.max(1) != dpi {
            let scale = dpi as f64 / side.dpi.max(1) as f64;
            let (w, h) = (img.width() as f64 * scale, img.height() as f64 * scale);
            img = img.resize_exact(
                w.round() as u32,
                h.round() as u32,
                imageops::FilterType::Triangle,
            );
        }
        let mut img = img
            .crop_imm(0, 0, img.width().min(width), img.height().min(half))
            .to_rgb8();
        if tidy.card && !whiten_outside_card(&mut img, dpi) {
            notes.push(format!(
                "The card on the {name} was not found clearly, so that side is as scanned."
            ));
        }
        if tidy.background {
            whiten_background(&mut img);
        }
        imageops::replace(&mut sheet, &img, 0, top as i64);
    }

    // Colour scans as JPEG; black-and-white (text, lines) as PNG, sharp.
    let color = if front.color == back.color {
        front.color
    } else {
        ColorMode::Color
    };
    let mut data = Vec::new();
    let mime = if color == ColorMode::BlackWhite {
        DynamicImage::ImageRgb8(sheet)
            .to_luma8()
            .write_to(&mut Cursor::new(&mut data), ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        "image/png"
    } else {
        let mut out = Cursor::new(&mut data);
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92);
        if color == ColorMode::Gray {
            enc.encode_image(&DynamicImage::ImageRgb8(sheet).to_luma8())
        } else {
            enc.encode_image(&sheet)
        }
        .map_err(|e| e.to_string())?;
        "image/jpeg"
    };
    Ok((
        Page {
            data,
            mime,
            dpi,
            color,
        },
        notes,
    ))
}

/// Tidy a saved scan or photo, as an ID page's sides are tidied. A page
/// shape (taller than wide, about A4's or Letter's proportions) is taken as
/// an ID page: each half is one side. Anything else is one card or page.
/// Notes say where a card was not found (that part is left as it was).
pub fn tidy_image(img: &mut RgbImage, dpi: u32, tidy: Tidy) -> Vec<String> {
    let mut notes = Vec::new();
    let (w, h) = img.dimensions();
    for (top, height, name) in parts(w, h) {
        let mut part = imageops::crop_imm(img, 0, top, w, height).to_image();
        if tidy.card && !whiten_outside_card(&mut part, dpi) {
            notes.push(format!("No card stood out in the {name}, so it is left as it was."));
        }
        if tidy.background {
            whiten_background(&mut part);
        }
        imageops::replace(img, &part, 0, top as i64);
    }
    notes
}

fn luma(p: &Rgb<u8>) -> u8 {
    ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8
}

fn chroma(p: &Rgb<u8>) -> u8 {
    p.0.iter().max().unwrap() - p.0.iter().min().unwrap()
}

/// The lid's brightness: most of a half-glass scan is lid (a card is about a
/// sixth of it), so the median brightness is the lid's. None when that is
/// dark (a black lid, or something covering the glass): nothing to whiten.
fn lid_level(img: &RgbImage) -> Option<u8> {
    let mut counts = [0u64; 256];
    for p in img.pixels() {
        counts[luma(p) as usize] += 1;
    }
    let half = img.pixels().len() as u64 / 2;
    let mut seen = 0;
    let level = (0..256).find(|&v| {
        seen += counts[v];
        seen > half
    })? as u8;
    (level >= 150).then_some(level)
}

/// Lid-coloured pixels (bright and without colour) become white.
fn whiten_background(img: &mut RgbImage) {
    let Some(lid) = lid_level(img) else { return };
    let floor = lid.saturating_sub(18);
    for p in img.pixels_mut() {
        if luma(p) >= floor && chroma(p) < 24 {
            *p = Rgb([255, 255, 255]);
        }
    }
}

/// Find the card (whatever is clearly not lid) and make everything outside
/// it white, keeping a small margin. False, with nothing changed, when no
/// card-sized thing stands out.
fn whiten_outside_card(img: &mut RgbImage, dpi: u32) -> bool {
    let Some((x0, y0, x1, y1)) = find_card(img, dpi) else {
        return false;
    };
    for (x, y, p) in img.enumerate_pixels_mut() {
        if x < x0 || x > x1 || y < y0 || y > y1 {
            *p = Rgb([255, 255, 255]);
        }
    }
    true
}

/// The card's box (left, top, right, bottom, in pixels): whatever is clearly
/// not lid, with a small margin. None when no card-sized thing stands out.
fn find_card(img: &RgbImage, dpi: u32) -> Option<(u32, u32, u32, u32)> {
    let lid = lid_level(img)?;
    let (w, h) = img.dimensions();
    // In u16: a bright pixel plus the margin passes 255.
    let not_lid = |p: &Rgb<u8>| (luma(p) as u16) + 30 < lid as u16 || chroma(p) > 40;
    let mut cols = vec![0u32; w as usize];
    let mut rows = vec![0u32; h as usize];
    for (x, y, p) in img.enumerate_pixels() {
        if not_lid(p) {
            cols[x as usize] += 1;
            rows[y as usize] += 1;
        }
    }
    // A row or column counts when a fair share of it is not lid: specks of
    // dust do not.
    let min_col = (h / 100).max(4);
    let min_row = (w / 100).max(4);
    let span = |counts: &[u32], min: u32| {
        let first = counts.iter().position(|&c| c >= min)?;
        let last = counts.iter().rposition(|&c| c >= min)?;
        Some((first as u32, last as u32))
    };
    let (x0, x1) = span(&cols, min_col)?;
    let (y0, y1) = span(&rows, min_row)?;
    // About 2 mm around what was found, for the card's own pale edge.
    let margin = (dpi as f32 * 2.0 / 25.4).round() as u32;
    let (x0, y0) = (x0.saturating_sub(margin), y0.saturating_sub(margin));
    let (x1, y1) = ((x1 + margin).min(w - 1), (y1 + margin).min(h - 1));
    let found = (x1 - x0 + 1) as f64 * (y1 - y0 + 1) as f64;
    let share = found / (w as f64 * h as f64);
    // A card is about 15% of half the glass, an open passport most of it;
    // nearly all of it is no card standing out.
    (0.03..=0.97).contains(&share).then_some((x0, y0, x1, y1))
}

/// An outline around a card: four corners in order around it, each (x, y)
/// as fractions of the whole picture's width and height (0 to 1).
pub type Quad = [[f32; 2]; 4];

/// The parts a picture is tidied in: each half of a page shape (an ID
/// page), else the whole. (top row, rows, name).
fn parts(w: u32, h: u32) -> Vec<(u32, u32, &'static str)> {
    let ratio = h as f64 / w.max(1) as f64;
    if (1.25..=1.5).contains(&ratio) {
        vec![(0, h / 2, "top half"), (h / 2, h - h / 2, "bottom half")]
    } else {
        vec![(0, h, "picture")]
    }
}

/// Where the cards are, to start the outlines the person can adjust: one
/// per part (each half of a page, or the whole), with whether a card was
/// found there. Where none was, the outline is the part, a little inset.
pub fn find_cards(img: &RgbImage, dpi: u32) -> Vec<(Quad, bool)> {
    let (w, h) = img.dimensions();
    let (fw, fh) = (w.max(1) as f32, h.max(1) as f32);
    parts(w, h)
        .into_iter()
        .map(|(top, height, _)| {
            let part = imageops::crop_imm(img, 0, top, w, height).to_image();
            let (found, (x0, y0, x1, y1)) = match find_card(&part, dpi) {
                Some(b) => (true, b),
                None => {
                    let (dx, dy) = (w / 50, height / 50);
                    (false, (dx, dy, w - 1 - dx, height - 1 - dy))
                }
            };
            let (l, r) = (x0 as f32 / fw, (x1 + 1) as f32 / fw);
            let (t, b) = ((top + y0) as f32 / fh, (top + y1 + 1) as f32 / fh);
            ([[l, t], [r, t], [r, b], [l, b]], found)
        })
        .collect()
}

/// White everywhere outside the outlines; with none, all white.
pub fn whiten_outside(img: &mut RgbImage, quads: &[Quad]) {
    let (w, h) = img.dimensions();
    let (fw, fh) = (w as f32, h as f32);
    let mut spans: Vec<(f32, f32)> = Vec::new();
    for y in 0..h {
        // Where this row (through the pixels' middles) is inside an outline.
        let row = y as f32 + 0.5;
        spans.clear();
        for q in quads {
            let mut xs: Vec<f32> = Vec::with_capacity(4);
            for i in 0..4 {
                let (ax, ay) = (q[i][0] * fw, q[i][1] * fh);
                let (bx, by) = (q[(i + 1) % 4][0] * fw, q[(i + 1) % 4][1] * fh);
                if (ay <= row && row < by) || (by <= row && row < ay) {
                    xs.push(ax + (row - ay) * (bx - ax) / (by - ay));
                }
            }
            xs.sort_by(|a, b| a.total_cmp(b));
            for pair in xs.chunks(2) {
                if let [a, b] = pair {
                    spans.push((*a, *b));
                }
            }
        }
        for x in 0..w {
            let mid = x as f32 + 0.5;
            if !spans.iter().any(|&(a, b)| a <= mid && mid <= b) {
                img.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
    }
}

/// Tidy with outlines the person set (in place of Find the card's own),
/// then Clean background if asked.
pub fn tidy_outlined(img: &mut RgbImage, quads: &[Quad], background: bool) {
    whiten_outside(img, quads);
    if background {
        whiten_background(img);
    }
}

fn decode(p: &Page) -> image::ImageResult<DynamicImage> {
    image::load_from_memory(&p.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A side as a scanner sends it: white, with a dark "card" at (x, y).
    fn side(w: u32, h: u32, card: (u32, u32), shade: u8) -> Page {
        let mut img = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
        for y in card.1..card.1 + 20 {
            for x in card.0..card.0 + 30 {
                img.put_pixel(x, y, Rgb([shade, shade, shade]));
            }
        }
        let mut data = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut data), ImageFormat::Png)
            .unwrap();
        Page {
            data,
            mime: "image/png",
            dpi: 10,
            color: ColorMode::Color,
        }
    }

    fn at(p: &Page, x: u32, y: u32) -> u8 {
        image::load_from_memory(&p.data)
            .unwrap()
            .to_luma8()
            .get_pixel(x, y)
            .0[0]
    }

    #[test]
    fn each_side_keeps_its_place_in_its_half() {
        // A4 at 10 dpi: 82 x 116 px, halves of 58.
        let front = side(82, 58, (5, 6), 0);
        let back = side(82, 58, (40, 10), 0);
        let sheet = page(&front, &back, Paper::A4).unwrap();
        let img = image::load_from_memory(&sheet.data).unwrap();
        assert_eq!((img.width(), img.height()), (82, 116));
        assert!(at(&sheet, 10, 10) < 60, "front card where it lay");
        assert!(
            at(&sheet, 45, 58 + 15) < 60,
            "back card where it lay, in the lower half"
        );
        assert!(
            at(&sheet, 45, 15) > 200,
            "nothing of the back in the top half"
        );
        assert!(
            at(&sheet, 10, 58 + 10) > 200,
            "nothing of the front in the lower half"
        );
    }

    // A half-glass scan: a light grey lid with a darker "card" and a speck of dust.
    fn on_lid(card: (u32, u32, u32, u32)) -> RgbImage {
        let mut img = RgbImage::from_pixel(200, 140, Rgb([214, 214, 210]));
        for y in card.1..card.1 + card.3 {
            for x in card.0..card.0 + card.2 {
                img.put_pixel(x, y, Rgb([90, 120, 160]));
            }
        }
        img.put_pixel(180, 120, Rgb([20, 20, 20]));
        img
    }

    #[test]
    fn the_lid_becomes_white_and_the_card_stays() {
        let mut img = on_lid((20, 15, 80, 50));
        whiten_background(&mut img);
        assert_eq!(img.get_pixel(150, 100), &Rgb([255, 255, 255]));
        assert_eq!(img.get_pixel(40, 30), &Rgb([90, 120, 160]));
    }

    #[test]
    fn outside_the_card_becomes_white() {
        let mut img = on_lid((20, 15, 80, 50));
        assert!(whiten_outside_card(&mut img, 25)); // 25 dpi: a 2 px margin
        assert_eq!(
            img.get_pixel(180, 120),
            &Rgb([255, 255, 255]),
            "the speck outside goes"
        );
        assert_eq!(img.get_pixel(150, 100), &Rgb([255, 255, 255]));
        assert_eq!(
            img.get_pixel(40, 30),
            &Rgb([90, 120, 160]),
            "the card stays"
        );
        assert_eq!(
            img.get_pixel(18, 13),
            &Rgb([214, 214, 210]),
            "within the margin stays"
        );
    }

    #[test]
    fn outlines_start_on_the_cards_and_whiten_outside() {
        // An ID page's shape, a card in each half.
        let mut img = RgbImage::from_pixel(210, 297, Rgb([214, 214, 210]));
        for (x0, y0) in [(20u32, 20u32), (110, 170)] {
            for y in y0..y0 + 54 {
                for x in x0..x0 + 86 {
                    img.put_pixel(x, y, Rgb([90, 120, 160]));
                }
            }
        }
        let found = find_cards(&img, 25);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|(_, f)| *f));
        let top = found[0].0;
        assert!(top[0][0] < 20.0 / 210.0 && top[1][0] > 106.0 / 210.0, "{top:?}");
        // Keep only the front (a one-sided ID): the back half goes white.
        whiten_outside(&mut img, &[top]);
        assert_eq!(img.get_pixel(50, 40), &Rgb([90, 120, 160]));
        assert_eq!(img.get_pixel(150, 200), &Rgb([255, 255, 255]));
        assert_eq!(img.get_pixel(180, 100), &Rgb([255, 255, 255]));
    }

    #[test]
    fn a_tilted_outline_keeps_what_is_inside_it() {
        let mut img = RgbImage::from_pixel(100, 100, Rgb([0, 0, 0]));
        // A diamond: the middle stays, the corners go white.
        whiten_outside(&mut img, &[[[0.5, 0.1], [0.9, 0.5], [0.5, 0.9], [0.1, 0.5]]]);
        assert_eq!(img.get_pixel(50, 50), &Rgb([0, 0, 0]));
        assert_eq!(img.get_pixel(5, 5), &Rgb([255, 255, 255]));
        assert_eq!(img.get_pixel(95, 95), &Rgb([255, 255, 255]));
    }

    #[test]
    fn a_bright_white_lid_is_lid_too() {
        // Pure white and near-white (brighter than 225) around a card.
        let mut img = RgbImage::from_pixel(200, 140, Rgb([250, 250, 250]));
        for y in 15..65 {
            for x in 20..100 {
                img.put_pixel(x, y, Rgb([90, 120, 160]));
            }
        }
        img.put_pixel(150, 100, Rgb([255, 255, 255]));
        assert!(whiten_outside_card(&mut img, 25));
        assert_eq!(img.get_pixel(150, 120), &Rgb([255, 255, 255]));
        assert_eq!(img.get_pixel(40, 30), &Rgb([90, 120, 160]));
    }

    #[test]
    fn no_card_found_leaves_the_scan_alone() {
        let mut img = RgbImage::from_pixel(200, 140, Rgb([214, 214, 210]));
        let before = img.clone();
        assert!(!whiten_outside_card(&mut img, 25));
        assert_eq!(img, before);
    }

    #[test]
    fn a_saved_id_page_is_tidied_half_by_half() {
        // A4's shape, a card in each half, on a grey lid.
        let mut img = RgbImage::from_pixel(210, 297, Rgb([214, 214, 210]));
        for (x0, y0) in [(20u32, 20u32), (110, 170)] {
            for y in y0..y0 + 54 {
                for x in x0..x0 + 86 {
                    img.put_pixel(x, y, Rgb([90, 120, 160]));
                }
            }
        }
        let notes = tidy_image(&mut img, 25, Tidy { background: true, card: true });
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(img.get_pixel(50, 40), &Rgb([90, 120, 160]), "front card kept");
        assert_eq!(img.get_pixel(150, 200), &Rgb([90, 120, 160]), "back card kept");
        assert_eq!(img.get_pixel(180, 100), &Rgb([255, 255, 255]));
        assert_eq!(img.get_pixel(30, 250), &Rgb([255, 255, 255]));
    }

    #[test]
    fn a_whole_glass_scan_is_cut_to_its_half() {
        // The scanner ignored the half-page area and sent all of the glass,
        // with something in its lower part that must not reach the page.
        let front = side(82, 116, (5, 80), 0);
        let back = side(82, 58, (5, 5), 0);
        let sheet = page(&front, &back, Paper::A4).unwrap();
        assert!(at(&sheet, 10, 85) > 200);
    }
}
