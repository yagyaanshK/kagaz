//! The viewer of Computers & phones: sessions open in the window. A
//! session (kagaz-core `remote::session`) keeps the device's screen; a
//! thread here takes what changed and hands it to the page as binary
//! packets over a channel, the way the cameras' pictures travel
//! (media.rs). The page paints them on a canvas and says when it has, so
//! that no more than two pictures are ever on their way to it: a slow
//! page skips pictures instead of falling behind.
//!
//! A packet starts with its kind:
//!   1  a picture update: sequence number (u32), count (u32), then pieces
//!      of 16 bytes each (kind, 3 unused, x, y, width, height, from x,
//!      from y, all u16) followed, for a picture, by its RGBA pixels.
//!      Piece kinds: 1 the screen's size, 2 a picture, 3 a move.
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

/// The page may be this many pictures behind before Kagaz waits for it.
const AHEAD: u32 = 2;

struct Open {
    input: InputSender,
    numbers: Arc<Numbers>,
    /// Pictures sent that the page has not painted yet.
    unpainted: Arc<(Mutex<u32>, Condvar)>,
    stop: Arc<AtomicBool>,
}

/// The sessions open in the window.
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

#[cfg(target_os = "linux")]
fn clipboard_text(app: &AppHandle) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        let _ = tx.send(clipboard.wait_for_text().map(|t| t.to_string()));
    })
    .ok()?;
    rx.recv_timeout(Duration::from_secs(2)).ok().flatten()
}

#[cfg(target_os = "linux")]
fn set_clipboard_text(app: &AppHandle, text: String) -> bool {
    app.run_on_main_thread(move || {
        gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).set_text(&text);
    })
    .is_ok()
}

// The clipboard of the other systems comes with their viewers.
#[cfg(not(target_os = "linux"))]
fn clipboard_text(_app: &AppHandle) -> Option<String> {
    None
}

#[cfg(not(target_os = "linux"))]
fn set_clipboard_text(_app: &AppHandle, _text: String) -> bool {
    false
}

/// Does this build carry the clipboard between here and the device?
const CLIPBOARD_HERE: bool = cfg!(target_os = "linux");

/// Take what changes on the session's screen to the page until either
/// side ends it.
fn forward(app: AppHandle, id: u64, session: remote::Session, channel: Channel<InvokeResponseBody>, open: &Open) {
    let (unpainted, stop) = (open.unpainted.clone(), open.stop.clone());
    std::thread::spawn(move || {
        let mut seq = 0u32;
        let send = |packet: Vec<u8>| channel.send(InvokeResponseBody::Raw(packet)).is_ok();
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
                        let kept = set_clipboard_text(&app, text.clone());
                        let news = serde_json::json!({ "type": "clipboard", "characters": text.chars().count(), "kept": kept });
                        (news_packet(news), false)
                    }
                    Event::Pointer(p) => (pointer_packet(&p), false),
                    Event::PointerDefault => (news_packet(serde_json::json!({ "type": "pointer-default" })), false),
                    Event::Bell => (news_packet(serde_json::json!({ "type": "bell" })), false),
                    Event::Sending(how) => {
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
}

/// Reach a saved device and open a session with it; its picture then
/// arrives on `channel`.
#[tauri::command]
pub async fn remote_connect(
    app: AppHandle,
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
        } = asking;
        let typed = password.filter(|p| !p.is_empty());
        let typed_user = user.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        // Nothing typed: the remembered one, if the person asked for that
        // and the store can be reached.
        let kept = match &typed {
            None if device.remember => vault::get(&device.id).ok().flatten(),
            _ => None,
        };
        let remembered = kept.is_some();
        let sign_in = SignIn {
            user: typed_user.clone().or(device.user.clone()),
            password: typed.clone().or(kept),
        };
        let trust = Trust {
            known: device.identity.certificate.clone(),
            accept_new: accept_certificate,
        };
        let view = view.map(|(width, height, scale)| View { width, height, scale });
        let session = session::open(
            answer.addr,
            answer.way,
            &sign_in,
            &device.settings,
            &trust,
            view,
            Duration::from_secs(10),
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
        if let (Some(password), Some(keep)) = (&typed, remember) {
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
        let open = Open {
            input: session.sender(),
            numbers: session.numbers.clone(),
            unpainted: Arc::new((Mutex::new(0), Condvar::new())),
            stop: Arc::new(AtomicBool::new(false)),
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
        };
        forward(app.clone(), id, session, channel, &open);
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
    })
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

/// Offer what is on this computer's clipboard to the device. Says how
/// many characters went, or none when there is no text to offer. `asked`
/// when the person pressed the button for it: the text is then offered
/// even over files offered since it was last said.
#[tauri::command]
pub async fn remote_clipboard_push(app: AppHandle, session: u64, asked: Option<bool>) -> Result<Option<usize>, String> {
    if !CLIPBOARD_HERE {
        return Err("the clipboard is not carried on this system yet".into());
    }
    blocking(move || {
        let Some(text) = clipboard_text(&app).filter(|t| !t.is_empty()) else {
            return Ok(None);
        };
        let characters = text.chars().count();
        let input = if asked == Some(true) { Input::ClipboardAsked(text) } else { Input::Clipboard(text) };
        app.state::<Viewers>().with(session, |open| open.input.send(input))?;
        Ok(Some(characters))
    })
    .await
}

/// Offer files to the device: they wait on its clipboard until they are
/// pasted there. Without `paths` the person is asked which; says how many
/// were offered, none when they chose nothing. How the files get on
/// arrives with the session's news.
#[tauri::command]
pub async fn remote_send_files(app: AppHandle, session: u64, paths: Option<Vec<String>>) -> Result<usize, String> {
    blocking(move || {
        let paths: Vec<std::path::PathBuf> = match paths {
            Some(paths) => paths.into_iter().map(Into::into).collect(),
            None => {
                use tauri_plugin_dialog::DialogExt;
                let picked = app.dialog().file().set_title("Send to the device").blocking_pick_files();
                picked.unwrap_or_default().into_iter().filter_map(|p| p.into_path().ok()).collect()
            }
        };
        if paths.is_empty() {
            return Ok(0);
        }
        let count = paths.len();
        app.state::<Viewers>().with(session, |open| open.input.send(Input::Files(paths)))?;
        Ok(count)
    })
    .await
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
