//! Both sides of an ID card (or a passport page) on one page, the way a
//! printer's "2 in 1 ID copy" does it: each side is a scan of the top half
//! of the glass (`ScanRequest::half_page`), and the page is the front's half
//! above the back's, each just as it lay on the glass. Nothing looks for the
//! card: wherever it sat in that half is where it shows on the page.

use super::{ColorMode, Page, Paper};
use image::{imageops, DynamicImage, ImageFormat, Rgb, RgbImage};
use std::io::Cursor;

/// The page: `front` in the top half, `back` in the bottom half, at the
/// front's resolution. A side bigger than its half (a scanner that sent the
/// whole glass) is cut to the half; a smaller one keeps its place at the
/// top left, the glass's corner.
pub fn page(front: &Page, back: &Page, paper: Paper) -> Result<Page, String> {
    let (w_mils, h_mils) = paper.size_mils().unwrap_or((8268, 11693));
    let dpi = front.dpi.max(1);
    let px = |mils: u32| ((mils as u64 * dpi as u64) / 1000) as u32;
    let (width, height) = (px(w_mils), px(h_mils));
    let half = height / 2;

    let mut sheet = RgbImage::from_pixel(width, height, Rgb([255, 255, 255]));
    for (side, top, name) in [(front, 0, "front"), (back, half, "back")] {
        let mut img = decode(side).map_err(|e| format!("the {name} scan could not be read: {e}"))?;
        // The same size on paper even if the back came at another resolution.
        if side.dpi.max(1) != dpi {
            let scale = dpi as f64 / side.dpi.max(1) as f64;
            let (w, h) = (img.width() as f64 * scale, img.height() as f64 * scale);
            img = img.resize_exact(w.round() as u32, h.round() as u32, imageops::FilterType::Triangle);
        }
        let img = img.crop_imm(0, 0, img.width().min(width), img.height().min(half)).to_rgb8();
        imageops::replace(&mut sheet, &img, 0, top as i64);
    }

    // Colour scans as JPEG; black-and-white (text, lines) as PNG, sharp.
    let color = if front.color == back.color { front.color } else { ColorMode::Color };
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
    Ok(Page { data, mime, dpi, color })
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
        DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut data), ImageFormat::Png).unwrap();
        Page { data, mime: "image/png", dpi: 10, color: ColorMode::Color }
    }

    fn at(p: &Page, x: u32, y: u32) -> u8 {
        image::load_from_memory(&p.data).unwrap().to_luma8().get_pixel(x, y).0[0]
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
        assert!(at(&sheet, 45, 58 + 15) < 60, "back card where it lay, in the lower half");
        assert!(at(&sheet, 45, 15) > 200, "nothing of the back in the top half");
        assert!(at(&sheet, 10, 58 + 10) > 200, "nothing of the front in the lower half");
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
