//! VNC (the RFB protocol), spoken by Kagaz itself: the screen of a phone
//! running a VNC server, a Mac's Screen Sharing with a VNC password, a
//! Linux computer's VNC.
//!
//! The picture arrives as changed areas, each packed one of several ways
//! (`decode.rs`), into the shared [`Screen`]. Kagaz asks for the next
//! update as soon as the last one is in, so the device never sends
//! pictures faster than they arrive. VNC itself protects nothing: the
//! screen, the keys and (with a VNC password) eight letters of password
//! travel as they are, so the path has to do the protecting (Tailscale
//! does; a home network does not).

pub mod clip;
pub mod decode;
pub mod encode;
pub mod server;

use super::keys;
use super::screen::{Event, Rect, Screen, Sending};
use super::session::{
    buttons_for, Info, Input, InputSender, Numbers, OpenError, Session, SignIn, LEFT, MIDDLE, RIGHT,
};
use super::{Settings, Speaks};
use decode::{bad, bytes_of, u16_of, u32_of, u8_of};
use des::cipher::generic_array::GenericArray;
use des::cipher::{BlockEncrypt, KeyInit};
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// The pixel layout Kagaz asks of every server: 32 bits a pixel, 24 of
/// them colour, low byte first, red in the lowest byte, then green, blue.
pub(crate) const PIXEL_FORMAT: [u8; 16] = [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0];

/// How often the delay is measured.
const PING_EVERY: Duration = Duration::from_secs(2);
/// No answer to a measurement for this long: the link is gone.
const DEAD_AFTER: Duration = Duration::from_secs(20);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// What a VNC password turns the server's challenge into: DES with the
/// password's first eight bytes as the key, each byte's bits reversed.
pub(crate) fn password_response(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    des_response(password.as_bytes(), challenge)
}

fn des_response(password: &[u8], challenge: &[u8; 16]) -> [u8; 16] {
    let mut key = [0u8; 8];
    for (k, b) in key.iter_mut().zip(password) {
        *k = b.reverse_bits();
    }
    let des = des::Des::new(&GenericArray::from(key));
    let mut out = *challenge;
    for block in out.as_chunks_mut::<8>().0 {
        des.encrypt_block(GenericArray::from_mut_slice(block));
    }
    out
}

/// A kind of sign-in a server offers that Kagaz does not have, in words.
fn security_name(kind: u8) -> String {
    match kind {
        5 | 6 | 13 | 129..=134 => "RealVNC's own sign-in".into(),
        16 => "TightVNC's own sign-in".into(),
        18 | 19 => "VNC over TLS (VeNCrypt)".into(),
        20 => "SASL".into(),
        30 | 33 | 35 | 36 => "a Mac account (turn on \"VNC viewers may control screen with password\" there)".into(),
        113..=115 => "a Windows account (UltraVNC)".into(),
        n => format!("kind {n}"),
    }
}

/// A reason the server gives, as text.
fn reason(r: &mut impl Read) -> io::Result<String> {
    let len = u32_of(r)? as usize;
    if len > 4096 {
        return Err(bad("a reason longer than any reason"));
    }
    Ok(String::from_utf8_lossy(&bytes_of(r, len)?).trim().to_string())
}

struct Hello {
    width: u16,
    height: u16,
    name: String,
}

/// From the server's greeting to its screen's size and name.
fn handshake(r: &mut impl Read, w: &mut impl Write, sign_in: &SignIn) -> Result<Hello, OpenError> {
    let greeting = bytes_of(r, 12)?;
    if !greeting.starts_with(b"RFB ") {
        return Err(OpenError::Other("what answers there is not a VNC server".into()));
    }
    let number = |from: usize| -> u32 { String::from_utf8_lossy(&greeting[from..from + 3]).parse().unwrap_or(0) };
    let (major, minor) = (number(4), number(8));
    // 3.3, 3.7 and 3.8 are all there is; anything later speaks 3.8 too.
    let minor = if major > 3 || minor >= 8 {
        8
    } else if minor == 7 {
        7
    } else {
        3
    };
    w.write_all(format!("RFB 003.00{minor}\n").as_bytes())?;

    let kind = if minor == 3 {
        match u32_of(r)? {
            0 => return Err(OpenError::Refused(reason(r)?)),
            n => n as u8,
        }
    } else {
        let count = u8_of(r)? as usize;
        if count == 0 {
            return Err(OpenError::Refused(reason(r)?));
        }
        let offered = bytes_of(r, count)?;
        // No sign-in if it offers that, else a VNC password.
        let Some(kind) = [1u8, 2].into_iter().find(|k| offered.contains(k)) else {
            let mut names: Vec<String> = offered.iter().map(|k| security_name(*k)).collect();
            names.dedup();
            return Err(OpenError::Unsupported(format!(
                "its VNC server only takes a sign-in Kagaz does not have yet: {}. A VNC password, or none, works",
                names.join(", ")
            )));
        };
        if kind == 2 && sign_in.password.is_none() {
            return Err(OpenError::NeedsPassword);
        }
        w.write_all(&[kind])?;
        kind
    };
    match kind {
        1 => {}
        2 => {
            let Some(password) = &sign_in.password else {
                return Err(OpenError::NeedsPassword);
            };
            let mut challenge = [0u8; 16];
            r.read_exact(&mut challenge)?;
            w.write_all(&password_response(password, &challenge))?;
        }
        n => {
            return Err(OpenError::Unsupported(format!(
                "its VNC server only takes a sign-in Kagaz does not have yet: {}",
                security_name(n)
            )))
        }
    }
    // The result: always after a password; without one, from 3.8 on.
    if kind == 2 || minor == 8 {
        match u32_of(r)? {
            0 => {}
            result => {
                let why = if minor == 8 { reason(r).unwrap_or_default() } else { String::new() };
                return Err(match (kind, result) {
                    (2, 1) => OpenError::WrongPassword,
                    (_, 2) => OpenError::Refused("too many tries; wait a little and try again".into()),
                    _ if why.is_empty() => OpenError::Refused("it gave no reason".into()),
                    _ => OpenError::Refused(why),
                });
            }
        }
    }
    // Share the screen with whoever else is looking at it.
    w.write_all(&[1])?;
    let (width, height) = (u16_of(r)?, u16_of(r)?);
    bytes_of(r, 16)?;
    let len = u32_of(r)? as usize;
    if len > 65536 {
        return Err(OpenError::Other("a name longer than any name".into()));
    }
    let name = String::from_utf8_lossy(&bytes_of(r, len)?).trim().to_string();
    Ok(Hello { width, height, name })
}

/// The ways of packing Kagaz reads, best first, and with `jpeg` how much
/// a Tight server may lose in its JPEG areas (9 the least; none: nothing).
fn set_encodings(jpeg: Option<u8>) -> Vec<u8> {
    use decode::*;
    let mut list = vec![
        TIGHT,
        ZRLE,
        HEXTILE,
        RRE,
        COPY_RECT,
        RAW,
        CURSOR,
        DESKTOP_SIZE,
        LAST_RECT,
        EXTENDED_CLIPBOARD,
    ];
    if let Some(level) = jpeg {
        list.push(JPEG_QUALITY_0 + level.min(9) as i32);
    }
    let mut out = vec![2, 0];
    out.extend_from_slice(&(list.len() as u16).to_be_bytes());
    for e in list {
        out.extend_from_slice(&e.to_be_bytes());
    }
    out
}

fn update_request(incremental: bool, rect: Rect) -> [u8; 10] {
    let mut out = [3, incremental as u8, 0, 0, 0, 0, 0, 0, 0, 0];
    out[2..4].copy_from_slice(&rect.x.to_be_bytes());
    out[4..6].copy_from_slice(&rect.y.to_be_bytes());
    out[6..8].copy_from_slice(&rect.width.to_be_bytes());
    out[8..10].copy_from_slice(&rect.height.to_be_bytes());
    out
}

fn pointer_event(out: &mut Vec<u8>, buttons: u8, x: u16, y: u16) {
    out.extend_from_slice(&[5, buttons]);
    out.extend_from_slice(&x.to_be_bytes());
    out.extend_from_slice(&y.to_be_bytes());
}

fn key_event(out: &mut Vec<u8>, keysym: u32, down: bool) {
    out.extend_from_slice(&[4, down as u8, 0, 0]);
    out.extend_from_slice(&keysym.to_be_bytes());
}

/// Clipboard text the old way.
fn cut_text(out: &mut Vec<u8>, text: &str) {
    let bytes = clip::to_latin1(text);
    out.extend_from_slice(&[6, 0, 0, 0]);
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend(bytes);
}

/// An extended clipboard message: the old one with its length negative.
fn cut_extended(out: &mut Vec<u8>, payload: &[u8]) {
    out.extend_from_slice(&[6, 0, 0, 0]);
    out.extend_from_slice(&(-(payload.len() as i32)).to_be_bytes());
    out.extend_from_slice(payload);
}

/// The picture setting (0 sharpest to 10 smoothest) as Tight's JPEG level:
/// none at the sharp end (nothing is lost), then 9 down to 1.
fn jpeg_level(picture: u8) -> Option<u8> {
    (picture > 0).then(|| 10u8.saturating_sub(picture).max(1))
}

/// The picture setting that suits a link with this delay.
fn picture_for(delay: Option<u32>) -> u8 {
    match delay {
        None | Some(0..30) => 0,
        Some(30..80) => 2,
        Some(80..160) => 4,
        Some(_) => 6,
    }
}

#[derive(Default)]
struct Clipboard {
    /// What the server can do with the extended clipboard: its flags and
    /// how much text it takes unasked.
    server: Option<(u32, u32)>,
    /// The text last seen, from either side: it is not sent back.
    last: Option<String>,
}

/// What the two threads of a session share.
struct Link {
    out: Mutex<TcpStream>,
    screen: Arc<Screen>,
    numbers: Arc<Numbers>,
    /// When the measurement in flight left.
    ping: Mutex<Option<Instant>>,
    delays: Mutex<VecDeque<u32>>,
    clipboard: Mutex<Clipboard>,
    /// The session has ended, and its reason has been told.
    closing: AtomicBool,
}

impl Link {
    fn send(&self, bytes: &[u8]) -> io::Result<()> {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        out.write_all(bytes)?;
        self.numbers.add_sent(bytes.len());
        Ok(())
    }

    /// An update arrived: if a measurement was out, it is the answer.
    fn pong(&self) {
        let Some(left) = self.ping.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        let mut delays = self.delays.lock().unwrap_or_else(|e| e.into_inner());
        delays.push_back(left.elapsed().as_millis().min(u32::MAX as u128 - 1) as u32);
        if delays.len() > 5 {
            delays.pop_front();
        }
        // The best of the last few: an update in the way makes one slow.
        self.numbers.set_delay(*delays.iter().min().expect("just pushed"));
    }

    /// The session is over; only the first reason given is told.
    fn end(&self, why: &str) {
        if self.closing.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.out.lock().unwrap_or_else(|e| e.into_inner()).shutdown(Shutdown::Both);
        self.screen.post(Event::Ended(why.into()));
    }

    /// Text arrived from the device.
    fn text_in(&self, text: String) {
        let mut c = self.clipboard.lock().unwrap_or_else(|e| e.into_inner());
        if c.last.as_deref() == Some(&text) {
            return;
        }
        c.last = Some(text.clone());
        drop(c);
        self.screen.post(Event::Clipboard(text));
    }

    /// An extended clipboard message from the device.
    fn clipboard_in(&self, payload: &[u8]) -> io::Result<()> {
        let mut out = Vec::new();
        match clip::parse(payload)? {
            clip::Message::Caps { flags, text_max } => {
                self.clipboard.lock().unwrap_or_else(|e| e.into_inner()).server = Some((flags, text_max));
                cut_extended(&mut out, &clip::caps());
            }
            clip::Message::Notify(formats) => {
                if formats & clip::TEXT != 0 {
                    cut_extended(&mut out, &clip::request());
                }
            }
            clip::Message::Request(formats) => {
                let last = self.clipboard.lock().unwrap_or_else(|e| e.into_inner()).last.clone();
                if let (true, Some(text)) = (formats & clip::TEXT != 0, last) {
                    cut_extended(&mut out, &clip::provide(&text));
                }
            }
            clip::Message::Peek => {
                let has = self.clipboard.lock().unwrap_or_else(|e| e.into_inner()).last.is_some();
                cut_extended(&mut out, &clip::notify(has));
            }
            clip::Message::Provide(Some(text)) => self.text_in(text),
            clip::Message::Provide(None) => {}
        }
        if !out.is_empty() {
            self.send(&out)?;
        }
        Ok(())
    }

    /// Text was copied here: offer it to the device, the best way the
    /// device has.
    fn clipboard_out(&self, text: &str, out: &mut Vec<u8>) {
        let mut c = self.clipboard.lock().unwrap_or_else(|e| e.into_inner());
        if c.last.as_deref() == Some(text) {
            return;
        }
        c.last = Some(text.to_string());
        match c.server {
            // Small enough to send unasked.
            Some((flags, most)) if flags & clip::PROVIDE != 0 && clip::wire_len(text) <= most as usize => {
                cut_extended(out, &clip::provide(text));
            }
            // Announce it; the device asks when it wants it.
            Some((flags, _)) if flags & clip::NOTIFY != 0 => cut_extended(out, &clip::notify(true)),
            _ => cut_text(out, text),
        }
    }
}

/// A reader that counts what it reads.
struct Counted<R> {
    inner: R,
    numbers: Arc<Numbers>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.numbers.add_received(n);
        Ok(n)
    }
}

/// Everything the device sends, until the connection ends.
fn read_loop(r: &mut impl Read, link: &Link, mut size: (u16, u16)) -> io::Result<()> {
    use decode::*;
    let mut decoder = Decoder::default();
    let screen = &link.screen;
    loop {
        match u8_of(r)? {
            0 => {
                link.pong();
                u8_of(r)?;
                let count = u16_of(r)?;
                // After a new size, ask for all of it.
                let mut whole = false;
                for _ in 0..count {
                    let (x, y, w, h) = (u16_of(r)?, u16_of(r)?, u16_of(r)?, u16_of(r)?);
                    let encoding = u32_of(r)? as i32;
                    let rect = Rect::new(x, y, w, h);
                    let (width, height) = (w as usize, h as usize);
                    match encoding {
                        RAW => screen.put(rect, &decoder.raw(r, width, height)?),
                        COPY_RECT => {
                            let (from_x, from_y) = (u16_of(r)?, u16_of(r)?);
                            screen.copy(from_x, from_y, rect);
                        }
                        RRE => screen.put(rect, &decoder.rre(r, width, height)?),
                        HEXTILE => screen.put(rect, &decoder.hextile(r, width, height)?),
                        ZRLE => screen.put(rect, &decoder.zrle(r, width, height)?),
                        TIGHT => match decoder.tight(r, width, height)? {
                            Decoded::Pixels(rgba) => screen.put(rect, &rgba),
                            Decoded::Fill(rgb) => screen.fill(rect, rgb),
                        },
                        CURSOR => screen.post(Event::Pointer(cursor(r, width, height, x, y)?)),
                        DESKTOP_SIZE => {
                            screen.resize(w, h);
                            size = (w, h);
                            whole = true;
                        }
                        LAST_RECT => break,
                        other => {
                            return Err(bad(format!(
                                "the device packed its picture a way Kagaz did not ask for ({other})"
                            )))
                        }
                    }
                }
                screen.commit();
                link.numbers.add_update();
                link.send(&update_request(!whole, Rect::new(0, 0, size.0, size.1)))?;
            }
            // A colour table: Kagaz asked for true colours, so it is unused.
            1 => {
                u8_of(r)?;
                u16_of(r)?;
                let count = u16_of(r)? as usize;
                bytes_of(r, count * 6)?;
            }
            2 => screen.post(Event::Bell),
            3 => {
                bytes_of(r, 3)?;
                let len = u32_of(r)? as i32;
                if len >= 0 {
                    link.text_in(clip::from_latin1(&bytes_of(r, len as usize)?));
                } else {
                    link.clipboard_in(&bytes_of(r, len.unsigned_abs() as usize)?)?;
                }
            }
            other => return Err(bad(format!("the device sent a message Kagaz does not know ({other})"))),
        }
    }
}

/// Everything that goes to the device: what the person does, and the
/// measurement of the delay.
fn write_loop(rx: mpsc::Receiver<Input>, link: &Link, settings: &Settings) {
    // The key symbol sent for each key that is down, so that its release
    // names the same one whatever Shift did in between.
    let mut held: HashMap<String, u32> = HashMap::new();
    let mut swapped = settings.swapped_buttons;
    let mut picture = settings.picture;
    let mut in_use = picture.unwrap_or(0);
    link.numbers.set_picture(in_use);
    let mut buttons = 0u8;
    let mut next_ping = Instant::now() + Duration::from_millis(300);
    loop {
        let mut batch = Vec::new();
        match rx.recv_timeout(next_ping.saturating_duration_since(Instant::now())) {
            Ok(input) => batch.push(input),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => batch.push(Input::Close),
        }
        while batch.len() < 256 {
            match rx.try_recv() {
                Ok(input) => batch.push(input),
                Err(_) => break,
            }
        }
        let mut out = Vec::new();
        let mut close = false;
        for (i, input) in batch.iter().enumerate() {
            match input {
                Input::Pointer { x, y, buttons: now } => {
                    // Of moves in a row only the last matters.
                    if matches!(batch.get(i + 1), Some(Input::Pointer { buttons: next, .. }) if next == now) {
                        continue;
                    }
                    // The three buttons VNC has; its further ones are the
                    // wheel's.
                    buttons = buttons_for(*now & (LEFT | MIDDLE | RIGHT), swapped);
                    pointer_event(&mut out, buttons, *x, *y);
                }
                Input::Wheel { x, y, up, right } => {
                    // A notch is a press and release of a button of its own.
                    let turns = [(*up, 8u8, 16u8), (*right, 64, 32)];
                    for (notches, forward, back) in turns {
                        let bit = if notches > 0 { forward } else { back };
                        for _ in 0..notches.unsigned_abs().min(20) {
                            pointer_event(&mut out, buttons | bit, *x, *y);
                            pointer_event(&mut out, buttons, *x, *y);
                        }
                    }
                }
                Input::Key { key, code, down } => {
                    let id = if code.is_empty() { key } else { code };
                    let keysym = if *down {
                        keys::keysym(key, code).inspect(|k| {
                            held.insert(id.clone(), *k);
                        })
                    } else {
                        held.remove(id).or_else(|| keys::keysym(key, code))
                    };
                    if let Some(keysym) = keysym {
                        key_event(&mut out, keysym, *down);
                    }
                }
                Input::Clipboard(text) | Input::ClipboardAsked(text) => link.clipboard_out(text, &mut out),
                Input::Files(_) => link.screen.post(Event::Sending(Sending::Failed {
                    why: "VNC carries no files".into(),
                })),
                Input::Picture(p) => picture = *p,
                Input::SwappedButtons(now) => swapped = *now,
                // A VNC server's screen is the size it is, and it sends
                // no sound.
                Input::Resize { .. } | Input::Sound(_) => {}
                // VNC has no word for the lock keys' state.
                Input::Locks { .. } => {}
                Input::Close => close = true,
            }
        }
        if close {
            // Leave no key held down on the device.
            for (_, keysym) in held.drain() {
                key_event(&mut out, keysym, false);
            }
            let _ = link.send(&out);
            link.end("closed");
            return;
        }
        if Instant::now() >= next_ping {
            next_ping = Instant::now() + PING_EVERY;
            let mut ping = link.ping.lock().unwrap_or_else(|e| e.into_inner());
            match *ping {
                Some(left) if left.elapsed() > DEAD_AFTER => {
                    drop(ping);
                    link.end("it stopped answering");
                    return;
                }
                Some(_) => {}
                // One pixel, asked for whether it changed or not: the
                // device answers at once, and the wait is the delay.
                None => {
                    *ping = Some(Instant::now());
                    out.extend_from_slice(&update_request(false, Rect::new(0, 0, 1, 1)));
                }
            }
        }
        // The picture setting: the person's, or what suits the link.
        let want = picture.unwrap_or_else(|| picture_for(link.numbers.read().delay));
        if want != in_use {
            in_use = want;
            link.numbers.set_picture(in_use);
            out.extend(set_encodings(jpeg_level(in_use)));
        }
        if !out.is_empty() && link.send(&out).is_err() {
            link.end("the connection to the device broke");
            return;
        }
    }
}

/// Open a session with the VNC server at `addr`.
pub fn open(addr: SocketAddr, sign_in: &SignIn, settings: &Settings, timeout: Duration) -> Result<Session, OpenError> {
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let numbers = Arc::new(Numbers::default());
    let mut reader = Counted {
        inner: BufReader::with_capacity(1 << 16, stream.try_clone()?),
        numbers: numbers.clone(),
    };
    let hello = handshake(&mut reader, &mut stream, sign_in)?;

    let screen = Arc::new(Screen::new());
    screen.resize(hello.width, hello.height);
    let picture = settings.picture.unwrap_or(0);
    let mut first = vec![0, 0, 0, 0];
    first.extend_from_slice(&PIXEL_FORMAT);
    first.extend(set_encodings(jpeg_level(picture)));
    first.extend_from_slice(&update_request(false, Rect::new(0, 0, hello.width, hello.height)));
    stream.write_all(&first)?;
    numbers.add_sent(first.len());
    // From here on the device may be quiet for as long as nothing moves.
    stream.set_read_timeout(None)?;

    let link = Arc::new(Link {
        out: Mutex::new(stream),
        screen: screen.clone(),
        numbers: numbers.clone(),
        ping: Mutex::new(None),
        delays: Mutex::new(VecDeque::new()),
        clipboard: Mutex::new(Clipboard::default()),
        closing: AtomicBool::new(false),
    });
    let (input, rx) = mpsc::channel();
    {
        let link = link.clone();
        let size = (hello.width, hello.height);
        std::thread::Builder::new()
            .name("vnc-read".into())
            .spawn(move || {
                let why = match read_loop(&mut reader, &link, size) {
                    Err(e) if e.kind() == io::ErrorKind::InvalidData => e.to_string(),
                    _ => "the device closed the connection".into(),
                };
                link.end(&why);
            })
            .map_err(|e| OpenError::Other(e.to_string()))?;
    }
    {
        let settings = settings.clone();
        std::thread::Builder::new()
            .name("vnc-write".into())
            .spawn(move || write_loop(rx, &link, &settings))
            .map_err(|e| OpenError::Other(e.to_string()))?;
    }
    Ok(Session {
        info: Info {
            speaks: Speaks::Vnc,
            name: hello.name,
            encrypted: false,
            sound: false,
            files: false,
            monitors: false,
            resizes: false,
            picture_setting: true,
            identity: None,
        },
        screen,
        numbers,
        input: InputSender::new(input, None),
    })
}

#[cfg(test)]
mod tests;
