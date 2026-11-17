//! Pixels packed the ways a VNC server packs them, for the test-pattern
//! server (`server.rs`) and the tests of the decoders. Each one writes
//! what `decode.rs` reads, in Kagaz's pixel layout (R, G, B, unused).

use crate::remote::screen::Pointer;
use flate2::{Compress, Compression, FlushCompress};

type Rgb = [u8; 3];

fn pixels(rgba: &[u8]) -> Vec<Rgb> {
    rgba.as_chunks::<4>().0.iter().map(|p| [p[0], p[1], p[2]]).collect()
}

/// The colours of some pixels in the order they first appear, or none
/// when there are more than `most`.
fn colours(px: &[Rgb], most: usize) -> Option<Vec<Rgb>> {
    let mut list: Vec<Rgb> = Vec::new();
    for p in px {
        if !list.contains(p) {
            if list.len() == most {
                return None;
            }
            list.push(*p);
        }
    }
    Some(list)
}

fn put_run(out: &mut Vec<u8>, run: usize) {
    let mut rest = run - 1;
    while rest >= 255 {
        out.push(255);
        rest -= 255;
    }
    out.push(rest as u8);
}

fn put_compact(out: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        out.push(len as u8);
    } else if len < 0x4000 {
        out.extend_from_slice(&[(len & 0x7f) as u8 | 0x80, (len >> 7) as u8]);
    } else {
        out.extend_from_slice(&[(len & 0x7f) as u8 | 0x80, ((len >> 7) & 0x7f) as u8 | 0x80, (len >> 14) as u8]);
    }
}

/// Pack `input` on a stream that runs on, ending where the other side can
/// unpack all of it.
fn deflate(z: &mut Compress, input: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(input.len() / 2 + 128);
    let mut at = 0;
    loop {
        if out.len() == out.capacity() {
            out.reserve(out.capacity());
        }
        let was = z.total_in();
        z.compress_vec(&input[at..], &mut out, FlushCompress::Sync)
            .expect("packing into memory does not fail");
        at += (z.total_in() - was) as usize;
        if at >= input.len() && out.len() < out.capacity() {
            return out;
        }
    }
}

pub fn raw(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for p in out.as_chunks_mut::<4>().0 {
        p[3] = 0;
    }
    out
}

/// A background (the first pixel's colour) and every run of another
/// colour as a rectangle one pixel high.
pub fn rre(rgba: &[u8], width: usize) -> Vec<u8> {
    let px = pixels(rgba);
    let background = px[0];
    let mut pieces = Vec::new();
    let mut count = 0u32;
    for (y, row) in px.chunks(width).enumerate() {
        let mut x = 0;
        while x < width {
            let colour = row[x];
            let run = row[x..].iter().take_while(|p| **p == colour).count();
            if colour != background {
                pieces.extend_from_slice(&[colour[0], colour[1], colour[2], 0]);
                for v in [x, y, run, 1] {
                    pieces.extend_from_slice(&(v as u16).to_be_bytes());
                }
                count += 1;
            }
            x += run;
        }
    }
    let mut out = count.to_be_bytes().to_vec();
    out.extend_from_slice(&[background[0], background[1], background[2], 0]);
    out.extend(pieces);
    out
}

/// How one ZRLE tile is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Raw,
    Solid,
    Palette,
    Runs,
    PaletteRuns,
}

/// One ZRLE tile packed one way, or none when that way cannot hold it.
pub fn zrle_tile(px: &[Rgb], width: usize, how: Tile) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match how {
        Tile::Raw => {
            out.push(0);
            for p in px {
                out.extend_from_slice(p);
            }
        }
        Tile::Solid => {
            if px.iter().any(|p| *p != px[0]) {
                return None;
            }
            out.push(1);
            out.extend_from_slice(&px[0]);
        }
        Tile::Palette => {
            let list = colours(px, 16)?;
            if list.len() < 2 {
                return None;
            }
            out.push(list.len() as u8);
            for c in &list {
                out.extend_from_slice(c);
            }
            let bits = match list.len() {
                2 => 1,
                3..=4 => 2,
                _ => 4,
            };
            for row in px.chunks(width) {
                let mut packed = vec![0u8; (row.len() * bits).div_ceil(8)];
                for (x, p) in row.iter().enumerate() {
                    let index = list.iter().position(|c| c == p).expect("in the list") as u8;
                    let at = x * bits;
                    packed[at / 8] |= index << (8 - bits - at % 8);
                }
                out.extend(packed);
            }
        }
        Tile::Runs => {
            out.push(128);
            let mut at = 0;
            while at < px.len() {
                let run = px[at..].iter().take_while(|p| **p == px[at]).count();
                out.extend_from_slice(&px[at]);
                put_run(&mut out, run);
                at += run;
            }
        }
        Tile::PaletteRuns => {
            let list = colours(px, 127)?;
            if list.len() < 2 {
                return None;
            }
            out.push(128 + list.len() as u8);
            for c in &list {
                out.extend_from_slice(c);
            }
            let mut at = 0;
            while at < px.len() {
                let run = px[at..].iter().take_while(|p| **p == px[at]).count();
                let index = list.iter().position(|c| *c == px[at]).expect("in the list") as u8;
                if run == 1 {
                    out.push(index);
                } else {
                    out.push(index | 128);
                    put_run(&mut out, run);
                }
                at += run;
            }
        }
    }
    Some(out)
}

/// One Hextile tile; `background` is what the tile before left behind.
fn hextile_tile(out: &mut Vec<u8>, px: &[Rgb], width: usize, background: &mut Option<Rgb>) {
    const RAW: u8 = 1;
    const BACKGROUND: u8 = 2;
    const FOREGROUND: u8 = 4;
    const PIECES: u8 = 8;
    const COLOURED: u8 = 16;
    let raw = |out: &mut Vec<u8>, background: &mut Option<Rgb>| {
        out.push(RAW);
        for p in px {
            out.extend_from_slice(&[p[0], p[1], p[2], 0]);
        }
        // After a raw tile nothing is carried over.
        *background = None;
    };
    let Some(list) = colours(px, 8) else {
        return raw(out, background);
    };
    // The background: the colour most of the tile has.
    let back = *list
        .iter()
        .max_by_key(|c| px.iter().filter(|p| p == c).count())
        .expect("a tile has pixels");
    // Every run of another colour, one pixel high.
    let mut runs: Vec<(Rgb, u8, u8)> = Vec::new();
    for (y, row) in px.chunks(width).enumerate() {
        let mut x = 0;
        while x < row.len() {
            let run = row[x..].iter().take_while(|p| **p == row[x]).count();
            if row[x] != back {
                runs.push((row[x], ((x as u8) << 4) | y as u8, ((run as u8 - 1) << 4)));
            }
            x += run;
        }
    }
    if runs.len() > 255 || runs.len() * 6 + 6 > px.len() * 4 {
        return raw(out, background);
    }
    let mut kind = 0;
    if *background != Some(back) {
        kind |= BACKGROUND;
    }
    let two = list.len() == 2;
    if !runs.is_empty() {
        kind |= PIECES | if two { FOREGROUND } else { COLOURED };
    }
    out.push(kind);
    if kind & BACKGROUND != 0 {
        out.extend_from_slice(&[back[0], back[1], back[2], 0]);
    }
    *background = Some(back);
    if runs.is_empty() {
        return;
    }
    if two {
        let fore = runs[0].0;
        out.extend_from_slice(&[fore[0], fore[1], fore[2], 0]);
    }
    out.push(runs.len() as u8);
    for (colour, xy, wh) in runs {
        if !two {
            out.extend_from_slice(&[colour[0], colour[1], colour[2], 0]);
        }
        out.extend_from_slice(&[xy, wh]);
    }
}

pub fn hextile(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let px = pixels(rgba);
    let mut out = Vec::new();
    let mut background = None;
    for ty in (0..height).step_by(16) {
        for tx in (0..width).step_by(16) {
            let (tw, th) = ((width - tx).min(16), (height - ty).min(16));
            let tile: Vec<Rgb> = (0..th)
                .flat_map(|y| px[(ty + y) * width + tx..(ty + y) * width + tx + tw].iter().copied())
                .collect();
            hextile_tile(&mut out, &tile, tw, &mut background);
        }
    }
    out
}

/// How one Tight area is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tight {
    Fill,
    Copy,
    Palette,
    Gradient,
    /// JPEG at this quality, 1 to 100.
    Jpeg(u8),
}

/// What carries over from one area to the next within a connection.
pub struct Encoder {
    zrle: Compress,
    tight: [Compress; 4],
    /// Pack many-coloured Tight areas as differences, not as they are.
    pub gradient: bool,
}

impl Default for Encoder {
    fn default() -> Encoder {
        Encoder {
            zrle: Compress::new(Compression::fast(), true),
            tight: std::array::from_fn(|_| Compress::new(Compression::fast(), true)),
            gradient: false,
        }
    }
}

impl Encoder {
    /// Each tile the smallest way that holds it.
    pub fn zrle(&mut self, rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
        let px = pixels(rgba);
        let mut tiles = Vec::new();
        for ty in (0..height).step_by(64) {
            for tx in (0..width).step_by(64) {
                let (tw, th) = ((width - tx).min(64), (height - ty).min(64));
                let tile: Vec<Rgb> = (0..th)
                    .flat_map(|y| px[(ty + y) * width + tx..(ty + y) * width + tx + tw].iter().copied())
                    .collect();
                let best = [Tile::Solid, Tile::Palette, Tile::PaletteRuns, Tile::Runs, Tile::Raw]
                    .iter()
                    .filter_map(|how| zrle_tile(&tile, tw, *how))
                    .min_by_key(|t| t.len())
                    .expect("raw holds any tile");
                tiles.extend(best);
            }
        }
        self.zrle_packed(&tiles)
    }

    /// Tiles already packed, on the connection's stream with their length.
    pub fn zrle_packed(&mut self, tiles: &[u8]) -> Vec<u8> {
        let packed = deflate(&mut self.zrle, tiles);
        let mut out = (packed.len() as u32).to_be_bytes().to_vec();
        out.extend(packed);
        out
    }

    /// The way that suits the area: one colour, a palette, or all of its
    /// pixels (as JPEG when a quality is given).
    pub fn tight(&mut self, rgba: &[u8], width: usize, height: usize, jpeg: Option<u8>) -> Vec<u8> {
        let px = pixels(rgba);
        let how = match colours(&px, 256) {
            Some(list) if list.len() == 1 => Tight::Fill,
            Some(_) => Tight::Palette,
            None => match jpeg {
                Some(quality) if width >= 16 && height >= 16 => Tight::Jpeg(quality),
                _ if self.gradient => Tight::Gradient,
                _ => Tight::Copy,
            },
        };
        self.tight_as(rgba, width, height, how).expect("the way was chosen to fit")
    }

    /// An area packed one way, or none when that way cannot hold it.
    pub fn tight_as(&mut self, rgba: &[u8], width: usize, height: usize, how: Tight) -> Option<Vec<u8>> {
        let px = pixels(rgba);
        let mut out = Vec::new();
        match how {
            Tight::Fill => {
                if px.iter().any(|p| *p != px[0]) {
                    return None;
                }
                out.push(0x80);
                out.extend_from_slice(&px[0]);
            }
            Tight::Jpeg(quality) => {
                let rgb: Vec<u8> = px.iter().flatten().copied().collect();
                let mut jpeg = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
                    .encode(&rgb, width as u32, height as u32, image::ExtendedColorType::Rgb8)
                    .ok()?;
                out.push(0x90);
                put_compact(&mut out, jpeg.len());
                out.extend(jpeg);
            }
            Tight::Copy => {
                out.push(0x00);
                let data: Vec<u8> = px.iter().flatten().copied().collect();
                self.tight_data(&mut out, 0, &data);
            }
            Tight::Palette => {
                let list = colours(&px, 256)?;
                if list.len() < 2 {
                    return None;
                }
                let stream = if list.len() == 2 { 1 } else { 2 };
                out.extend_from_slice(&[0x40 | (stream as u8) << 4, 1, (list.len() - 1) as u8]);
                for c in &list {
                    out.extend_from_slice(c);
                }
                let index = |p: &Rgb| list.iter().position(|c| c == p).expect("in the list") as u8;
                let data: Vec<u8> = if list.len() == 2 {
                    let mut bits = Vec::new();
                    for row in px.chunks(width) {
                        let mut packed = vec![0u8; width.div_ceil(8)];
                        for (x, p) in row.iter().enumerate() {
                            packed[x / 8] |= index(p) << (7 - x % 8);
                        }
                        bits.extend(packed);
                    }
                    bits
                } else {
                    px.iter().map(index).collect()
                };
                self.tight_data(&mut out, stream, &data);
            }
            Tight::Gradient => {
                out.extend_from_slice(&[0x40 | 3 << 4, 2]);
                let mut data = Vec::with_capacity(px.len() * 3);
                for y in 0..height {
                    for x in 0..width {
                        // What is to the left, above, and above to the left.
                        let at = |x: usize, y: usize| px[y * width + x];
                        let left = if x > 0 { at(x - 1, y) } else { [0; 3] };
                        let up = if y > 0 { at(x, y - 1) } else { [0; 3] };
                        let up_left = if x > 0 && y > 0 { at(x - 1, y - 1) } else { [0; 3] };
                        for (c, value) in at(x, y).into_iter().enumerate() {
                            let predicted = (left[c] as i32 + up[c] as i32 - up_left[c] as i32).clamp(0, 255);
                            data.push((value as i32 - predicted) as u8);
                        }
                    }
                }
                self.tight_data(&mut out, 3, &data);
            }
        }
        Some(out)
    }

    fn tight_data(&mut self, out: &mut Vec<u8>, stream: usize, data: &[u8]) {
        if data.len() < 12 {
            out.extend_from_slice(data);
            return;
        }
        let packed = deflate(&mut self.tight[stream], data);
        put_compact(out, packed.len());
        out.extend(packed);
    }
}

/// The pointer's shape: its pixels, then one bit each for where it is.
pub fn cursor(pointer: &Pointer) -> Vec<u8> {
    let (width, height) = (pointer.width as usize, pointer.height as usize);
    let mut out = raw(&pointer.rgba);
    let row_len = width.div_ceil(8);
    let mut mask = vec![0u8; row_len * height];
    for y in 0..height {
        for x in 0..width {
            if pointer.rgba[(y * width + x) * 4 + 3] != 0 {
                mask[y * row_len + x / 8] |= 1 << (7 - x % 8);
            }
        }
    }
    out.extend(mask);
    out
}

#[cfg(test)]
mod tests {
    use super::super::decode::{self, Decoded, Decoder};
    use super::*;
    use crate::remote::pattern;

    /// The test card, and a few awkward sizes of it.
    fn cards() -> Vec<(usize, usize, Vec<u8>)> {
        [(320, 200), (65, 17), (1, 1), (129, 70), (16, 16), (7, 300)]
            .iter()
            .map(|(w, h)| (*w, *h, pattern::card(*w as u16, *h as u16, 3)))
            .collect()
    }

    fn opaque(mut rgba: Vec<u8>) -> Vec<u8> {
        for p in rgba.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        rgba
    }

    #[test]
    fn raw_rre_and_hextile_come_back_the_same() {
        let mut d = Decoder::default();
        for (w, h, card) in cards() {
            assert_eq!(d.raw(&mut &raw(&card)[..], w, h).unwrap(), card, "raw {w}x{h}");
            assert_eq!(d.rre(&mut &rre(&card, w)[..], w, h).unwrap(), card, "rre {w}x{h}");
            let packed = hextile(&card, w, h);
            let mut src = &packed[..];
            assert_eq!(d.hextile(&mut src, w, h).unwrap(), card, "hextile {w}x{h}");
            assert!(src.is_empty(), "hextile {w}x{h} left bytes over");
        }
    }

    #[test]
    fn hextile_uses_every_kind_of_tile() {
        let card = pattern::card(320, 200, 0);
        let packed = hextile(&card, 320, 200);
        // Smaller than raw: the flat and striped areas packed.
        assert!(packed.len() < card.len() / 2, "{} of {}", packed.len(), card.len());
    }

    #[test]
    fn zrle_comes_back_the_same_over_many_areas() {
        let mut e = Encoder::default();
        let mut d = Decoder::default();
        // One stream for the connection: areas one after another.
        for round in 0..3 {
            for (w, h, card) in cards() {
                let packed = e.zrle(&card, w, h);
                let mut src = &packed[..];
                assert_eq!(d.zrle(&mut src, w, h).unwrap(), card, "zrle {w}x{h} round {round}");
                assert!(src.is_empty());
            }
        }
    }

    #[test]
    fn every_kind_of_zrle_tile() {
        let mut e = Encoder::default();
        let mut d = Decoder::default();
        // Tiles of 1, 2, 4, 16, 100 and many colours, 64 by 64 and smaller.
        for (tw, th) in [(64usize, 64usize), (10, 3), (1, 64), (33, 5)] {
            for colours in [1usize, 2, 3, 4, 5, 16, 17, 100, 127, 128, 4000] {
                let px: Vec<Rgb> = (0..tw * th)
                    .map(|i| {
                        // Runs of a few pixels over `colours` colours.
                        let c = (i / 3 * 7919) % colours;
                        [(c % 256) as u8, (c / 256) as u8, 77]
                    })
                    .collect();
                let rgba: Vec<u8> = px.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
                for how in [Tile::Raw, Tile::Solid, Tile::Palette, Tile::Runs, Tile::PaletteRuns] {
                    let Some(tile) = zrle_tile(&px, tw, how) else { continue };
                    let packed = e.zrle_packed(&tile);
                    let got = d.zrle(&mut &packed[..], tw, th).unwrap();
                    assert_eq!(got, rgba, "{how:?} {tw}x{th} with {colours} colours");
                }
            }
        }
    }

    #[test]
    fn every_kind_of_tight_area() {
        let mut e = Encoder::default();
        let mut d = Decoder::default();
        let two: Vec<u8> = (0..37 * 9).flat_map(|i| if i % 3 == 0 { [9, 9, 9, 255] } else { [200, 1, 2, 255] }).collect();
        let few: Vec<u8> = (0..37 * 9).flat_map(|i| [(i % 40) as u8, 0, 5, 255]).collect();
        let one: Vec<u8> = [4, 5, 6, 255].repeat(37 * 9);
        let tiny: Vec<u8> = vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255];
        let card = pattern::card(150, 90, 1);
        let cases: [(&[u8], usize, usize, Tight); 9] = [
            (&one, 37, 9, Tight::Fill),
            (&two, 37, 9, Tight::Palette),
            (&few, 37, 9, Tight::Palette),
            (&few, 37, 9, Tight::Copy),
            (&few, 37, 9, Tight::Gradient),
            (&card, 150, 90, Tight::Copy),
            (&card, 150, 90, Tight::Gradient),
            // Too few bytes to be worth packing: sent as they are.
            (&tiny, 3, 1, Tight::Copy),
            (&tiny, 3, 1, Tight::Gradient),
        ];
        // Twice over, so that each stream carries on from its last area.
        for round in 0..2 {
            for (rgba, w, h, how) in cases {
                let packed = e.tight_as(rgba, w, h, how).unwrap();
                let mut src = &packed[..];
                let got = match d.tight(&mut src, w, h).unwrap() {
                    Decoded::Pixels(p) => p,
                    Decoded::Fill(c) => [c[0], c[1], c[2], 255].repeat(w * h),
                };
                assert_eq!(got, opaque(rgba.to_vec()), "{how:?} {w}x{h} round {round}");
                assert!(src.is_empty(), "{how:?} left bytes over");
            }
        }
    }

    #[test]
    fn tight_jpeg_is_close_to_the_picture() {
        let mut e = Encoder::default();
        let mut d = Decoder::default();
        // A smooth picture: JPEG's own kind.
        let (w, h) = (96usize, 64usize);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % w * 2) as u8, (i / w * 3) as u8, 128, 255])
            .collect();
        let packed = e.tight_as(&rgba, w, h, Tight::Jpeg(90)).unwrap();
        let Decoded::Pixels(got) = d.tight(&mut &packed[..], w, h).unwrap() else { panic!() };
        let worst = got.iter().zip(&rgba).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst < 24, "off by {worst}");
        assert!(packed.len() < rgba.len() / 4);
    }

    #[test]
    fn tight_chooses_by_what_the_area_holds() {
        let mut e = Encoder::default();
        let card = pattern::card(64, 64, 0);
        let one: Vec<u8> = [4, 5, 6, 255].repeat(64);
        assert_eq!(e.tight(&one, 8, 8, None)[0] >> 4, 8);
        let noisy: Vec<u8> = (0..64u32 * 64).flat_map(|i| [(i * 7) as u8, ((i * 13) >> 3) as u8, (i >> 4) as u8, 255]).collect();
        assert_eq!(e.tight(&noisy, 64, 64, Some(80))[0] >> 4, 9);
        assert_eq!(e.tight(&noisy, 64, 64, None)[0] >> 4, 0);
        let _ = card;
    }

    #[test]
    fn the_pointer_shape_comes_back_the_same() {
        let pointer = Pointer {
            width: 11,
            height: 5,
            hot_x: 2,
            hot_y: 3,
            rgba: (0..11 * 5).flat_map(|i| [i as u8, 2, 3, if i % 3 == 0 { 0 } else { 255 }]).collect(),
        };
        let packed = cursor(&pointer);
        let got = decode::cursor(&mut &packed[..], 11, 5, 2, 3).unwrap();
        // Where the pointer is not, its colour does not matter.
        for (a, b) in got.rgba.chunks(4).zip(pointer.rgba.chunks(4)) {
            assert_eq!(a[3], b[3]);
            if a[3] != 0 {
                assert_eq!(a, b);
            }
        }
        assert_eq!((got.hot_x, got.hot_y), (2, 3));
    }

    #[test]
    fn broken_data_is_an_error_not_a_crash() {
        let mut d = Decoder::default();
        // A ZRLE block that is not zlib.
        let mut junk = 5u32.to_be_bytes().to_vec();
        junk.extend_from_slice(b"hello");
        assert!(d.zrle(&mut &junk[..], 10, 10).is_err());
        // Cut short.
        assert!(Decoder::default().raw(&mut &[0u8; 7][..], 2, 1).is_err());
        // An area no screen has.
        assert!(Decoder::default().raw(&mut &[0u8; 8][..], 60000, 60000).is_err());
        // A Hextile piece that leaves its tile.
        let tile = [2 | 8 | 16, 0, 0, 0, 0, 1, 9, 9, 9, 0, 0xff, 0xff];
        assert!(Decoder::default().hextile(&mut &tile[..], 4, 4).is_err());
        // A Tight filter that does not exist.
        assert!(Decoder::default().tight(&mut &[0x40u8, 7][..], 4, 4).is_err());
    }
}
