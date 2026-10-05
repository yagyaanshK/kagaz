//! PWG Raster (PWG 5102.4), the page format every IPP Everywhere printer
//! accepts: a sync word, then per page a fixed 1796-byte header and the
//! pixels, run-length compressed. Big-endian throughout.

/// One page ready to be encoded: 8-bit pixels, row-major, no padding.
pub struct RasterPage<'a> {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    /// 1 for sgray_8 (0 = black), 3 for srgb_8.
    pub components: u32,
    pub pixels: &'a [u8],
    /// Page size in points, as the printer reports its media.
    pub width_pt: u32,
    pub height_pt: u32,
    /// IPP media keyword, e.g. "iso_a4_210x297mm".
    pub media: &'a str,
    /// 0 one-sided, 1 two-sided.
    pub duplex: bool,
    /// Flip the back side on the short edge.
    pub tumble: bool,
}

pub const HEADER_LEN: usize = 1796;
const COLOR_SPACE_SGRAY: u32 = 18;
const COLOR_SPACE_SRGB: u32 = 19;

/// Encode a whole document.
pub fn encode(pages: &[RasterPage]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"RaS2");
    for page in pages {
        out.extend_from_slice(&header(page, pages.len() as u32));
        compress_into(&mut out, page);
    }
    out
}

fn header(p: &RasterPage, total_pages: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(HEADER_LEN);
    let text = |h: &mut Vec<u8>, s: &str, len: usize| {
        let bytes = s.as_bytes();
        let n = bytes.len().min(len - 1);
        h.extend_from_slice(&bytes[..n]);
        h.extend(std::iter::repeat_n(0u8, len - n));
    };
    let u = |h: &mut Vec<u8>, v: u32| h.extend_from_slice(&v.to_be_bytes());
    let zero = |h: &mut Vec<u8>, n: usize| h.extend(std::iter::repeat_n(0u8, n));

    text(&mut h, "PWG Raster", 64); // PwgRaster
    text(&mut h, "", 64); // MediaColor
    text(&mut h, "", 64); // MediaType
    text(&mut h, "", 64); // PrintContentOptimize
    zero(&mut h, 12);
    u(&mut h, 0); // CutMedia
    u(&mut h, p.duplex as u32); // Duplex
    u(&mut h, p.dpi); // HWResolution x
    u(&mut h, p.dpi); // HWResolution y
    zero(&mut h, 16);
    u(&mut h, 0); // InsertSheet
    u(&mut h, 0); // Jog
    u(&mut h, 0); // LeadingEdge
    zero(&mut h, 12);
    u(&mut h, 0); // MediaPosition (auto)
    u(&mut h, 0); // MediaWeight
    zero(&mut h, 8);
    u(&mut h, 1); // NumCopies
    u(&mut h, 0); // Orientation
    zero(&mut h, 4);
    u(&mut h, p.width_pt); // PageSize
    u(&mut h, p.height_pt);
    zero(&mut h, 8);
    u(&mut h, p.tumble as u32); // Tumble
    u(&mut h, p.width); // Width
    u(&mut h, p.height); // Height
    zero(&mut h, 4);
    u(&mut h, 8); // BitsPerColor
    u(&mut h, 8 * p.components); // BitsPerPixel
    u(&mut h, p.width * p.components); // BytesPerLine
    u(&mut h, 0); // ColorOrder chunky
    u(
        &mut h,
        if p.components == 1 {
            COLOR_SPACE_SGRAY
        } else {
            COLOR_SPACE_SRGB
        },
    );
    zero(&mut h, 16);
    u(&mut h, p.components); // NumColors
    zero(&mut h, 28);
    u(&mut h, total_pages); // TotalPageCount
    u(&mut h, 1); // CrossFeedTransform
    u(&mut h, 1); // FeedTransform
    u(&mut h, 0); // ImageBoxLeft
    u(&mut h, 0); // ImageBoxTop
    u(&mut h, 0); // ImageBoxRight
    u(&mut h, 0); // ImageBoxBottom
    u(&mut h, 0); // AlternatePrimary
    u(&mut h, 0); // PrintQuality (default)
    zero(&mut h, 20);
    u(&mut h, 0); // VendorIdentifier
    u(&mut h, 0); // VendorLength
    zero(&mut h, 1088); // VendorData
    zero(&mut h, 64);
    text(&mut h, "", 64); // RenderingIntent
    text(&mut h, p.media, 64); // PageSizeName
    debug_assert_eq!(h.len(), HEADER_LEN);
    h
}

/// PWG compression: a line-repeat byte, then runs of either repeated pixels
/// (count byte 0..=127 means count+1 copies of one pixel) or literal pixels
/// (count byte 129..=255 means 257-count pixels follow).
fn compress_into(out: &mut Vec<u8>, p: &RasterPage) {
    let bpp = p.components as usize;
    let stride = p.width as usize * bpp;
    let mut y = 0usize;
    let rows = p.height as usize;
    while y < rows {
        let line = &p.pixels[y * stride..(y + 1) * stride];
        // Collapse identical following lines, up to 256 in one group.
        let mut repeat = 0usize;
        while y + repeat + 1 < rows
            && repeat < 255
            && &p.pixels[(y + repeat + 1) * stride..(y + repeat + 2) * stride] == line
        {
            repeat += 1;
        }
        out.push(repeat as u8);
        compress_line(out, line, bpp);
        y += repeat + 1;
    }
}

fn compress_line(out: &mut Vec<u8>, line: &[u8], bpp: usize) {
    let pixels = line.len() / bpp;
    let px = |i: usize| &line[i * bpp..(i + 1) * bpp];
    let mut i = 0;
    while i < pixels {
        // How many times does this pixel repeat?
        let mut run = 1;
        while i + run < pixels && run < 128 && px(i + run) == px(i) {
            run += 1;
        }
        if run > 1 {
            out.push((run - 1) as u8);
            out.extend_from_slice(px(i));
            i += run;
            continue;
        }
        // Literal stretch: until a pixel repeats, up to 128.
        let start = i;
        let mut n = 1;
        while i + n < pixels && n < 128 && px(i + n) != px(i + n - 1) {
            n += 1;
        }
        // Do not swallow the first pixel of a following run.
        if i + n < pixels && n > 1 && px(i + n) == px(i + n - 1) {
            n -= 1;
        }
        out.push((257 - n) as u8);
        out.extend_from_slice(&line[start * bpp..(start + n) * bpp]);
        i += n;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A decoder for the tests only.
    pub(crate) fn decompress(data: &[u8], width: usize, height: usize, bpp: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(width * height * bpp);
        let mut pos = 0;
        while out.len() < width * height * bpp {
            let repeat = data[pos] as usize + 1;
            pos += 1;
            let mut line = Vec::with_capacity(width * bpp);
            while line.len() < width * bpp {
                let c = data[pos];
                pos += 1;
                if c < 128 {
                    let px = &data[pos..pos + bpp];
                    for _ in 0..(c as usize + 1) {
                        line.extend_from_slice(px);
                    }
                    pos += bpp;
                } else {
                    let n = 257 - c as usize;
                    line.extend_from_slice(&data[pos..pos + n * bpp]);
                    pos += n * bpp;
                }
            }
            for _ in 0..repeat {
                out.extend_from_slice(&line);
            }
        }
        assert_eq!(pos, data.len(), "trailing bytes");
        out
    }

    fn page<'a>(w: u32, h: u32, comps: u32, pixels: &'a [u8]) -> RasterPage<'a> {
        RasterPage {
            width: w,
            height: h,
            dpi: 300,
            components: comps,
            pixels,
            width_pt: 595,
            height_pt: 842,
            media: "iso_a4_210x297mm",
            duplex: false,
            tumble: false,
        }
    }

    #[test]
    fn header_is_1796_bytes_with_the_right_fields() {
        let pixels = vec![255u8; 4];
        let doc = encode(&[page(2, 2, 1, &pixels)]);
        assert_eq!(&doc[..4], b"RaS2");
        let h = &doc[4..4 + HEADER_LEN];
        assert_eq!(&h[..10], b"PWG Raster");
        let u = |off: usize| u32::from_be_bytes([h[off], h[off + 1], h[off + 2], h[off + 3]]);
        assert_eq!(u(276), 300); // HWResolution
        assert_eq!(u(352), 595); // PageSize width
        assert_eq!(u(372), 2); // Width
        assert_eq!(u(376), 2); // Height
        assert_eq!(u(384), 8); // BitsPerColor
        assert_eq!(u(388), 8); // BitsPerPixel
        assert_eq!(u(392), 2); // BytesPerLine
        assert_eq!(u(400), COLOR_SPACE_SGRAY);
        assert_eq!(u(420), 1); // NumColors
        assert_eq!(u(452), 1); // TotalPageCount
        assert_eq!(&h[1732..1748], b"iso_a4_210x297mm");
        // Two white lines collapse into one group: repeat=1, run of 2 pixels.
        assert_eq!(&doc[4 + HEADER_LEN..], &[1, 1, 255]);
    }

    #[test]
    fn compression_round_trips_mixed_content() {
        let (w, h) = (300usize, 7usize);
        let mut pixels = vec![255u8; w * h];
        for x in 0..w {
            pixels[2 * w + x] = (x * 7 % 251) as u8; // noisy line, mostly literals
            pixels[3 * w + x] = if x % 10 < 5 { 0 } else { 255 }; // short runs
        }
        let line3 = pixels[3 * w..4 * w].to_vec();
        pixels[5 * w..6 * w].copy_from_slice(&line3); // not adjacent, no collapse
        let p = page(w as u32, h as u32, 1, &pixels);
        let doc = encode(&[p]);
        let back = decompress(&doc[4 + HEADER_LEN..], w, h, 1);
        assert_eq!(back, pixels);

        // Colour, with runs longer than 128.
        let mut rgb = vec![0u8; 300 * 3 * 2];
        for px in rgb[300 * 3..].chunks_mut(3) {
            px.copy_from_slice(&[10, 20, 30]);
        }
        let p = page(300, 2, 3, &rgb);
        let doc = encode(&[p]);
        assert_eq!(decompress(&doc[4 + HEADER_LEN..], 300, 2, 3), rgb);
    }
}
