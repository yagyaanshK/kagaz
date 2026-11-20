//! The viewer of Computers & phones: sessions open in the window. A
//! session (kagaz-core `remote::session`) keeps the device's screen; a
//! thread here takes what changed and hands it to the page as binary
//! packets over a channel, the way the cameras' pictures travel
//! (media.rs). The page paints them on a canvas and says when it has, so
//! that no more than one picture is ever on its way to it: a slow
//! page skips pictures instead of falling behind.
//!
//! A packet starts with its kind:
//!   1  a picture update: sequence number (u32), count (u32), then pieces
//!      of 16 bytes each (kind, 3 unused, x, y, width, height, from x,
//!      from y, all u16) followed, for a picture, by its RGBA pixels.
//!      Piece kinds: 1 the screen's size, 2 a picture, 3 a move, 4 a
//!      picture of the device's H.264 stream, as it came (for a page that
//!      decodes it, see `video` in `Asking`): its second byte has 1 to
//!      paint it and 2 for a stream that starts over, x and y are where
//!      the picture's top left goes, width is the number of areas it
//!      paints; then the stream's length (u32), each area (x, y, width,
//!      height, u16), and the stream (Annex B).
//!   2  news as JSON: {"type": "clipboard" | "bell" | "pointer-default" |
//!      "ended", ...}
//!   3  the pointer's shape: 3 unused, width, height, hot x, hot y (u16),
//!      then its RGBA pixels.
//! Numbers are little-endian.

use super::blocking;
use super::remote::{path_states, PathState};
use super::remote_vault as vault;
use kagaz_core::remote::session::{self, Numbers, Reading};
use kagaz_core::remote::{
    self, Devices, Event, Info, Input, InputSender, LocateError, OpenError, PathKind, Piece, Rect, Settings, SignIn, Trust,
    View,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager};

/// Linux: a remote screen's pictures go to the page through a WebSocket on
/// this computer's own loopback, not through the window's channel. In
/// WebKitGTK the channel hands each large message over by the page
/// fetching it, about 25 to 30 ms for a whole 1920x1080 picture against
/// about 15 ms pushed down a WebSocket (measured 2026-11-19, WebKitGTK
/// 2.52); small ones take under a millisecond either way. Lag comes first.
///
/// Only the page that was given its address can open it: the address has
/// a key nobody else has seen, it listens on loopback alone, and for one
/// page, once.
#[cfg(target_os = "linux")]
mod pipe {
    use base64::Engine as _;
    use sha1::Digest as _;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    pub struct Pipe {
        listener: TcpListener,
        key: String,
    }

    pub struct Socket(TcpStream);

    pub fn open() -> Option<Pipe> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
        listener.set_nonblocking(true).ok()?;
        let key = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        Some(Pipe { listener, key })
    }

    impl Pipe {
        pub fn address(&self) -> Option<String> {
            Some(format!("ws://127.0.0.1:{}/{}", self.listener.local_addr().ok()?.port(), self.key))
        }

        /// The page, once it has opened it; none if it does not within
        /// `within` (the channel carries the pictures then).
        pub fn accept(self, within: Duration) -> Option<Socket> {
            let end = Instant::now() + within;
            while Instant::now() < end {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        if let Some(socket) = self.handshake(stream) {
                            return Some(socket);
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
            None
        }

        fn handshake(&self, stream: TcpStream) -> Option<Socket> {
            stream.set_nonblocking(false).ok()?;
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
            let _ = stream.set_nodelay(true);
            let mut reader = BufReader::new(stream.try_clone().ok()?);
            let mut first = String::new();
            reader.read_line(&mut first).ok()?;
            if first.split(' ').nth(1) != Some(&format!("/{}", self.key)) {
                return None;
            }
            let mut client_key = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).ok()? == 0 {
                    return None;
                }
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("sec-websocket-key") {
                        client_key = Some(value.trim().to_string());
                    }
                }
            }
            let mut hash = sha1::Sha1::new();
            hash.update(client_key?.as_bytes());
            hash.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            let accept = base64::engine::general_purpose::STANDARD.encode(hash.finalize());
            let mut stream = stream;
            stream
                .write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes())
                .ok()?;
            stream.set_read_timeout(None).ok()?;
            // The page sends nothing but a goodbye; read it so the
            // connection can close.
            std::thread::spawn(move || {
                let mut sink = [0u8; 256];
                while matches!(std::io::Read::read(&mut reader.get_mut(), &mut sink), Ok(n) if n > 0) {}
            });
            Some(Socket(stream))
        }
    }

    impl Socket {
        /// One binary message; false once the page has gone.
        pub fn send(&mut self, data: &[u8]) -> bool {
            let mut head = vec![0x82];
            match data.len() {
                n if n < 126 => head.push(n as u8),
                n if n < 65536 => {
                    head.push(126);
                    head.extend_from_slice(&(n as u16).to_be_bytes());
                }
                n => {
                    head.push(127);
                    head.extend_from_slice(&(n as u64).to_be_bytes());
                }
            }
            self.0.write_all(&head).is_ok() && self.0.write_all(data).is_ok()
        }
    }

    /// Done with it: the connection ends for both handles, so the page
    /// sees it close and the thread reading its goodbye stops too.
    impl Drop for Socket {
        fn drop(&mut self) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// The page may be this many pictures behind before Kagaz waits for it:
/// one, so that a picture never waits behind another (lag comes first;
/// what changes meanwhile is merged into the next).
const AHEAD: u32 = 1;

struct Open {
    input: InputSender,
    /// The session's screen, told when the page has shown a picture.
    screen: Arc<remote::Screen>,
    numbers: Arc<Numbers>,
    /// Pictures sent that the page has not painted yet.
    unpainted: Arc<(Mutex<u32>, Condvar)>,
    stop: Arc<AtomicBool>,
    /// The window that shows it.
    window: String,
    /// The device, where it was reached, and who signed in (for files
    /// sent over SSH, with the same account).
    device: String,
    address: std::net::IpAddr,
    user: Option<String>,
    password: Option<String>,
    /// The session's folder of files sent through a drive, where it has one.
    sent: Option<std::path::PathBuf>,
}

/// The sessions open, in whichever window each is shown.
#[derive(Default)]
pub struct Viewers {
    list: Mutex<HashMap<u64, Open>>,
    next: AtomicU64,
}

impl Viewers {
    fn with<T>(&self, session: u64, f: impl FnOnce(&Open) -> T) -> Result<T, String> {
        let list = self.list.lock().map_err(|e| e.to_string())?;
        list.get(&session).map(f).ok_or_else(|| "this session has ended".to_string())
    }
}

/// Why a session did not open, for the page to act on.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Failed {
    /// The device asks for a password.
    Password,
    /// It asks who signs in: a user name and a password.
    User,
    /// It shows another certificate than the one remembered.
    Certificate,
    /// `remembered`: it was the one from the password store.
    WrongPassword { remembered: bool },
    /// No path answered: why, and how each did.
    Unreachable { why: String, paths: Vec<PathState> },
    Other { why: String },
}

impl Failed {
    fn other(e: impl ToString) -> Failed {
        Failed::Other { why: e.to_string() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Connected {
    session: u64,
    info: Info,
    width: u16,
    height: u16,
    /// The path in use ("Local network", "Tailscale") and where it led.
    path: String,
    address: String,
    millis: u32,
    /// Nobody on the networks in between can read it: the session protects
    /// itself, or it travels through Tailscale.
    protected: bool,
    settings: Settings,
    /// Where the page takes the pictures from, if not the channel (see
    /// `pipe`).
    pipe: Option<String>,
}

fn piece_head(out: &mut Vec<u8>, kind: u8, rect: Rect, from: (u16, u16)) {
    out.extend_from_slice(&[kind, 0, 0, 0]);
    for v in [rect.x, rect.y, rect.width, rect.height, from.0, from.1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// One update for the page: every piece, in order.
fn update_packet(seq: u32, pieces: &[Piece]) -> Vec<u8> {
    let pixels: usize = pieces
        .iter()
        .map(|p| match p {
            Piece::Picture { rgba, .. } => rgba.len(),
            Piece::Video(v) => 4 + v.regions.len() * 8 + v.stream.len(),
            _ => 0,
        })
        .sum();
    let mut out = Vec::with_capacity(12 + pieces.len() * 16 + pixels);
    out.extend_from_slice(&[1, 0, 0, 0]);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&(pieces.len() as u32).to_le_bytes());
    for piece in pieces {
        match piece {
            Piece::Size { width, height } => piece_head(&mut out, 1, Rect::new(0, 0, *width, *height), (0, 0)),
            Piece::Picture { rect, rgba } => {
                piece_head(&mut out, 2, *rect, (0, 0));
                out.extend_from_slice(rgba);
            }
            Piece::Move { from_x, from_y, rect } => piece_head(&mut out, 3, *rect, (*from_x, *from_y)),
            Piece::Video(v) => {
                let at = out.len();
                piece_head(&mut out, 4, Rect::new(v.origin_x, v.origin_y, v.regions.len() as u16, 0), (0, 0));
                out[at + 1] = u8::from(v.paint) | (u8::from(v.restart) << 1);
                out.extend_from_slice(&(v.stream.len() as u32).to_le_bytes());
                for r in &v.regions {
                    for n in [r.x, r.y, r.width, r.height] {
                        out.extend_from_slice(&n.to_le_bytes());
                    }
                }
                out.extend_from_slice(&v.stream);
            }
        }
    }
    out
}

fn news_packet(news: serde_json::Value) -> Vec<u8> {
    let mut out = vec![2];
    out.extend_from_slice(news.to_string().as_bytes());
    out
}

fn pointer_packet(p: &remote::Pointer) -> Vec<u8> {
    let mut out = vec![3, 0, 0, 0];
    for v in [p.width, p.height, p.hot_x, p.hot_y] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&p.rgba);
    out
}

// ---- this computer's clipboard ----

/// What is on this computer's clipboard that a session carries: its
/// text, and its picture as PNG (a screenshot, an image out of a page).
#[derive(Default)]
struct Copied {
    text: Option<String>,
    png: Option<Vec<u8>>,
}

/// The largest picture taken off the clipboard here.
const MOST_PICTURE: usize = 64 << 20;

#[cfg(target_os = "linux")]
fn clipboard_now(app: &AppHandle) -> Copied {
    let (tx, rx) = mpsc::channel();
    let asked = app.run_on_main_thread(move || {
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        let text = clipboard.wait_for_text().map(|t| t.to_string()).filter(|t| !t.is_empty());
        // As PNG where the program that copied it has it so (most do);
        // else whatever picture it has, made into PNG here.
        let png = clipboard
            .wait_for_contents(&gtk::gdk::Atom::intern("image/png"))
            .map(|contents| contents.data())
            .filter(|png| !png.is_empty())
            .or_else(|| {
                clipboard
                    .wait_is_image_available()
                    .then(|| clipboard.wait_for_image())
                    .flatten()
                    .and_then(|picture| picture.save_to_bufferv("png", &[]).ok())
            })
            .filter(|png| png.len() <= MOST_PICTURE);
        let _ = tx.send(Copied { text, png });
    });
    if asked.is_err() {
        return Copied::default();
    }
    rx.recv_timeout(Duration::from_secs(4)).unwrap_or_default()
}

/// Put what was copied on the device on this computer's clipboard: its
/// text, or its picture (as PNG, which programs here read) with the text
/// that came with it.
#[cfg(target_os = "linux")]
fn set_clipboard(app: &AppHandle, text: Option<String>, png: Option<Vec<u8>>) -> bool {
    app.run_on_main_thread(move || {
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        let Some(png) = png else {
            if let Some(text) = text {
                clipboard.set_text(&text);
            }
            return;
        };
        let mut targets = vec![gtk::TargetEntry::new("image/png", gtk::TargetFlags::empty(), 1)];
        if text.is_some() {
            for name in ["UTF8_STRING", "text/plain;charset=utf-8", "text/plain", "STRING", "TEXT"] {
                targets.push(gtk::TargetEntry::new(name, gtk::TargetFlags::empty(), 0));
            }
        }
        // Handed over when something here pastes it, in the form asked for.
        clipboard.set_with_data(&targets, move |_, selection, form| {
            if form == 1 {
                selection.set(&gtk::gdk::Atom::intern("image/png"), 8, &png);
            } else if let Some(text) = &text {
                selection.set_text(text);
            }
        });
    })
    .is_ok()
}

/// Put files on this computer's clipboard, as copied files: a paste in a
/// folder (Files, the desktop) copies them there.
#[cfg(target_os = "linux")]
fn set_clipboard_files(app: &AppHandle, paths: Vec<String>) -> bool {
    let uris: Vec<String> = paths.iter().map(|p| file_uri(p)).collect();
    app.run_on_main_thread(move || {
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        let targets = [
            gtk::TargetEntry::new("x-special/gnome-copied-files", gtk::TargetFlags::empty(), 0),
            gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 1),
        ];
        clipboard.set_with_data(&targets, move |_, selection, form| {
            let (atom, text) = if form == 0 {
                ("x-special/gnome-copied-files", format!("copy\n{}", uris.join("\n")))
            } else {
                ("text/uri-list", uris.iter().map(|u| format!("{u}\r\n")).collect())
            };
            selection.set(&gtk::gdk::Atom::intern(atom), 8, text.as_bytes());
        });
    })
    .is_ok()
}

#[cfg(not(target_os = "linux"))]
fn set_clipboard_files(_app: &AppHandle, _paths: Vec<String>) -> bool {
    false
}

// The clipboard of the other systems comes with their viewers.
#[cfg(not(target_os = "linux"))]
fn clipboard_now(_app: &AppHandle) -> Copied {
    Copied::default()
}

#[cfg(not(target_os = "linux"))]
fn set_clipboard(_app: &AppHandle, _text: Option<String>, _png: Option<Vec<u8>>) -> bool {
    false
}

/// Does this build carry the clipboard between here and the device?
const CLIPBOARD_HERE: bool = cfg!(target_os = "linux");

/// Take what changes on the session's screen to the page until either
/// side ends it.
/// The pipe a session's pictures take to the page, where there is one.
#[cfg(target_os = "linux")]
type Pipe = pipe::Pipe;
#[cfg(not(target_os = "linux"))]
type Pipe = ();

fn forward(app: AppHandle, id: u64, session: remote::Session, channel: Channel<InvokeResponseBody>, open: &Open, pipe: Option<Pipe>) {
    let (unpainted, stop) = (open.unpainted.clone(), open.stop.clone());
    std::thread::spawn(move || {
        let mut seq = 0u32;
        // The page opens the pipe as soon as it hears of it; a page that
        // does not leaves the pictures to the channel.
        #[cfg(target_os = "linux")]
        let mut socket = pipe.and_then(|p| p.accept(Duration::from_secs(2)));
        #[cfg(not(target_os = "linux"))]
        let _ = pipe;
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut send = |packet: Vec<u8>| {
            #[cfg(target_os = "linux")]
            if let Some(s) = socket.as_mut() {
                return s.send(&packet);
            }
            channel.send(InvokeResponseBody::Raw(packet)).is_ok()
        };
        'session: while !stop.load(Ordering::Acquire) {
            // Not more than a couple of pictures ahead of the page. A page
            // that says nothing for a second (hidden, busy) is not waited
            // for any longer: the next picture brings it up to date.
            {
                let (count, painted) = &*unpainted;
                let guard = count.lock().unwrap_or_else(|e| e.into_inner());
                let (mut guard, _) = painted
                    .wait_timeout_while(guard, Duration::from_secs(1), |n| *n >= AHEAD)
                    .unwrap_or_else(|e| e.into_inner());
                if *guard >= AHEAD {
                    *guard = 0;
                    // Taken as shown, so that the device is not held back
                    // by a page that is not there to show it.
                    session.screen.shown();
                }
            }
            if !session.screen.wait(Duration::from_millis(250)) {
                continue;
            }
            let taken = session.screen.take();
            if !taken.pieces.is_empty() {
                seq = seq.wrapping_add(1);
                *unpainted.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                if !send(update_packet(seq, &taken.pieces)) {
                    break;
                }
            }
            for event in taken.events {
                let (packet, last) = match event {
                    Event::Clipboard(text) => {
                        let characters = text.chars().count();
                        let kept = set_clipboard(&app, Some(text), None);
                        let news = serde_json::json!({ "type": "clipboard", "characters": characters, "kept": kept });
                        (news_packet(news), false)
                    }
                    Event::ClipboardPicture { png, text } => {
                        let kept = set_clipboard(&app, text, Some(png));
                        let news = serde_json::json!({ "type": "clipboard", "picture": true, "kept": kept });
                        (news_packet(news), false)
                    }
                    Event::Pointer(p) => (pointer_packet(&p), false),
                    Event::PointerDefault => (news_packet(serde_json::json!({ "type": "pointer-default" })), false),
                    Event::Bell => (news_packet(serde_json::json!({ "type": "bell" })), false),
                    Event::Sending(how) => {
                        // Files copied on the device, here now: on this
                        // computer's clipboard, for a paste in a folder.
                        if let remote::Sending::Fetched { paths, .. } = &how {
                            set_clipboard_files(&app, paths.clone());
                        }
                        let mut news = serde_json::to_value(&how).unwrap_or_default();
                        news["type"] = "sending".into();
                        (news_packet(news), false)
                    }
                    Event::Ended(why) => (news_packet(serde_json::json!({ "type": "ended", "why": why })), true),
                };
                if !send(packet) || last {
                    break 'session;
                }
            }
        }
        // Dropping the session closes it.
        drop(session);
        if let Ok(mut list) = app.state::<Viewers>().list.lock() {
            list.remove(&id);
        }
    });
}

/// What the page says when it asks for a session.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Asking {
    /// Typed by the person, when the device asked.
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// With a typed password: keep it in the system's password store if
    /// it is accepted (true), or stop keeping one (false).
    #[serde(default)]
    pub remember: Option<bool>,
    /// The person has seen that the certificate changed and takes the
    /// new one.
    #[serde(default)]
    pub accept_certificate: bool,
    /// The viewer: its size in pixels and its scale in percent.
    #[serde(default)]
    pub view: Option<(u16, u16, u32)>,
    /// The page decodes H.264 itself (its graphics chip does): a device
    /// that sends it has its pictures handed over as they came, not as
    /// pixels (lag comes first).
    #[serde(default)]
    pub video: bool,
}

/// Reach a saved device and open a session with it; its picture then
/// arrives on `channel`.
#[tauri::command]
pub async fn remote_connect(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
    asking: Asking,
    channel: Channel<InvokeResponseBody>,
) -> Result<Connected, Failed> {
    tauri::async_runtime::spawn_blocking(move || {
        let list = Devices::default_path();
        let device = Devices::load(&list)
            .and_then(|d| d.find(&id).cloned())
            .map_err(Failed::other)?;
        let Some(way) = session::way_for(&device) else {
            return Err(Failed::other(format!(
                "{} has no way in that shows its screen (Remote Desktop or VNC)",
                device.name
            )));
        };
        let (device, answer) = match remote::locate(&list, &device.id, way.speaks, Duration::from_millis(2500)) {
            Ok(found) => found,
            Err(LocateError::NotReached(n)) => {
                return Err(Failed::Unreachable {
                    paths: path_states(&device, &n.reports),
                    why: n.why,
                })
            }
            Err(e) => return Err(Failed::other(e)),
        };
        let Asking {
            user,
            password,
            remember,
            accept_certificate,
            view,
            video,
        } = asking;
        // A computer running Kagaz takes a PIN, and an empty one too (the
        // person there is asked); its user is its id there, from pairing.
        let paired = device.paired;
        let typed = if paired { password } else { password.filter(|p| !p.is_empty()) };
        let typed_user = if paired { None } else { user.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) };
        // Nothing typed: the remembered one, if the person asked for that
        // and the store can be reached.
        let kept = match &typed {
            None if device.remember => vault::get(&device.id).ok().flatten(),
            _ => None,
        };
        let remembered = kept.is_some();
        let sign_in = SignIn {
            user: typed_user.clone().or(device.user.clone()),
            password: device.password_from(&list, typed.clone().or(kept)),
        };
        let trust = Trust {
            known: device.identity.certificate.clone(),
            accept_new: accept_certificate,
        };
        let view = view.map(|(width, height, scale)| View { width, height, scale, video });
        let session = session::open(
            answer.addr,
            answer.way,
            &sign_in,
            &device.settings,
            &trust,
            view,
            device.sign_in_wait(Duration::from_secs(10)),
        )
        .map_err(|e| match e {
            OpenError::NeedsPassword => Failed::Password,
            OpenError::NeedsUser => Failed::User,
            OpenError::WrongPassword => Failed::WrongPassword { remembered },
            OpenError::CertificateChanged => Failed::Certificate,
            other => Failed::other(format!("{}: {other}", device.name)),
        })?;
        // It let the person in: who signed in is the device's from now
        // on, and the certificate it showed is the one to expect (the
        // first, or the new one the person took).
        let seen = session.info.identity.clone();
        if typed_user.is_some() || seen.is_some() {
            let _ = Devices::update(&list, |d| {
                if let Some(seen) = &seen {
                    if accept_certificate {
                        d.trust_identity(&device.id, seen);
                    } else {
                        d.note_identity(&device.id, seen);
                    }
                }
                if let (Some(user), Some(dev)) = (&typed_user, d.devices.iter_mut().find(|x| x.id == device.id)) {
                    dev.user = Some(user.clone());
                }
                Ok(())
            });
        }
        // It was accepted: keep it, or stop keeping one, as asked.
        if let (Some(password), Some(keep)) = (typed.as_ref().filter(|p| !p.is_empty()), remember) {
            let stored = if keep { vault::set(&device.id, password) } else { vault::forget(&device.id) };
            if stored.is_ok() && device.remember != keep {
                let _ = Devices::update(&list, |d| {
                    if let Some(dev) = d.devices.iter_mut().find(|x| x.id == device.id) {
                        dev.remember = keep;
                    }
                    Ok(())
                });
            }
        }
        let viewers = app.state::<Viewers>();
        let id = viewers.next.fetch_add(1, Ordering::SeqCst) + 1;
        // The page says when it has shown each picture: the session holds
        // the device back until it has (lag comes first).
        session.screen.pace();
        let open = Open {
            input: session.sender(),
            screen: session.screen.clone(),
            numbers: session.numbers.clone(),
            unpainted: Arc::new((Mutex::new(0), Condvar::new())),
            stop: Arc::new(AtomicBool::new(false)),
            window: window.label().to_string(),
            device: device.id.clone(),
            address: answer.addr.ip(),
            user: sign_in.user.clone(),
            password: if device.paired { None } else { sign_in.password.clone() },
            sent: session.info.sent.clone(),
        };
        let (width, height) = session.screen.size();
        let path = &device.paths[answer.path];
        let connected = Connected {
            session: id,
            info: session.info.clone(),
            width,
            height,
            path: path.kind.label().to_string(),
            address: answer.addr.ip().to_string(),
            millis: answer.millis,
            protected: session.info.encrypted || path.kind == PathKind::Tailscale,
            settings: device.settings.clone(),
            pipe: None,
        };
        #[cfg(target_os = "linux")]
        let pipe = pipe::open();
        #[cfg(not(target_os = "linux"))]
        let pipe: Option<Pipe> = None;
        #[cfg(target_os = "linux")]
        let connected = Connected { pipe: pipe.as_ref().and_then(|p| p.address()), ..connected };
        forward(app.clone(), id, session, channel, &open, pipe);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(sent) = open.sent.clone() {
            watch_arrivals(app.clone(), id, sent);
        }
        viewers.list.lock().map_err(Failed::other)?.insert(id, open);
        Ok(connected)
    })
    .await
    .map_err(Failed::other)?
}

/// What the person does in the viewer, as the page tells it.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Did {
    Pointer { x: u16, y: u16, buttons: u8 },
    Wheel { x: u16, y: u16, up: i16, right: i16 },
    Key { key: String, code: String, down: bool },
    /// Sharper (0) to smoother (10); none lets Kagaz follow the link.
    Picture { value: Option<u8> },
    /// The device has its mouse buttons swapped.
    Swapped { on: bool },
    /// Play the device's sound here, or not, for now.
    Sound { on: bool },
    /// The lock keys as they are on this keyboard.
    Locks { caps: bool, num: bool, scroll: bool },
    /// The viewer is this size now, in pixels.
    Resize { width: u16, height: u16 },
}

#[tauri::command]
pub fn remote_input(viewers: tauri::State<'_, Viewers>, session: u64, did: Vec<Did>) -> Result<(), String> {
    viewers.with(session, |open| {
        for d in did {
            open.input.send(match d {
                Did::Pointer { x, y, buttons } => Input::Pointer { x, y, buttons },
                Did::Wheel { x, y, up, right } => Input::Wheel { x, y, up, right },
                Did::Key { key, code, down } => Input::Key { key, code, down },
                Did::Picture { value } => Input::Picture(value.map(|v| v.min(10))),
                Did::Swapped { on } => Input::SwappedButtons(on),
                Did::Sound { on } => Input::Sound(on),
                Did::Locks { caps, num, scroll } => Input::Locks { caps, num, scroll },
                Did::Resize { width, height } => Input::Resize { width, height },
            });
        }
    })
}

/// The page painted a picture: one more may be sent.
#[tauri::command]
pub fn remote_painted(viewers: tauri::State<'_, Viewers>, session: u64) -> Result<(), String> {
    viewers.with(session, |open| {
        let (count, painted) = &*open.unpainted;
        let mut n = count.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        painted.notify_all();
        open.screen.shown();
        open.input.send(Input::Shown);
    })
}

/// The page could not decode the device's H.264 after all: pixels from
/// now on, starting with all of the screen.
#[tauri::command]
pub fn remote_video_off(viewers: tauri::State<'_, Viewers>, session: u64) -> Result<(), String> {
    viewers.with(session, |open| open.screen.video_off())
}

/// The session in numbers: the delay, what travelled, the picture setting.
#[tauri::command]
pub fn remote_numbers(viewers: tauri::State<'_, Viewers>, session: u64) -> Result<Told, String> {
    viewers.with(session, |open| Told {
        reading: open.numbers.read(),
        via: open.numbers.via(),
        sound_via: open.numbers.sound_via(),
    })
}

/// The numbers, and in a few words how the picture travels (which of
/// Remote Desktop's pictures, by which codecs), where the session knows.
#[derive(Serialize)]
pub struct Told {
    #[serde(flatten)]
    reading: Reading,
    via: String,
    /// The same for its sound: which channel, which format, how much.
    sound_via: String,
}

/// What was offered to the device from this computer's clipboard.
#[derive(Serialize, Default)]
pub struct Offered {
    /// So many characters of text.
    characters: Option<usize>,
    /// A picture (with the text, where both were copied together).
    picture: bool,
}

/// Offer what is on this computer's clipboard to the device: its text,
/// its picture, or both where they were copied together. `asked` when the
/// person pressed the button for it: text is then offered even over
/// files offered since it was last said.
#[tauri::command]
pub async fn remote_clipboard_push(app: AppHandle, session: u64, asked: Option<bool>) -> Result<Offered, String> {
    if !CLIPBOARD_HERE {
        return Err("the clipboard is not carried on this system yet".into());
    }
    blocking(move || {
        let copied = clipboard_now(&app);
        let characters = copied.text.as_ref().map(|t| t.chars().count());
        let (input, picture) = match copied {
            Copied { png: Some(png), text } => (Input::ClipboardPicture { png, text }, true),
            Copied { text: Some(text), .. } if asked == Some(true) => (Input::ClipboardAsked(text), false),
            Copied { text: Some(text), .. } => (Input::Clipboard(text), false),
            _ => return Ok(Offered::default()),
        };
        app.state::<Viewers>().with(session, |open| open.input.send(input))?;
        Ok(Offered { characters, picture })
    })
    .await
}

/// A folder of this computer to share with a device, chosen by the
/// person; none when they chose nothing. A phone's dialog has no folder
/// chooser, so there it is always none.
#[tauri::command]
pub async fn remote_pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    #[cfg(desktop)]
    return blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let picked = app.dialog().file().set_title("A folder to share with the device").blocking_pick_folder();
        Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().to_string()))
    })
    .await;
    #[cfg(mobile)]
    {
        let _ = app;
        Ok(None)
    }
}

/// How files were sent, for the page to say.
#[derive(Serialize, Default)]
pub struct Sent {
    count: usize,
    /// "clipboard" (waiting to be pasted there), "ask" (the computer asks
    /// where to save them), "open" (their folder opens there), "drop"
    /// (saved where they were let go there), "ssh" (copied there into
    /// `folder`).
    how: String,
    folder: Option<String>,
}

/// Send files to the device, the way chosen for it (`how`, else its
/// setting): on its clipboard to be pasted; through the session's Kagaz
/// drive, with the computer asking where to save them or opening their
/// folder; or over SSH into a folder there. Without `paths` the person is
/// asked which (folders too, where they can go). A computer running Kagaz
/// takes them by itself. Files dropped on its screen here come with the
/// point they were let go at (`at`, in the screen's pixels there): with
/// a drive, the computer saves them where that is.
#[tauri::command]
pub async fn remote_send_files(
    app: AppHandle,
    session: u64,
    paths: Option<Vec<String>>,
    how: Option<String>,
    at: Option<(i32, i32)>,
) -> Result<Sent, String> {
    blocking(move || {
        let (device_id, sent, window, input, numbers) = app
            .state::<Viewers>()
            .with(session, |o| (o.device.clone(), o.sent.clone(), o.window.clone(), o.input.clone(), o.numbers.clone()))?;
        let device = Devices::load(&Devices::default_path()).ok().and_then(|d| d.find(&device_id).ok().cloned());
        let paired = device.as_ref().is_some_and(|d| d.paired);
        let chosen = how.or_else(|| device.as_ref().and_then(|d| d.settings.send_files.clone()));
        let how = match chosen.as_deref() {
            _ if paired => "clipboard",
            // Dropped: where they were let go, unless they are to be
            // pasted there.
            Some("clipboard") => "clipboard",
            _ if at.is_some() && sent.is_some() => "drop",
            Some("ask" | "open") if sent.is_some() => chosen.as_deref().unwrap_or("ask"),
            Some("ssh") => "ssh",
            _ if sent.is_some() => "ask",
            _ => "clipboard",
        }
        .to_string();
        let paths: Vec<std::path::PathBuf> = match paths {
            Some(paths) => paths.into_iter().map(Into::into).collect(),
            None => {
                use tauri_plugin_dialog::DialogExt;
                let picked = app.dialog().file().set_title("Send to the device").blocking_pick_files();
                picked.unwrap_or_default().into_iter().filter_map(|p| p.into_path().ok()).collect()
            }
        };
        if paths.is_empty() {
            return Ok(Sent::default());
        }
        let count = paths.len();
        match how.as_str() {
            "clipboard" => {
                input.send(Input::Files(paths));
                Ok(Sent { count, how, folder: None })
            }
            "ssh" => {
                let folder = device
                    .as_ref()
                    .and_then(|d| d.settings.ssh_folder.clone())
                    .filter(|f| !f.trim().is_empty())
                    .unwrap_or_else(|| "Downloads".into());
                over_ssh(&app, session, device.as_ref().ok_or("this device is no longer saved")?, &paths, &folder)?;
                Ok(Sent { count, how, folder: Some(folder) })
            }
            _ => {
                let what = if how == "open" { "open" } else { "save" };
                let at = if how == "drop" { at } else { None };
                through_drive(&app, &window, &input, &numbers, sent.as_deref(), &paths, what, at).map(|_| Sent { count, how, folder: None })
            }
        }
    })
    .await
}

/// Through the session's Kagaz drive: the files put in a batch folder of
/// its own, the drive shown there, and Kagaz's helper there asked to do
/// `what` with them ("save", where `at` is if let go there, or "open"),
/// started first if it is not running.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn through_drive(
    app: &AppHandle,
    window: &str,
    input: &kagaz_core::remote::InputSender,
    numbers: &Numbers,
    sent: Option<&std::path::Path>,
    paths: &[std::path::PathBuf],
    what: &str,
    at: Option<(i32, i32)>,
) -> Result<(), String> {
    let sent = sent.ok_or("this session has no drive for files")?;
    let batch = stage_batch(app, window, sent, paths)?;
    input.send(Input::ShowSent);
    kagaz_core::remote::rdp::ask(sent, what, &batch, at)?;
    helper_started(app, window, input, numbers, sent)
}

/// The files put in a batch folder of their own on the drive, large ones
/// said to the page as they get ready. The batch's name.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn stage_batch(app: &AppHandle, window: &str, sent: &std::path::Path, paths: &[std::path::PathBuf]) -> Result<String, String> {
    use tauri::Emitter;
    let mut told = std::time::Instant::now();
    kagaz_core::remote::rdp::stage(sent, paths, |done, total| {
        if told.elapsed() > Duration::from_millis(250) || done == total {
            told = std::time::Instant::now();
            let _ = app.emit_to(window, "remote-send-ready", serde_json::json!({ "done": done, "total": total }));
        }
    })
}

/// Kagaz's helper on the computer, started if it is not running (it says
/// it is, every two seconds, on the drive): by Kagaz share there where it
/// is turned on (it says so on the drive), else from Windows' Run box. A
/// Run box opened in the last quarter minute is the person's to run: they
/// are reminded. After that, one closed unrun (Cancel, on a real PC)
/// leaves nothing running, so the Run box opens again, and what was asked
/// before it is dropped rather than done long after.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn helper_started(
    app: &AppHandle,
    window: &str,
    input: &kagaz_core::remote::InputSender,
    numbers: &Numbers,
    sent: &std::path::Path,
) -> Result<(), String> {
    use kagaz_core::remote::rdp::{drop_stale, helper_alive, helper_run_line, helper_started_there};
    use tauri::Emitter;
    static STARTED: Mutex<Vec<(std::path::PathBuf, std::time::Instant)>> = Mutex::new(Vec::new());
    const WAITS: Duration = Duration::from_secs(15);
    if helper_alive(sent) {
        return Ok(());
    }
    {
        let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
        if helper_started_there(sent) {
            // Kagaz share there starts it (or has, and it is starting).
            return Ok(());
        }
        let before = started.iter().find(|(dir, _)| dir == sent).map(|(_, when)| when.elapsed());
        if before.is_some_and(|ago| ago < WAITS) {
            let _ = app.emit_to(window, "remote-run-box", ());
            return Ok(());
        }
        if before.is_some() {
            drop_stale(sent, WAITS);
        }
        started.retain(|(dir, _)| dir != sent);
        started.push((sent.to_path_buf(), std::time::Instant::now()));
    }
    // Kagaz share there sees the drive within a second or two of its
    // showing.
    let until = std::time::Instant::now() + Duration::from_millis(2500);
    while std::time::Instant::now() < until {
        if helper_alive(sent) || helper_started_there(sent) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let ran = run_there(app, window, input, numbers, &helper_run_line());
    if ran.is_err() {
        let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
        started.retain(|(dir, _)| dir != sent);
    }
    ran
}

/// Files from the computer (its drop zone, its clipboard): Kagaz's helper
/// there puts each lot in `.kagaz/in-<id>/` on the drive, then says so
/// with `in-<id>.txt`; each lot goes where the device's setting says
/// (`arrived`). Looked for while the session lasts; the drive is a folder
/// of this computer, so looking costs nothing on the way.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn watch_arrivals(app: AppHandle, session: u64, sent: std::path::PathBuf) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(400));
        let Ok((device, window)) = app.state::<Viewers>().with(session, |o| (o.device.clone(), o.window.clone())) else {
            return;
        };
        let dir = sent.join(".kagaz");
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_prefix("in-").and_then(|n| n.strip_suffix(".txt")) else { continue };
            let lot = dir.join(format!("in-{id}"));
            let _ = std::fs::remove_file(e.path());
            arrived(&app, &window, &device, &lot);
            let _ = std::fs::remove_dir_all(&lot);
        }
    });
}

/// A lot of files from the computer, moved where its device's setting
/// says: Downloads, a folder chosen once, or (unless said) a folder the
/// person chooses now. The page is told where they went.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn arrived(app: &AppHandle, window: &str, device_id: &str, lot: &std::path::Path) {
    use tauri::Emitter;
    let count = std::fs::read_dir(lot).map(|e| e.count()).unwrap_or(0);
    if count == 0 {
        return;
    }
    let device = Devices::load(&Devices::default_path()).ok().and_then(|d| d.find(device_id).ok().cloned());
    let name = device.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| "the computer".into());
    let settings = device.map(|d| d.settings).unwrap_or_default();
    let downloads = app.path().download_dir().ok();
    let into = match settings.receive_files.as_deref() {
        Some("downloads") => downloads,
        Some("folder") => settings.receive_folder.map(std::path::PathBuf::from).filter(|p| p.is_dir()).or(downloads),
        _ => {
            use tauri_plugin_dialog::DialogExt;
            let what = if count == 1 { "a file".to_string() } else { format!("{count} files") };
            let mut chooser = app.dialog().file().set_title(format!("Where to save {what} from {name}"));
            if let Some(d) = &downloads {
                chooser = chooser.set_directory(d);
            }
            chooser.blocking_pick_folder().and_then(|p| p.into_path().ok())
        }
    };
    let Some(into) = into else {
        let _ = app.emit_to(window, "remote-received", serde_json::json!({ "count": count, "kept": false }));
        return;
    };
    match kagaz_core::remote::rdp::bring_in(lot, &into) {
        Ok(placed) => {
            let _ = app.emit_to(
                window,
                "remote-received",
                serde_json::json!({ "count": count, "kept": true, "folder": into, "first": placed.first() }),
            );
        }
        Err(e) => {
            let _ = app.emit_to(window, "remote-received", serde_json::json!({ "count": count, "kept": false, "why": e.to_string() }));
        }
    }
}

/// Files copied on the device, fetched to here for this computer's
/// clipboard (the window lost the focus: the person may paste them
/// elsewhere); `big`: whatever their size (the person said so).
#[tauri::command]
pub fn remote_fetch_their_files(app: AppHandle, session: u64, big: Option<bool>) -> Result<(), String> {
    app.state::<Viewers>().with(session, |o| o.input.send(Input::TheirFiles { big: big.unwrap_or(false) }))
}

/// A drag made on the computer has left the session's window here with
/// the button still held: Kagaz's helper there is asked to catch it at
/// the pointer (held at the screen's edge). Where it carries files, their
/// count comes back, and a drag of them begins here (this computer's
/// own, from this window, while the button is still down): the page then
/// lets go there, so the computer's drop lands on the helper, which has
/// Windows copy them onto the Kagaz drive; let go here (a folder, the
/// desktop), they are handed over as soon as they are all here. None:
/// nothing to carry (no files dragged, no helper running there).
#[tauri::command]
pub async fn remote_drag_out(window: tauri::WebviewWindow, session: u64) -> Result<Option<usize>, String> {
    let app = window.app_handle().clone();
    blocking(move || {
        #[cfg(target_os = "linux")]
        {
            use kagaz_core::remote::rdp::{answer, ask, cancel, helper_alive};
            let sent = app.state::<Viewers>().with(session, |o| o.sent.clone())?;
            let Some(sent) = sent.filter(|s| helper_alive(s)) else { return Ok(None) };
            let id = ask(&sent, "catch", "", None)?;
            let until = std::time::Instant::now() + Duration::from_secs(4);
            let caught = loop {
                if let Some(n) = answer(&sent, "caught", &id) {
                    break n;
                }
                if answer(&sent, "done", &id).is_some() || std::time::Instant::now() > until {
                    break 0;
                }
                std::thread::sleep(Duration::from_millis(30));
            };
            if caught <= 0 {
                cancel(&sent, &id);
                return Ok(None);
            }
            drag_out_here(&window, sent, id)?;
            Ok(Some(caught as usize))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (app, window, session);
            Ok(None)
        }
    })
    .await
}

/// A drag here of files on their way from the computer (into the
/// drive's `out-<id>`), begun from the session's window while the button
/// is down, for other programs only (Files, the desktop). Where it is let
/// go, the files are handed over once they are all here; until then that
/// program waits (two minutes at most).
#[cfg(target_os = "linux")]
fn drag_out_here(window: &tauri::WebviewWindow, sent: std::path::PathBuf, id: String) -> Result<(), String> {
    window
        .with_webview(move |webview| {
            use gtk::prelude::*;
            use std::cell::RefCell;
            use std::rc::Rc;
            let view = webview.inner();
            let handlers: Rc<RefCell<Vec<gtk::glib::SignalHandlerId>>> = Rc::default();
            let (from, which) = (sent.clone(), id.clone());
            let gives = view.connect_drag_data_get(move |_, _, selection, _, _| {
                let until = std::time::Instant::now() + Duration::from_secs(120);
                while kagaz_core::remote::rdp::answer(&from, "out", &which).is_none() && std::time::Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(50));
                }
                let dir = from.join(".kagaz").join(format!("out-{which}"));
                let mut uris: Vec<String> = std::fs::read_dir(&dir)
                    .map(|e| e.flatten().map(|e| file_uri(&e.path().to_string_lossy())).collect())
                    .unwrap_or_default();
                uris.sort();
                let uris: Vec<&str> = uris.iter().map(String::as_str).collect();
                selection.set_uris(&uris);
            });
            handlers.borrow_mut().push(gives);
            let all = handlers.clone();
            let ends = view.connect_drag_end(move |view, _| {
                for handler in all.borrow_mut().drain(..) {
                    view.disconnect(handler);
                }
            });
            handlers.borrow_mut().push(ends);
            let targets = gtk::TargetList::new(&[gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::OTHER_APP, 0)]);
            if view.drag_begin_with_coordinates(&targets, gtk::gdk::DragAction::COPY, 1, None, -1, -1).is_none() {
                for handler in handlers.borrow_mut().drain(..) {
                    view.disconnect(handler);
                }
            }
        })
        .map_err(|e| e.to_string())
}

/// A file:// address, with what is not plain in a path escaped.
#[cfg(target_os = "linux")]
fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Whether Kagaz's helper runs on the computer (for files from it: its
/// drop zone), or Kagaz share there starts it; false where there is no
/// drive (no Remote Desktop to Windows, a computer running Kagaz).
#[tauri::command]
pub fn remote_helper_running(app: AppHandle, session: u64) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let sent = app.state::<Viewers>().with(session, |o| o.sent.clone()).ok().flatten();
        sent.is_some_and(|s| kagaz_core::remote::rdp::helper_alive(&s) || kagaz_core::remote::rdp::helper_started_there(&s))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (app, session);
        false
    }
}

/// Kagaz's helper started on the computer (the person asked: to send
/// files from it), from Windows' Run box unless it runs already.
#[tauri::command]
pub async fn remote_helper_start(app: AppHandle, session: u64) -> Result<(), String> {
    blocking(move || {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let (sent, window, input, numbers) =
                app.state::<Viewers>().with(session, |o| (o.sent.clone(), o.window.clone(), o.input.clone(), o.numbers.clone()))?;
            let sent = sent.ok_or("this session has no drive for files")?;
            input.send(Input::ShowSent);
            helper_started(&app, &window, &input, &numbers, &sent)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (app, session);
            Err("this comes to this system later".into())
        }
    })
    .await
}

/// Files being dragged over a session's screen (see `remote_drag_begin`):
/// the drag's id, its session's drive, its batch.
static DRAGS: Mutex<Vec<(String, std::path::PathBuf, String)>> = Mutex::new(Vec::new());

/// Files dragged from here over a Windows computer's screen: put on its
/// Kagaz drive, and Kagaz's helper there asked to hold a real Windows
/// drag of them at the pointer (started first if need be). When it does,
/// "remote-drag-ready" tells the page, which presses the button there,
/// moves, and lets go: wherever that is takes them as any drop. The
/// drag's id; none where files go another way (a computer running Kagaz,
/// one whose files are to be pasted, a session without a drive).
#[tauri::command]
pub async fn remote_drag_begin(app: AppHandle, session: u64, paths: Vec<String>) -> Result<Option<String>, String> {
    blocking(move || {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use tauri::Emitter;
            let (device_id, sent, window, input, numbers) = app
                .state::<Viewers>()
                .with(session, |o| (o.device.clone(), o.sent.clone(), o.window.clone(), o.input.clone(), o.numbers.clone()))?;
            let Some(sent) = sent else { return Ok(None) };
            let device = Devices::load(&Devices::default_path()).ok().and_then(|d| d.find(&device_id).ok().cloned());
            if device.as_ref().is_some_and(|d| d.paired || d.settings.send_files.as_deref() == Some("clipboard")) {
                return Ok(None);
            }
            let paths: Vec<std::path::PathBuf> = paths.into_iter().map(Into::into).collect();
            let batch = stage_batch(&app, &window, &sent, &paths)?;
            input.send(Input::ShowSent);
            let id = kagaz_core::remote::rdp::ask(&sent, "drag", &batch, None)?;
            DRAGS.lock().unwrap_or_else(|e| e.into_inner()).push((id.clone(), sent.clone(), batch));
            let drag = id.clone();
            std::thread::spawn(move || {
                let started = helper_started(&app, &window, &input, &numbers, &sent);
                if let Err(e) = started {
                    let _ = app.emit_to(&window, "remote-drag-ready", serde_json::json!({ "id": drag, "failed": e }));
                    return;
                }
                let until = std::time::Instant::now() + Duration::from_secs(40);
                while std::time::Instant::now() < until {
                    let current = DRAGS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(id, _, _)| *id == drag);
                    if !current {
                        return;
                    }
                    if kagaz_core::remote::rdp::answer(&sent, "ready", &drag).is_some() {
                        let _ = app.emit_to(&window, "remote-drag-ready", serde_json::json!({ "id": drag }));
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            Ok(Some(id))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (app, session, paths);
            Ok(None)
        }
    })
    .await
}

/// How a drag ended there.
#[derive(Serialize, Default)]
pub struct DragEnd {
    /// "dropped" (a real drop there: `effect` says whether anything took
    /// them), "saving" (let go before the drag was held there: the helper
    /// saves them where that was), or "none".
    how: String,
    effect: i32,
}

/// The end of a drag begun with `remote_drag_begin`: `pressed` when the
/// page had pressed the button there (and has now let go, or pressed
/// Escape where the drag left the screen); else, let go at `at`, the
/// files are saved where that was, the way Send files does.
#[tauri::command]
pub async fn remote_drag_end(app: AppHandle, session: u64, id: String, pressed: bool, at: Option<(i32, i32)>) -> Result<DragEnd, String> {
    blocking(move || {
        let drag = {
            let mut drags = DRAGS.lock().unwrap_or_else(|e| e.into_inner());
            let i = drags.iter().position(|(d, _, _)| *d == id);
            i.map(|i| drags.remove(i))
        };
        let Some((id, sent, batch)) = drag else { return Ok(DragEnd { how: "none".into(), effect: 0 }) };
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use kagaz_core::remote::rdp::{answer, ask, cancel, take_back};
            if pressed {
                // Windows' drop is done when the target has the files'
                // names; copying them is its own after that.
                let until = std::time::Instant::now() + Duration::from_secs(15);
                while std::time::Instant::now() < until {
                    if let Some(effect) = answer(&sent, "done", &id) {
                        return Ok(DragEnd { how: "dropped".into(), effect });
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                return Ok(DragEnd { how: "dropped".into(), effect: -1 });
            }
            if !take_back(&sent, &id) {
                cancel(&sent, &id);
            }
            match at {
                Some(at) => {
                    let (input, numbers, window) =
                        app.state::<Viewers>().with(session, |o| (o.input.clone(), o.numbers.clone(), o.window.clone()))?;
                    ask(&sent, "save", &batch, Some(at))?;
                    helper_started(&app, &window, &input, &numbers, &sent)?;
                    Ok(DragEnd { how: "saving".into(), effect: 0 })
                }
                None => Ok(DragEnd { how: "none".into(), effect: 0 }),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (app, session, pressed, at, sent, batch, id);
            Ok(DragEnd { how: "none".into(), effect: 0 })
        }
    })
    .await
}

/// The Kagaz drive shown there, then Windows' Run box opened with `line`
/// pasted into it, for the person to run with Enter there (the page in
/// `window` says so). The line waits for the drive itself.
///
/// Pasted, not typed: the Run box completes what is typed from what it
/// ran before, and in doing so set itself back to the first letters,
/// losing those typed meanwhile ("power" and the line's end, on a real
/// computer). The line goes on its clipboard for the paste; this
/// computer's clipboard is offered there again after.
///
/// Not run by Kagaz: what has the focus there cannot be seen from here,
/// and the Run box can take a second or more to come (the Windows key
/// changes the taskbar at once, which once passed for it), so a key
/// pressed blind could land elsewhere, where Enter opens and Delete
/// deletes. The person sees the Run box and presses Enter. Kagaz share
/// there (`--drops`) needs none of this.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_there(app: &AppHandle, window: &str, input: &kagaz_core::remote::InputSender, numbers: &Numbers, line: &str) -> Result<(), String> {
    use tauri::Emitter;
    input.send(Input::ShowSent);
    input.send(Input::Clipboard(line.to_string()));
    std::thread::sleep(Duration::from_millis(300));
    let key = |key: &str, code: &str, down: bool| Input::Key { key: key.into(), code: code.into(), down };
    // Windows + R: the Run box.
    let before = numbers.read().updates;
    input.send(key("Meta", "MetaLeft", true));
    input.send(key("r", "KeyR", true));
    input.send(key("r", "KeyR", false));
    input.send(key("Meta", "MetaLeft", false));
    // Pasted only once the Run box is likely on its screen: a second and
    // a half at least, the screen changed, then still a moment (or a
    // moving picture there keeps it changing).
    let asked = std::time::Instant::now();
    let (mut last, mut still) = (before, std::time::Instant::now());
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let now = numbers.read().updates;
        if now != last {
            (last, still) = (now, std::time::Instant::now());
        }
        let waited = asked.elapsed();
        if last != before && waited >= Duration::from_millis(1500) && (still.elapsed() >= Duration::from_millis(400) || waited >= Duration::from_millis(4000)) {
            break;
        }
        if waited >= Duration::from_secs(8) {
            return Err("Windows' Run box did not open there; nothing was pasted. Try again in a moment.".into());
        }
    }
    // All of what is in it chosen, so that the paste takes its place (a
    // Run box shown again keeps what was pasted before).
    input.send(key("Control", "ControlLeft", true));
    input.send(key("a", "KeyA", true));
    input.send(key("a", "KeyA", false));
    input.send(key("v", "KeyV", true));
    input.send(key("v", "KeyV", false));
    input.send(key("Control", "ControlLeft", false));
    let _ = app.emit_to(window, "remote-run-box", ());
    // This computer's clipboard there again, in the line's place, once
    // the paste has had it.
    std::thread::sleep(Duration::from_millis(1500));
    let again = match clipboard_now(app) {
        Copied { png: Some(png), text } => Some(Input::ClipboardPicture { png, text }),
        Copied { text: Some(text), .. } => Some(Input::Clipboard(text)),
        _ => None,
    };
    if let Some(again) = again {
        input.send(again);
    }
    Ok(())
}

/// The computer's own graphics chip for drawing and encoding its Remote
/// Desktop screen: Windows' settings for it turned on there (`on`), or
/// what was there before put back. Windows asks there for an
/// administrator; this waits up to two minutes for the answer. Says
/// what to do next.
#[tauri::command]
pub async fn remote_graphics(app: AppHandle, session: u64, on: bool) -> Result<String, String> {
    blocking(move || {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let (device_id, sent, input, numbers, window) = app.state::<Viewers>().with(session, |o| {
                (o.device.clone(), o.sent.clone(), o.input.clone(), o.numbers.clone(), o.window.clone())
            })?;
            let sent = sent.ok_or("this session has no drive to start it from (Remote Desktop to Windows only)")?;
            let helper = sent.join(".kagaz");
            std::fs::create_dir_all(&helper).map_err(|e| e.to_string())?;
            std::fs::write(helper.join("graphics.ps1"), kagaz_core::remote::rdp::GRAPHICS_SCRIPT).map_err(|e| e.to_string())?;
            let result = helper.join("graphics-result.txt");
            let _ = std::fs::remove_file(&result);
            run_there(&app, &window, &input, &numbers, &kagaz_core::remote::rdp::helper_line("graphics.ps1", &[if on { "on" } else { "off" }]))?;
            let until = std::time::Instant::now() + Duration::from_secs(120);
            let code = loop {
                if let Ok(text) = std::fs::read_to_string(&result) {
                    // Written by PowerShell, perhaps as UTF-16: the digits.
                    let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
                    if !digits.is_empty() {
                        break digits.parse::<u32>().unwrap_or(1);
                    }
                }
                if std::time::Instant::now() > until {
                    return Err("No answer from the computer within two minutes: Windows' question there may still be open.".into());
                }
                std::thread::sleep(Duration::from_millis(300));
            };
            match code {
                0 => {
                    let list = Devices::default_path();
                    let _ = Devices::update(&list, |d| {
                        if let Some(dev) = d.devices.iter_mut().find(|x| x.id == device_id) {
                            dev.settings.graphics_chip = on;
                        }
                        Ok(())
                    });
                    Ok(if on {
                        "Turned on there. Sign out of Windows there (or restart it) and connect again: then its graphics chip draws and encodes the screen.".into()
                    } else {
                        "Put back as it was. It takes effect at Windows' next sign-in there.".into()
                    })
                }
                1223 => Err("Windows' administrator question there was not answered yes: nothing changed.".into()),
                _ => Err("Windows' settings could not be changed there.".into()),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (app, session, on);
            Err("this comes to this system later".into())
        }
    })
    .await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn through_drive(
    _app: &AppHandle,
    _window: &str,
    _input: &kagaz_core::remote::InputSender,
    _numbers: &Numbers,
    _sent: Option<&std::path::Path>,
    _paths: &[std::path::PathBuf],
    _what: &str,
    _at: Option<(i32, i32)>,
) -> Result<(), String> {
    Err("sending through a drive comes to this system later".into())
}

/// Over SSH into `folder` there (relative to the account's home), with
/// the system's scp, the account that signed in to the session and its
/// password (handed to scp by a helper only it can read), checking the
/// device's key as `kagaz remote ssh` does.
fn over_ssh(app: &AppHandle, session: u64, device: &kagaz_core::remote::Device, paths: &[std::path::PathBuf], folder: &str) -> Result<(), String> {
    use kagaz_core::remote::Speaks;
    let way = device.way(Speaks::Ssh).ok_or("this device has no SSH way saved: add one on its page (port 22 on Windows' OpenSSH server)")?;
    let (address, user, password) = app.state::<Viewers>().with(session, |o| (o.address, o.user.clone(), o.password.clone()))?;
    let host = match address {
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        v4 => v4.to_string(),
    };
    let target = match user.as_deref().or(device.user.as_deref()) {
        Some(u) => format!("{u}@{host}:{folder}/"),
        None => format!("{host}:{folder}/"),
    };
    let mut cmd = std::process::Command::new("scp");
    cmd.arg("-r")
        .arg("-P")
        .arg(way.port.to_string())
        .arg("-o")
        .arg(format!("HostKeyAlias=kagaz-{}", device.id))
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .args(paths)
        .arg(&target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    // The password, if the session had one, through a helper in a folder
    // of its own that only this account can read, gone afterwards.
    let helper = match &password {
        Some(pw) => {
            let dir = kagaz_core::paths::cache_dir().join(format!("askpass-{}", std::process::id()));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            }
            let script = dir.join("askpass");
            std::fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$KAGAZ_SSH_PASSWORD\"\n").map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700));
            }
            cmd.env("SSH_ASKPASS", &script).env("SSH_ASKPASS_REQUIRE", "force").env("KAGAZ_SSH_PASSWORD", pw);
            Some(dir)
        }
        None => {
            cmd.arg("-o").arg("BatchMode=yes");
            None
        }
    };
    let out = cmd.output();
    if let Some(dir) = helper {
        let _ = std::fs::remove_dir_all(dir);
    }
    let out = out.map_err(|_| "the system's scp could not be started (is OpenSSH's client installed?)".to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let why = if said.contains("Permission denied") {
        "the computer did not take the account's password or key"
    } else if said.contains("Connection refused") || said.contains("timed out") {
        "the computer's SSH server did not answer (Windows: Settings › System › Optional features › OpenSSH Server)"
    } else if said.contains("No such file") {
        "that folder is not there"
    } else {
        "scp could not copy them"
    };
    Err(format!("Not sent over SSH: {why}."))
}

/// The whole of this screen for the window, or back to a window; without
/// `on`, whether it has the whole screen now. The window's own full
/// screen, not the page's: a page's full screen ends with Esc, which in a
/// session belongs to the device.
#[tauri::command]
pub fn remote_fullscreen(window: tauri::WebviewWindow, on: Option<bool>) -> Result<bool, String> {
    #[cfg(desktop)]
    {
        if let Some(on) = on {
            window.set_fullscreen(on).map_err(|e| e.to_string())?;
            return Ok(on);
        }
        window.is_fullscreen().map_err(|e| e.to_string())
    }
    #[cfg(not(desktop))]
    {
        let _ = (window, on);
        Err("a phone's window has the whole screen already".into())
    }
}

/// The keys this computer keeps for itself (Print Screen, the Windows
/// key, Alt + Tab, the system's shortcuts) go to the window instead while
/// `on`, and so to the device; the page says when (see `wantKeys` there).
/// Says whether the window has them now.
///
/// On Wayland the system asks the person, the first time, whether Kagaz
/// may, and keeps a key of its own to take them back (GNOME: Super +
/// Esc). Its question takes the focus from the window, and withdrawing
/// the request would take the question away again: so there, nothing
/// changes while the window is not `focused` (the system gives the keys
/// to whatever has the focus meanwhile anyway). Under X11 the keys would
/// stay with a window that lost the focus, so they are let go with it.
#[tauri::command]
pub async fn remote_keys(app: AppHandle, window: tauri::WebviewWindow, on: bool, focused: bool) -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        // Which window has them: one at a time, the one in front.
        static HELD: Mutex<Option<String>> = Mutex::new(None);
        let (tx, rx) = mpsc::channel();
        let label = window.label().to_string();
        app.run_on_main_thread(move || {
            let Ok(mut holder) = HELD.lock() else { return };
            let was = holder.as_deref() == Some(label.as_str());
            let held = (|| {
                let window = window.gtk_window().ok()?;
                let display = window.display();
                let wayland = display.type_().name().contains("Wayland");
                let want = if wayland && !focused { was } else { on && focused };
                if want == was {
                    return Some(was);
                }
                let seat = display.default_seat()?;
                if !want {
                    seat.ungrab();
                    return Some(false);
                }
                // Another of Kagaz's windows had them: they pass to this one.
                if holder.is_some() {
                    seat.ungrab();
                }
                // The keyboard alone: a grab of it by itself is what tells
                // a Wayland system to leave its shortcuts to the window.
                let status = seat.grab(&window.window()?, gtk::gdk::SeatCapabilities::KEYBOARD, true, None, None, None);
                Some(status == gtk::gdk::GrabStatus::Success)
            })()
            .unwrap_or(false);
            if held {
                *holder = Some(label);
            } else if was {
                *holder = None;
            }
            let _ = tx.send(held);
        })
        .map_err(|e| e.to_string())?;
        Ok(rx.recv_timeout(Duration::from_secs(4)).unwrap_or(false))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, window, on, focused);
        Err("this system's keys stay with it".into())
    }
}

/// Caps Lock, Num Lock and Scroll Lock as they are on this keyboard now.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Locks {
    caps: bool,
    num: bool,
    scroll: bool,
}

/// The lock keys' state, from the system itself. The page cannot be
/// asked: WebKitGTK tells a page about Caps Lock only, and says "off" for
/// the other two whatever they are, so a computer was told that Num Lock
/// was off at the start of every session and its number pad came out the
/// other way round from this keyboard's (the owner, 2026-11-18).
#[tauri::command]
pub async fn remote_locks(app: AppHandle, window: tauri::WebviewWindow) -> Result<Locks, String> {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        let (tx, rx) = mpsc::channel();
        app.run_on_main_thread(move || {
            let locks = window.gtk_window().ok().and_then(|w| gtk::gdk::Keymap::for_display(&w.display())).map(|keys| Locks {
                caps: keys.is_caps_locked(),
                num: keys.is_num_locked(),
                scroll: keys.is_scroll_locked(),
            });
            let _ = tx.send(locks);
        })
        .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(4)).ok().flatten().ok_or_else(|| "the keyboard's state is not known".to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, window);
        Err("the page knows the lock keys here".into())
    }
}

/// A pinch on the touchpad (or the screen) while a device's screen is
/// shown is the device's: the page is told how far the fingers have
/// spread (`remote-pinch`) and passes it on as a zoom there. Left alone,
/// the webview would enlarge Kagaz's own page instead, the screen's area
/// in it would change with every move of the fingers, and a Remote
/// Desktop computer would redraw its screen for each. While `on`, the
/// webview's own pinch is off; with it off again, Kagaz's other pages
/// zoom as they always did. Says whether the webview's pinch was found
/// (it is kept under a name of WebKitGTK's own choosing).
#[tauri::command]
pub async fn remote_pinch(window: tauri::WebviewWindow, on: bool) -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        use gtk::glib::translate::from_glib_none;
        use gtk::prelude::*;
        use tauri::Emitter;
        thread_local! {
            /// For each window: Kagaz's own pinch on its webview, and
            /// the phase the webview's had before it was first turned off.
            static OURS: std::cell::RefCell<HashMap<String, (gtk::GestureZoom, gtk::PropagationPhase)>> = std::cell::RefCell::new(HashMap::new());
        }
        let page = window.clone();
        let (tx, rx) = mpsc::channel();
        window
            .with_webview(move |webview| {
                let view = webview.inner();
                // WebKitGTK keeps its pinch under this name on its widget.
                let theirs: Option<gtk::EventController> = unsafe {
                    let held = gtk::glib::gobject_ffi::g_object_get_data(view.as_ptr().cast(), c"wk-view-zoom-gesture".as_ptr());
                    (!held.is_null()).then(|| from_glib_none(held.cast::<gtk::ffi::GtkEventController>()))
                };
                // Without it there is nothing to turn off, and a pinch
                // passed on as well would zoom twice.
                let _ = tx.send(theirs.is_some());
                let Some(theirs) = theirs else { return };
                OURS.with(|ours| {
                    let mut ours = ours.borrow_mut();
                    let (mine, was) = ours.entry(page.label().to_string()).or_insert_with(|| {
                        let mine = gtk::GestureZoom::new(&view);
                        mine.set_propagation_phase(gtk::PropagationPhase::None);
                        let to = page.clone();
                        mine.connect_begin(move |_, _| {
                            let _ = to.emit_to(to.label(), "remote-pinch", Pinch { scale: 1.0, begin: true, end: false });
                        });
                        let to = page.clone();
                        mine.connect_end(move |_, _| {
                            let _ = to.emit_to(to.label(), "remote-pinch", Pinch { scale: 1.0, begin: false, end: true });
                        });
                        let to = page.clone();
                        mine.connect_scale_changed(move |_, scale| {
                            let _ = to.emit_to(to.label(), "remote-pinch", Pinch { scale, begin: false, end: false });
                        });
                        (mine, theirs.propagation_phase())
                    });
                    theirs.set_propagation_phase(if on { gtk::PropagationPhase::None } else { *was });
                    mine.set_propagation_phase(if on { gtk::PropagationPhase::Target } else { gtk::PropagationPhase::None });
                });
            })
            .map_err(|e| e.to_string())?;
        Ok(rx.recv_timeout(Duration::from_secs(4)).unwrap_or(false))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (window, on);
        Err("a pinch stays with this system here".into())
    }
}

/// How far the fingers of a pinch have spread since it began (1 at its
/// beginning, 2 at twice as far apart), and when the fingers lift.
#[cfg(target_os = "linux")]
#[derive(Clone, Serialize)]
struct Pinch {
    scale: f64,
    begin: bool,
    end: bool,
}

/// A session is open in that window: closing it would end the session,
/// so the person is asked first (remote_windows.rs holds the window back;
/// the page asks).
#[cfg(desktop)]
pub fn open_in(app: &AppHandle, window: &str) -> bool {
    app.try_state::<Viewers>()
        .is_some_and(|viewers| viewers.list.lock().is_ok_and(|list| list.values().any(|open| open.window == window)))
}

/// That window is gone: so are the sessions it showed.
#[cfg(desktop)]
pub fn end_in(app: &AppHandle, window: &str) {
    let Some(viewers) = app.try_state::<Viewers>() else { return };
    let Ok(mut list) = viewers.list.lock() else { return };
    let gone: Vec<u64> = list.iter().filter(|(_, open)| open.window == window).map(|(id, _)| *id).collect();
    for id in gone {
        if let Some(open) = list.remove(&id) {
            open.stop.store(true, Ordering::Release);
            open.input.send(Input::Close);
            open.unpainted.1.notify_all();
        }
    }
}

/// The person said yes: the window closes, and its sessions with it.
#[tauri::command]
pub fn remote_close_window(window: tauri::WebviewWindow) {
    let _ = window.destroy();
}

/// Files the window was started to send (`kagaz remote send` starts it
/// with `KAGAZ_SEND`, a list of paths): handed out once, to the first
/// session that opens.
#[tauri::command]
pub fn remote_startup_files() -> Vec<String> {
    static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if ASKED.swap(true, Ordering::AcqRel) {
        return Vec::new();
    }
    std::env::var("KAGAZ_SEND").ok().and_then(|list| serde_json::from_str(&list).ok()).unwrap_or_default()
}

#[tauri::command]
pub fn remote_disconnect(viewers: tauri::State<'_, Viewers>, session: u64) {
    if let Ok(mut list) = viewers.list.lock() {
        if let Some(open) = list.remove(&session) {
            open.stop.store(true, Ordering::Release);
            open.input.send(Input::Close);
            open.unpainted.1.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pipe: the page with its address gets the WebSocket's answer
    /// and each message whole and in order; anyone without the key gets
    /// nothing, and the page still can after them.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_pipe_opens_for_its_key_alone_and_carries_whole_messages() {
        use std::io::{Read, Write};
        let pipe = super::pipe::open().unwrap();
        let address = pipe.address().unwrap();
        let rest = address.strip_prefix("ws://").unwrap();
        let (host, path) = rest.split_once('/').unwrap();
        let host = host.to_string();
        let path = format!("/{path}");
        let page = std::thread::spawn(move || {
            // A stranger first, with a key of its own making.
            let mut stranger = std::net::TcpStream::connect(&host).unwrap();
            stranger.write_all(b"GET /0123 HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").unwrap();
            let mut got = Vec::new();
            let _ = stranger.read_to_end(&mut got);
            assert!(got.is_empty(), "a stranger was answered");
            let mut page = std::net::TcpStream::connect(&host).unwrap();
            page.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").as_bytes()).unwrap();
            let mut all = Vec::new();
            page.read_to_end(&mut all).unwrap();
            all
        });
        let mut socket = pipe.accept(Duration::from_secs(5)).expect("the page got in");
        let big = vec![7u8; 70_000];
        assert!(socket.send(b"hi"));
        assert!(socket.send(&[1u8; 300]));
        assert!(socket.send(&big));
        drop(socket);
        let all = page.join().unwrap();
        let text = String::from_utf8_lossy(&all);
        // RFC 6455's own example key and answer.
        assert!(text.starts_with("HTTP/1.1 101") && text.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="), "{text}");
        let mut at = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let mut messages = Vec::new();
        while at < all.len() {
            assert_eq!(all[at], 0x82);
            let (len, head) = match all[at + 1] {
                126 => (u16::from_be_bytes([all[at + 2], all[at + 3]]) as usize, 4),
                127 => (u64::from_be_bytes(all[at + 2..at + 10].try_into().unwrap()) as usize, 10),
                n => (n as usize, 2),
            };
            messages.push(all[at + head..at + head + len].to_vec());
            at += head + len;
        }
        assert_eq!(messages, vec![b"hi".to_vec(), vec![1u8; 300], big]);
    }

    #[test]
    fn an_update_packet_holds_its_pieces_in_order() {
        let pieces = [
            Piece::Size { width: 640, height: 480 },
            Piece::Picture {
                rect: Rect::new(1, 2, 2, 1),
                rgba: vec![9, 8, 7, 255, 6, 5, 4, 255],
            },
            Piece::Move {
                from_x: 10,
                from_y: 20,
                rect: Rect::new(30, 40, 50, 60),
            },
        ];
        let p = update_packet(7, &pieces);
        assert_eq!(p[..12], [1, 0, 0, 0, 7, 0, 0, 0, 3, 0, 0, 0]);
        // The size.
        assert_eq!(p[12..28], [1, 0, 0, 0, 0, 0, 0, 0, 0x80, 2, 0xe0, 1, 0, 0, 0, 0]);
        // The picture, then its eight bytes.
        assert_eq!(p[28..44], [2, 0, 0, 0, 1, 0, 2, 0, 2, 0, 1, 0, 0, 0, 0, 0]);
        assert_eq!(p[44..52], [9, 8, 7, 255, 6, 5, 4, 255]);
        // The move.
        assert_eq!(p[52..68], [3, 0, 0, 0, 30, 0, 40, 0, 50, 0, 60, 0, 10, 0, 20, 0]);
        assert_eq!(p.len(), 68);
    }

    #[test]
    fn what_the_page_did_reads_from_its_json() {
        let did: Vec<Did> = serde_json::from_str(
            r#"[{"t":"pointer","x":3,"y":4,"buttons":1},{"t":"key","key":"a","code":"KeyA","down":true},
                {"t":"wheel","x":1,"y":2,"up":-1,"right":0},{"t":"picture","value":null},{"t":"swapped","on":true},
                {"t":"resize","width":1280,"height":700}]"#,
        )
        .unwrap();
        assert_eq!(did.len(), 6);
        let asking: Asking = serde_json::from_str(r#"{"password":"x","view":[1280,700,200]}"#).unwrap();
        assert_eq!(asking.view, Some((1280, 700, 200)));
        assert!(!asking.accept_certificate && asking.user.is_none());
        assert!(matches!(did[3], Did::Picture { value: None }));
    }

    #[test]
    fn a_failure_tells_the_page_its_kind() {
        let json = serde_json::to_value(Failed::WrongPassword { remembered: true }).unwrap();
        assert_eq!(json["kind"], "wrong-password");
        assert_eq!(json["remembered"], true);
        let json = serde_json::to_value(Failed::Unreachable {
            why: "it is off".into(),
            paths: vec![],
        })
        .unwrap();
        assert_eq!(json["kind"], "unreachable");
        assert_eq!(json["why"], "it is off");
    }
}
