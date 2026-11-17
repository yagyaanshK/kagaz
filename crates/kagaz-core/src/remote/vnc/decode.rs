//! What a VNC server sends for an area, turned into pixels.
//!
//! Kagaz asks every server for the same pixel layout (32 bits, red in the
//! lowest byte), so a pixel on the wire is the bytes R, G, B and one unused;
//! the encodings that drop the unused byte (ZRLE, Tight) send R, G, B.

use crate::remote::screen::Pointer;
use flate2::{Decompress, FlushDecompress, Status};
use std::io::{self, Read};

pub const RAW: i32 = 0;
pub const COPY_RECT: i32 = 1;
pub const RRE: i32 = 2;
pub const HEXTILE: i32 = 5;
pub const TIGHT: i32 = 7;
pub const ZRLE: i32 = 16;
/// Not pictures: the pointer's shape, a new screen size, "no more areas in
/// this update", and clipboard text beyond Latin-1.
pub const CURSOR: i32 = -239;
pub const DESKTOP_SIZE: i32 = -223;
pub const LAST_RECT: i32 = -224;
pub const EXTENDED_CLIPBOARD: i32 = 0xC0A1_E5CE_u32 as i32;
/// Tight's JPEG quality, from this (worst) up to nine more (best).
pub const JPEG_QUALITY_0: i32 = -32;

/// No area is larger than this many pixels (8192 by 8192).
const MAX_PIXELS: usize = 1 << 26;
/// No packed block is larger than this.
const MAX_PACKED: usize = 1 << 28;

pub fn bad(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

pub fn u8_of(r: &mut impl Read) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

pub fn u16_of(r: &mut impl Read) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

pub fn u32_of(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

pub fn bytes_of(r: &mut impl Read, len: usize) -> io::Result<Vec<u8>> {
    if len > MAX_PACKED {
        return Err(bad("a block larger than any screen needs"));
    }
    let mut b = vec![0u8; len];
    r.read_exact(&mut b)?;
    Ok(b)
}

fn rgb_of(r: &mut impl Read) -> io::Result<[u8; 3]> {
    let mut b = [0u8; 3];
    r.read_exact(&mut b)?;
    Ok(b)
}

/// A whole pixel: R, G, B and the unused byte.
fn pixel_of(r: &mut impl Read) -> io::Result<[u8; 3]> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok([b[0], b[1], b[2]])
}

fn area(width: usize, height: usize) -> io::Result<usize> {
    let n = width * height;
    if n > MAX_PIXELS {
        return Err(bad(format!("an area of {width} by {height} is larger than any screen")));
    }
    Ok(n)
}

/// The pixels of an area being put together.
struct Canvas {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> io::Result<Canvas> {
        Ok(Canvas {
            width,
            height,
            rgba: vec![255; area(width, height)? * 4],
        })
    }

    fn set(&mut self, x: usize, y: usize, rgb: [u8; 3]) {
        let at = (y * self.width + x) * 4;
        self.rgba[at..at + 3].copy_from_slice(&rgb);
    }

    fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, rgb: [u8; 3]) -> io::Result<()> {
        if x + width > self.width || y + height > self.height {
            return Err(bad("a piece outside its area"));
        }
        for row in y..y + height {
            let at = (row * self.width + x) * 4;
            for p in self.rgba[at..at + width * 4].as_chunks_mut::<4>().0 {
                p[..3].copy_from_slice(&rgb);
            }
        }
        Ok(())
    }
}

/// Unpack `input` on a stream that runs on from the blocks before it.
fn inflate(z: &mut Decompress, input: &[u8]) -> io::Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity((input.len() * 4).max(4096));
    let mut at = 0;
    loop {
        if out.len() == out.capacity() {
            if out.len() > MAX_PACKED {
                return Err(bad("a packed block that unpacks to more than any screen needs"));
            }
            out.reserve(out.capacity());
        }
        let (was_in, was_out) = (z.total_in(), z.total_out());
        let status = z
            .decompress_vec(&input[at..], &mut out, FlushDecompress::Sync)
            .map_err(|e| bad(format!("packed data that does not unpack: {e}")))?;
        let took = (z.total_in() - was_in) as usize;
        let gave = z.total_out() - was_out;
        at += took;
        let room = out.len() < out.capacity();
        if status == Status::StreamEnd || (at >= input.len() && room) || (took == 0 && gave == 0) {
            return Ok(out);
        }
    }
}

/// A length in one to three bytes, seven bits in each of the first two.
fn compact_length(r: &mut impl Read) -> io::Result<usize> {
    let a = u8_of(r)? as usize;
    if a & 0x80 == 0 {
        return Ok(a);
    }
    let b = u8_of(r)? as usize;
    if b & 0x80 == 0 {
        return Ok((a & 0x7f) | (b << 7));
    }
    let c = u8_of(r)? as usize;
    Ok((a & 0x7f) | ((b & 0x7f) << 7) | (c << 14))
}

/// A run's length: bytes added up until one is not 255, plus one.
fn run_length(src: &mut &[u8]) -> io::Result<usize> {
    let mut n = 1;
    loop {
        let b = u8_of(src)? as usize;
        n += b;
        if b != 255 {
            return Ok(n);
        }
    }
}

/// What an area decodes to.
pub enum Decoded {
    /// `width * height * 4` bytes of RGBA.
    Pixels(Vec<u8>),
    /// One colour all over.
    Fill([u8; 3]),
}

/// What carries over from one area to the next within a connection.
pub struct Decoder {
    zrle: Decompress,
    tight: [Decompress; 4],
    background: [u8; 3],
    foreground: [u8; 3],
}

impl Default for Decoder {
    fn default() -> Decoder {
        Decoder {
            zrle: Decompress::new(true),
            tight: std::array::from_fn(|_| Decompress::new(true)),
            background: [0; 3],
            foreground: [0; 3],
        }
    }
}

impl Decoder {
    pub fn raw(&mut self, r: &mut impl Read, width: usize, height: usize) -> io::Result<Vec<u8>> {
        let mut rgba = bytes_of(r, area(width, height)? * 4)?;
        for p in rgba.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        Ok(rgba)
    }

    /// A background and rectangles of one colour each on it.
    pub fn rre(&mut self, r: &mut impl Read, width: usize, height: usize) -> io::Result<Vec<u8>> {
        let count = u32_of(r)?;
        let mut canvas = Canvas::new(width, height)?;
        let background = pixel_of(r)?;
        canvas.fill(0, 0, width, height, background)?;
        for _ in 0..count {
            let colour = pixel_of(r)?;
            let (x, y, w, h) = (u16_of(r)?, u16_of(r)?, u16_of(r)?, u16_of(r)?);
            canvas.fill(x as usize, y as usize, w as usize, h as usize, colour)?;
        }
        Ok(canvas.rgba)
    }

    /// Tiles of 16 by 16: raw, or a background with rectangles on it.
    pub fn hextile(&mut self, r: &mut impl Read, width: usize, height: usize) -> io::Result<Vec<u8>> {
        const RAW: u8 = 1;
        const BACKGROUND: u8 = 2;
        const FOREGROUND: u8 = 4;
        const PIECES: u8 = 8;
        const COLOURED: u8 = 16;
        let mut canvas = Canvas::new(width, height)?;
        for ty in (0..height).step_by(16) {
            for tx in (0..width).step_by(16) {
                let (tw, th) = ((width - tx).min(16), (height - ty).min(16));
                let kind = u8_of(r)?;
                if kind & RAW != 0 {
                    for y in 0..th {
                        for x in 0..tw {
                            let p = pixel_of(r)?;
                            canvas.set(tx + x, ty + y, p);
                        }
                    }
                    continue;
                }
                if kind & BACKGROUND != 0 {
                    self.background = pixel_of(r)?;
                }
                canvas.fill(tx, ty, tw, th, self.background)?;
                if kind & FOREGROUND != 0 {
                    self.foreground = pixel_of(r)?;
                }
                if kind & PIECES == 0 {
                    continue;
                }
                for _ in 0..u8_of(r)? {
                    let colour = if kind & COLOURED != 0 { pixel_of(r)? } else { self.foreground };
                    let (xy, wh) = (u8_of(r)?, u8_of(r)?);
                    let (x, y) = ((xy >> 4) as usize, (xy & 15) as usize);
                    let (w, h) = ((wh >> 4) as usize + 1, (wh & 15) as usize + 1);
                    if x + w > tw || y + h > th {
                        return Err(bad("a piece outside its tile"));
                    }
                    canvas.fill(tx + x, ty + y, w, h, colour)?;
                }
            }
        }
        Ok(canvas.rgba)
    }

    /// Tiles of 64 by 64, each raw, one colour, a palette or runs, all
    /// packed on one zlib stream for the connection.
    pub fn zrle(&mut self, r: &mut impl Read, width: usize, height: usize) -> io::Result<Vec<u8>> {
        let len = u32_of(r)? as usize;
        let packed = bytes_of(r, len)?;
        let data = inflate(&mut self.zrle, &packed)?;
        let src = &mut &data[..];
        let mut canvas = Canvas::new(width, height)?;
        let mut palette = [[0u8; 3]; 128];
        for ty in (0..height).step_by(64) {
            for tx in (0..width).step_by(64) {
                let (tw, th) = ((width - tx).min(64), (height - ty).min(64));
                let kind = u8_of(src)?;
                match kind {
                    0 => {
                        for y in 0..th {
                            for x in 0..tw {
                                let p = rgb_of(src)?;
                                canvas.set(tx + x, ty + y, p);
                            }
                        }
                    }
                    1 => {
                        let p = rgb_of(src)?;
                        canvas.fill(tx, ty, tw, th, p)?;
                    }
                    2..=16 => {
                        let colours = kind as usize;
                        for p in palette.iter_mut().take(colours) {
                            *p = rgb_of(src)?;
                        }
                        let bits = match colours {
                            2 => 1,
                            3..=4 => 2,
                            _ => 4,
                        };
                        for y in 0..th {
                            // Each row starts on a new byte, highest bits first.
                            let row = bytes_of(src, (tw * bits).div_ceil(8))?;
                            for x in 0..tw {
                                let at = x * bits;
                                let index = (row[at / 8] >> (8 - bits - at % 8)) & ((1 << bits) - 1);
                                if index as usize >= colours {
                                    return Err(bad("a colour outside the tile's palette"));
                                }
                                canvas.set(tx + x, ty + y, palette[index as usize]);
                            }
                        }
                    }
                    128 => {
                        let mut at = 0;
                        while at < tw * th {
                            let p = rgb_of(src)?;
                            let run = run_length(src)?;
                            if at + run > tw * th {
                                return Err(bad("a run past the end of its tile"));
                            }
                            for i in at..at + run {
                                canvas.set(tx + i % tw, ty + i / tw, p);
                            }
                            at += run;
                        }
                    }
                    130..=255 => {
                        let colours = kind as usize - 128;
                        for p in palette.iter_mut().take(colours) {
                            *p = rgb_of(src)?;
                        }
                        let mut at = 0;
                        while at < tw * th {
                            let b = u8_of(src)?;
                            let index = (b & 127) as usize;
                            if index >= colours {
                                return Err(bad("a colour outside the tile's palette"));
                            }
                            let run = if b & 128 != 0 { run_length(src)? } else { 1 };
                            if at + run > tw * th {
                                return Err(bad("a run past the end of its tile"));
                            }
                            for i in at..at + run {
                                canvas.set(tx + i % tw, ty + i / tw, palette[index]);
                            }
                            at += run;
                        }
                    }
                    _ => return Err(bad(format!("a kind of tile ZRLE does not have ({kind})"))),
                }
            }
        }
        Ok(canvas.rgba)
    }

    /// One colour, a JPEG picture, or pixels through a filter (as they
    /// are, by palette, or as differences from their neighbours), packed
    /// on one of four zlib streams.
    pub fn tight(&mut self, r: &mut impl Read, width: usize, height: usize) -> io::Result<Decoded> {
        let control = u8_of(r)?;
        for (i, z) in self.tight.iter_mut().enumerate() {
            if control & (1 << i) != 0 {
                z.reset(true);
            }
        }
        let pixels = area(width, height)?;
        match control >> 4 {
            8 => return Ok(Decoded::Fill(rgb_of(r)?)),
            9 => {
                let len = compact_length(r)?;
                let jpeg = bytes_of(r, len)?;
                let picture = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
                    .map_err(|e| bad(format!("a JPEG picture that does not open: {e}")))?
                    .into_rgba8();
                if picture.width() as usize != width || picture.height() as usize != height {
                    return Err(bad("a JPEG picture of another size than its area"));
                }
                return Ok(Decoded::Pixels(picture.into_raw()));
            }
            kind if kind & 8 != 0 => return Err(bad(format!("a kind of area Tight does not have ({kind})"))),
            _ => {}
        }
        let stream = ((control >> 4) & 3) as usize;
        let filter = if control & 0x40 != 0 { u8_of(r)? } else { 0 };
        let mut canvas = Canvas::new(width, height)?;
        match filter {
            // As they are.
            0 => {
                let data = self.tight_data(r, stream, pixels * 3)?;
                for (to, from) in canvas.rgba.as_chunks_mut::<4>().0.iter_mut().zip(data.as_chunks::<3>().0) {
                    to[..3].copy_from_slice(from);
                }
            }
            // By palette: one bit each for two colours, else one byte.
            1 => {
                let colours = u8_of(r)? as usize + 1;
                let table = bytes_of(r, colours * 3)?;
                let colour = |i: usize| -> io::Result<[u8; 3]> {
                    table
                        .get(i * 3..i * 3 + 3)
                        .map(|c| [c[0], c[1], c[2]])
                        .ok_or_else(|| bad("a colour outside the area's palette"))
                };
                if colours == 2 {
                    let row_len = width.div_ceil(8);
                    let data = self.tight_data(r, stream, row_len * height)?;
                    for y in 0..height {
                        for x in 0..width {
                            let bit = (data[y * row_len + x / 8] >> (7 - x % 8)) & 1;
                            canvas.set(x, y, colour(bit as usize)?);
                        }
                    }
                } else {
                    let data = self.tight_data(r, stream, pixels)?;
                    for (i, index) in data.iter().take(pixels).enumerate() {
                        canvas.set(i % width, i / width, colour(*index as usize)?);
                    }
                }
            }
            // As differences from what the pixels to the left and above
            // would predict.
            2 => {
                let data = self.tight_data(r, stream, pixels * 3)?;
                let mut above = vec![0u8; width * 3];
                let mut row = vec![0u8; width * 3];
                for y in 0..height {
                    for x in 0..width {
                        for c in 0..3 {
                            let left = if x > 0 { row[(x - 1) * 3 + c] as i32 } else { 0 };
                            let up = above[x * 3 + c] as i32;
                            let up_left = if x > 0 { above[(x - 1) * 3 + c] as i32 } else { 0 };
                            let predicted = (left + up - up_left).clamp(0, 255);
                            row[x * 3 + c] = (data[(y * width + x) * 3 + c] as i32 + predicted) as u8;
                        }
                        canvas.set(x, y, [row[x * 3], row[x * 3 + 1], row[x * 3 + 2]]);
                    }
                    std::mem::swap(&mut above, &mut row);
                }
            }
            _ => return Err(bad(format!("a filter Tight does not have ({filter})"))),
        }
        Ok(Decoded::Pixels(canvas.rgba))
    }

    /// `expected` bytes of Tight data: as they are when few, else packed.
    fn tight_data(&mut self, r: &mut impl Read, stream: usize, expected: usize) -> io::Result<Vec<u8>> {
        if expected < 12 {
            return bytes_of(r, expected);
        }
        let len = compact_length(r)?;
        let packed = bytes_of(r, len)?;
        let data = inflate(&mut self.tight[stream], &packed)?;
        if data.len() < expected {
            return Err(bad("packed data shorter than its area"));
        }
        Ok(data)
    }
}

/// The pointer's shape: its pixels, then one bit each for where it is.
pub fn cursor(r: &mut impl Read, width: usize, height: usize, hot_x: u16, hot_y: u16) -> io::Result<Pointer> {
    if width > 512 || height > 512 {
        return Err(bad("a pointer larger than any pointer"));
    }
    let mut rgba = bytes_of(r, width * height * 4)?;
    let row_len = width.div_ceil(8);
    let mask = bytes_of(r, row_len * height)?;
    for y in 0..height {
        for x in 0..width {
            let shown = (mask[y * row_len + x / 8] >> (7 - x % 8)) & 1 != 0;
            rgba[(y * width + x) * 4 + 3] = if shown { 255 } else { 0 };
        }
    }
    Ok(Pointer {
        width: width as u16,
        height: height as u16,
        hot_x,
        hot_y,
        rgba,
    })
}
