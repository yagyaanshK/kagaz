//! A small VNC server that shows the generated test card
//! ([`crate::remote::pattern`]), for trying and testing the viewer without
//! anyone's real screen: `kagaz remote pattern` runs it, and the tests
//! connect to it.
//!
//! It serves Kagaz's own pixel layout only, packs areas whichever way the
//! viewer asks for first, shows where the pointer is pressed (red for the
//! left button, green the middle, blue the right) and the last key in
//! squares, and keeps a record of what it heard.

use super::clip;
use super::decode::{self, bad, bytes_of, u16_of, u32_of, u8_of};
use super::encode::{self, Encoder};
use super::{password_response, PIXEL_FORMAT};
use crate::remote::pattern;
use crate::remote::screen::{Pointer, Rect};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Options {
    pub width: u16,
    pub height: u16,
    /// Ask for this VNC password (its first eight letters count).
    pub password: Option<String>,
    pub name: String,
    /// Pack every area this way, whatever the viewer prefers.
    pub force: Option<i32>,
    /// How many times a second the card moves; 0 keeps it still.
    pub ticks_per_second: u32,
    /// The protocol version it greets with: "003.003", "003.007", "003.008".
    pub version: &'static str,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            width: 1280,
            height: 720,
            password: None,
            name: "Kagaz test pattern".into(),
            force: None,
            ticks_per_second: 30,
            version: "003.008",
        }
    }
}

/// Something a viewer did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    Pointer { x: u16, y: u16, buttons: u8 },
    Key { keysym: u32, down: bool },
    /// Text the viewer's side copied.
    Text(String),
    Encodings(Vec<i32>),
}

struct Shared {
    options: Options,
    stop: AtomicBool,
    heard: Mutex<Vec<Heard>>,
    size: Mutex<(u16, u16)>,
    /// Text "copied on the device", and how many times that happened.
    copied: Mutex<(u64, String)>,
    /// Ticks added by hand, on top of those the clock gives.
    ticks: AtomicU32,
}

pub struct Server {
    pub addr: SocketAddr,
    shared: Arc<Shared>,
}

impl Server {
    /// Serve the card on `listener` until stopped or dropped.
    pub fn start(listener: TcpListener, options: Options) -> io::Result<Server> {
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let shared = Arc::new(Shared {
            size: Mutex::new((options.width, options.height)),
            options,
            stop: AtomicBool::new(false),
            heard: Mutex::new(Vec::new()),
            copied: Mutex::new((0, String::new())),
            ticks: AtomicU32::new(0),
        });
        let all = shared.clone();
        std::thread::Builder::new().name("vnc-pattern".into()).spawn(move || {
            while !all.stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let shared = all.clone();
                        let _ = std::thread::Builder::new().name("vnc-pattern-viewer".into()).spawn(move || {
                            let _ = stream.set_nonblocking(false);
                            let _ = viewer(stream, &shared);
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        })?;
        Ok(Server { addr, shared })
    }

    /// What viewers did so far, oldest first.
    pub fn heard(&self) -> Vec<Heard> {
        self.shared.heard.lock().unwrap().clone()
    }

    /// The screen changes size, as when a phone is turned.
    pub fn resize(&self, width: u16, height: u16) {
        *self.shared.size.lock().unwrap() = (width, height);
    }

    /// Text is copied "on the device".
    pub fn copy(&self, text: &str) {
        let mut copied = self.shared.copied.lock().unwrap();
        copied.0 += 1;
        copied.1 = text.to_string();
    }

    /// Move the card on by one tick, now.
    pub fn tick(&self) {
        self.shared.ticks.fetch_add(1, Ordering::AcqRel);
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn random_challenge() -> [u8; 16] {
    *uuid::Uuid::new_v4().as_bytes()
}

/// The pointer's shape this server gives: a small arrow.
fn arrow() -> Pointer {
    let (width, height) = (12u16, 18u16);
    let mut rgba = Vec::new();
    for y in 0..height {
        for x in 0..width {
            // A triangle with its point at the top left, white inside a
            // dark edge.
            let inside = x * 3 <= y * 2 && y < 16;
            let edge = inside && (x == 0 || (x + 1) * 3 > y * 2 || y == 15);
            rgba.extend_from_slice(match (inside, edge) {
                (true, true) => &[20, 20, 20, 255],
                (true, false) => &[255, 255, 255, 255],
                _ => &[0, 0, 0, 0],
            });
        }
    }
    Pointer {
        width,
        height,
        hot_x: 0,
        hot_y: 0,
        rgba,
    }
}

/// The card with what the viewer does drawn on it.
fn frame(width: u16, height: u16, tick: u32, pointer: (u16, u16, u8), key: Option<(u32, bool)>) -> Vec<u8> {
    let mut rgba = pattern::card(width, height, tick);
    let (w, h) = (width as i32, height as i32);
    let mut square = |cx: i32, cy: i32, half: i32, colour: [u8; 3]| {
        for y in (cy - half).max(0)..(cy + half + 1).min(h) {
            for x in (cx - half).max(0)..(cx + half + 1).min(w) {
                let at = (y as usize * width as usize + x as usize) * 4;
                rgba[at..at + 3].copy_from_slice(&colour);
            }
        }
    };
    let (x, y, buttons) = pointer;
    if buttons & 7 != 0 {
        let colour = match buttons & 7 {
            1 => [255, 0, 0],
            2 => [0, 255, 0],
            4 => [0, 0, 255],
            _ => [255, 255, 255],
        };
        square(x as i32, y as i32, 6, colour);
    }
    if let Some((keysym, down)) = key {
        // The key's number in sixteen squares along the bottom right.
        for bit in 0..16 {
            let on = keysym >> (15 - bit) & 1 == 1;
            let colour = match (on, down) {
                (true, true) => [80, 255, 120],
                (true, false) => [240, 240, 240],
                (false, _) => [70, 70, 70],
            };
            square(w - 8 * (16 - bit) + 3, h - 5, 3, colour);
        }
    }
    rgba
}

fn pixels_of(rgba: &[u8], width: usize, r: Rect) -> Vec<u8> {
    let mut out = Vec::with_capacity(r.area() as usize * 4);
    for y in r.y as usize..(r.y + r.height) as usize {
        let at = (y * width + r.x as usize) * 4;
        out.extend_from_slice(&rgba[at..at + r.width as usize * 4]);
    }
    out
}

fn paste(rgba: &mut [u8], width: usize, r: Rect, pixels: &[u8]) {
    let len = r.width as usize * 4;
    for row in 0..r.height as usize {
        let at = ((r.y as usize + row) * width + r.x as usize) * 4;
        rgba[at..at + len].copy_from_slice(&pixels[row * len..(row + 1) * len]);
    }
}

/// The areas where `now` differs from `had`, or that `must` be sent: in
/// strips of tiles 64 pixels high, none wider than Tight allows.
fn changed(had: &[u8], now: &[u8], width: u16, height: u16, must: Option<Rect>) -> Vec<Rect> {
    const TILE: u16 = 64;
    const WIDEST: u16 = 2048;
    let mut out: Vec<Rect> = Vec::new();
    let mut y = 0;
    while y < height {
        let th = TILE.min(height - y);
        let mut open: Option<Rect> = None;
        let mut x = 0;
        while x < width {
            let tw = TILE.min(width - x);
            let tile = Rect::new(x, y, tw, th);
            let differs = must.is_some_and(|m| m.intersects(&tile))
                || (0..th as usize).any(|row| {
                    let at = ((y as usize + row) * width as usize + x as usize) * 4;
                    had[at..at + tw as usize * 4] != now[at..at + tw as usize * 4]
                });
            match (&mut open, differs) {
                (Some(strip), true) if strip.width + tw <= WIDEST => strip.width += tw,
                (strip, true) => {
                    out.extend(strip.take());
                    *strip = Some(tile);
                }
                (strip, false) => out.extend(strip.take()),
            }
            x += tw;
        }
        out.extend(open);
        y += th;
    }
    out
}

fn write_rect(out: &mut Vec<u8>, r: Rect, encoding: i32) {
    for v in [r.x, r.y, r.width, r.height] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&encoding.to_be_bytes());
}

fn cut_text(payload_len: i32) -> Vec<u8> {
    let mut out = vec![3, 0, 0, 0];
    out.extend_from_slice(&payload_len.to_be_bytes());
    out
}

/// What a picture is a picture of: the tick, the pointer and the last key.
type Drawn = (u32, (u16, u16, u8), Option<(u32, bool)>);

/// Tight's JPEG levels as JPEG qualities.
const JPEG_QUALITY: [u8; 10] = [5, 10, 15, 25, 37, 50, 60, 70, 75, 80];

/// One viewer, from greeting to goodbye.
fn viewer(mut s: TcpStream, shared: &Shared) -> io::Result<()> {
    let options = &shared.options;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    s.write_all(format!("RFB {}\n", options.version).as_bytes())?;
    let theirs = bytes_of(&mut s, 12)?;
    let minor: u32 = String::from_utf8_lossy(&theirs[8..11]).parse().unwrap_or(3);
    let kind: u8 = if options.password.is_some() { 2 } else { 1 };
    if minor < 7 {
        s.write_all(&(kind as u32).to_be_bytes())?;
    } else {
        s.write_all(&[1, kind])?;
        if u8_of(&mut s)? != kind {
            return Err(bad("the viewer chose a sign-in that was not offered"));
        }
    }
    if let Some(password) = &options.password {
        let challenge = random_challenge();
        s.write_all(&challenge)?;
        let answer = bytes_of(&mut s, 16)?;
        if answer != password_response(password, &challenge) {
            s.write_all(&1u32.to_be_bytes())?;
            if minor >= 8 {
                let why = b"the password is wrong";
                s.write_all(&(why.len() as u32).to_be_bytes())?;
                s.write_all(why)?;
            }
            return Ok(());
        }
        s.write_all(&0u32.to_be_bytes())?;
    } else if minor >= 8 {
        s.write_all(&0u32.to_be_bytes())?;
    }
    u8_of(&mut s)?;
    let (mut width, mut height) = *shared.size.lock().unwrap();
    let mut hello = Vec::new();
    hello.extend_from_slice(&width.to_be_bytes());
    hello.extend_from_slice(&height.to_be_bytes());
    // Its own layout, as servers have: blue lowest. The viewer says what
    // it wants instead.
    hello.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    hello.extend_from_slice(&(options.name.len() as u32).to_be_bytes());
    hello.extend_from_slice(options.name.as_bytes());
    s.write_all(&hello)?;

    let started = Instant::now();
    let mut encoder = Encoder::default();
    let mut encodings: Vec<i32> = Vec::new();
    // What the viewer has on its screen, and of which tick.
    let mut had: Vec<u8> = vec![0; width as usize * height as usize * 4];
    let mut had_tick: Option<u32> = None;
    let mut asked = false;
    let mut must: Option<Rect> = None;
    let mut pointer = (0u16, 0u16, 0u8);
    let mut key: Option<(u32, bool)> = None;
    let mut arrow_sent = false;
    let mut caps_sent = false;
    let mut viewer_caps: Option<u32> = None;
    // Text copied before this viewer came is told to it too.
    let mut copied_seen = 0u64;
    let mut updates = 0u32;
    // What the last picture sent was a picture of.
    let mut drawn: Option<Drawn> = None;

    while !shared.stop.load(Ordering::Acquire) {
        // ---- what the viewer says, waiting only a moment for it ----
        s.set_read_timeout(Some(Duration::from_millis(5)))?;
        let mut first = [0u8; 1];
        let kind = match s.read(&mut first) {
            Ok(0) => return Ok(()),
            Ok(_) => Some(first[0]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => None,
            Err(e) => return Err(e),
        };
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        match kind {
            None => {}
            Some(0) => {
                bytes_of(&mut s, 3)?;
                if bytes_of(&mut s, 16)? != PIXEL_FORMAT {
                    return Err(bad("this test server only serves Kagaz's own pixel layout"));
                }
            }
            Some(2) => {
                u8_of(&mut s)?;
                let count = u16_of(&mut s)?;
                encodings = (0..count).map(|_| u32_of(&mut s).map(|e| e as i32)).collect::<io::Result<_>>()?;
                shared.heard.lock().unwrap().push(Heard::Encodings(encodings.clone()));
            }
            Some(3) => {
                let incremental = u8_of(&mut s)? != 0;
                let r = Rect::new(u16_of(&mut s)?, u16_of(&mut s)?, u16_of(&mut s)?, u16_of(&mut s)?);
                asked = true;
                if !incremental {
                    must = Some(must.map_or(r, |m| m.union(&r)));
                }
            }
            Some(4) => {
                let down = u8_of(&mut s)? != 0;
                u16_of(&mut s)?;
                let keysym = u32_of(&mut s)?;
                key = Some((keysym, down));
                shared.heard.lock().unwrap().push(Heard::Key { keysym, down });
            }
            Some(5) => {
                let buttons = u8_of(&mut s)?;
                let (x, y) = (u16_of(&mut s)?, u16_of(&mut s)?);
                pointer = (x, y, buttons);
                shared.heard.lock().unwrap().push(Heard::Pointer { x, y, buttons });
            }
            Some(6) => {
                bytes_of(&mut s, 3)?;
                let len = u32_of(&mut s)? as i32;
                if len >= 0 {
                    let text = clip::from_latin1(&bytes_of(&mut s, len as usize)?);
                    shared.heard.lock().unwrap().push(Heard::Text(text));
                } else {
                    match clip::parse(&bytes_of(&mut s, len.unsigned_abs() as usize)?)? {
                        clip::Message::Caps { flags, .. } => viewer_caps = Some(flags),
                        clip::Message::Provide(Some(text)) => shared.heard.lock().unwrap().push(Heard::Text(text)),
                        clip::Message::Notify(formats) if formats & clip::TEXT != 0 => {
                            let payload = clip::request();
                            s.write_all(&cut_text(-(payload.len() as i32)))?;
                            s.write_all(&payload)?;
                        }
                        clip::Message::Request(formats) if formats & clip::TEXT != 0 => {
                            let payload = clip::provide(&shared.copied.lock().unwrap().1);
                            s.write_all(&cut_text(-(payload.len() as i32)))?;
                            s.write_all(&payload)?;
                        }
                        clip::Message::Peek => {
                            let payload = clip::notify(!shared.copied.lock().unwrap().1.is_empty());
                            s.write_all(&cut_text(-(payload.len() as i32)))?;
                            s.write_all(&payload)?;
                        }
                        _ => {}
                    }
                }
            }
            Some(other) => return Err(bad(format!("a message this test server does not know ({other})"))),
        }
        // More may be waiting: hear it all before drawing.
        if kind.is_some() {
            continue;
        }

        // ---- the clipboard ----
        let extended = encodings.contains(&decode::EXTENDED_CLIPBOARD);
        if extended && !caps_sent {
            caps_sent = true;
            let payload = clip::caps();
            s.write_all(&cut_text(-(payload.len() as i32)))?;
            s.write_all(&payload)?;
        }
        let (count, text) = shared.copied.lock().unwrap().clone();
        if count != copied_seen && (!extended || viewer_caps.is_some()) {
            copied_seen = count;
            if extended {
                let payload = clip::notify(true);
                s.write_all(&cut_text(-(payload.len() as i32)))?;
                s.write_all(&payload)?;
            } else {
                let bytes = clip::to_latin1(&text);
                s.write_all(&cut_text(bytes.len() as i32))?;
                s.write_all(&bytes)?;
            }
        }

        // ---- the picture, when the viewer has asked for one ----
        if !asked {
            continue;
        }
        let mut update = Vec::new();
        let mut count = 0u16;
        let size = *shared.size.lock().unwrap();
        if size != (width, height) {
            if !encodings.contains(&decode::DESKTOP_SIZE) {
                return Err(bad("the screen changed size and the viewer cannot follow"));
            }
            (width, height) = size;
            had = vec![0; width as usize * height as usize * 4];
            had_tick = None;
            must = None;
            drawn = None;
            write_rect(&mut update, Rect::new(0, 0, width, height), decode::DESKTOP_SIZE);
            count += 1;
        } else {
            if !arrow_sent && encodings.contains(&decode::CURSOR) {
                arrow_sent = true;
                let arrow = arrow();
                write_rect(
                    &mut update,
                    Rect::new(arrow.hot_x, arrow.hot_y, arrow.width, arrow.height),
                    decode::CURSOR,
                );
                update.extend(encode::cursor(&arrow));
                count += 1;
            }
            let tick = shared.ticks.load(Ordering::Acquire)
                + match options.ticks_per_second {
                    0 => 0,
                    n => (started.elapsed().as_millis() as u64 * n as u64 / 1000) as u32,
                };
            // Nothing moved since the last picture: nothing to draw.
            if must.is_none() && drawn == Some((tick, pointer, key)) {
                if count > 0 {
                    let mut message = vec![0, 0];
                    message.extend_from_slice(&count.to_be_bytes());
                    message.extend(update);
                    s.write_all(&message)?;
                    asked = false;
                }
                continue;
            }
            drawn = Some((tick, pointer, key));
            let now = frame(width, height, tick, pointer, key);
            // The scrolling band, when the viewer has the tick before:
            // told as a move, the way a real screen's scrolling is.
            let (top, bottom) = pattern::scroll_rows(height);
            let step = pattern::SCROLL_STEP as u16;
            if encodings.contains(&decode::COPY_RECT)
                && options.force.is_none_or(|f| f == decode::COPY_RECT)
                && had_tick.is_some_and(|t| t + 1 == tick)
                && width > step
                && bottom > top
            {
                let to = Rect::new(0, top, width - step, bottom - top);
                let moved = pixels_of(&had, width as usize, Rect::new(step, top, to.width, to.height));
                paste(&mut had, width as usize, to, &moved);
                write_rect(&mut update, to, decode::COPY_RECT);
                update.extend_from_slice(&step.to_be_bytes());
                update.extend_from_slice(&top.to_be_bytes());
                count += 1;
            }
            let how = options
                .force
                .filter(|f| *f != decode::COPY_RECT)
                .or_else(|| {
                    encodings
                        .iter()
                        .copied()
                        .find(|e| [decode::TIGHT, decode::ZRLE, decode::HEXTILE, decode::RRE, decode::RAW].contains(e))
                })
                .unwrap_or(decode::RAW);
            let jpeg = encodings
                .iter()
                .find(|e| (decode::JPEG_QUALITY_0..decode::JPEG_QUALITY_0 + 10).contains(*e))
                .map(|e| JPEG_QUALITY[(e - decode::JPEG_QUALITY_0) as usize]);
            // Every other update the other of Tight's two plain ways.
            encoder.gradient = updates % 2 == 1;
            for r in changed(&had, &now, width, height, must.take()) {
                let rgba = pixels_of(&now, width as usize, r);
                let (w, h) = (r.width as usize, r.height as usize);
                write_rect(&mut update, r, how);
                update.extend(match how {
                    decode::TIGHT => encoder.tight(&rgba, w, h, jpeg),
                    decode::ZRLE => encoder.zrle(&rgba, w, h),
                    decode::HEXTILE => encode::hextile(&rgba, w, h),
                    decode::RRE => encode::rre(&rgba, w),
                    _ => encode::raw(&rgba),
                });
                paste(&mut had, width as usize, r, &rgba);
                count += 1;
            }
            had_tick = Some(tick);
        }
        if count == 0 {
            continue;
        }
        let mut message = vec![0, 0];
        message.extend_from_slice(&count.to_be_bytes());
        message.extend(update);
        s.write_all(&message)?;
        asked = false;
        updates += 1;
    }
    Ok(())
}
