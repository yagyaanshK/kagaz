//! A session with a device: its screen arriving, the mouse and keyboard
//! going there. The same for every protocol; what speaks it is in `vnc`
//! and `rdp`.

use super::screen::Screen;
use super::{Device, Identity, Settings, Speaks, Way};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Which mouse buttons are held, as bits.
pub const LEFT: u8 = 1;
pub const MIDDLE: u8 = 2;
pub const RIGHT: u8 = 4;
/// The mouse's side buttons (back and forward in a browser), where a
/// session carries them.
pub const BACK: u8 = 8;
pub const FORWARD: u8 = 16;

/// What the person does, on its way to the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// The pointer, in the device's own pixels, with the buttons held now.
    Pointer { x: u16, y: u16, buttons: u8 },
    /// The wheel: notches up (down when negative) and to the right (left).
    Wheel { x: u16, y: u16, up: i16, right: i16 },
    /// A key went down or up; `key` and `code` as the page names them
    /// (see [`super::keys`]).
    Key { key: String, code: String, down: bool },
    /// The text on the clipboard here: offered to the device when it is
    /// not the text last seen (so the same text is not offered over and
    /// over, nor over files offered since).
    Clipboard(String),
    /// The person asks for this text to be offered now, whatever was
    /// offered before.
    ClipboardAsked(String),
    /// A picture was copied here (a screenshot, an image), as PNG, with
    /// the text that was copied along with it: offered to the device as
    /// text is. A device whose kind of session carries only text takes
    /// the text.
    ClipboardPicture { png: Vec<u8>, text: Option<String> },
    /// Offer these files to the device: they wait on its clipboard until
    /// they are pasted there. How they get on comes back as
    /// [`super::Event::Sending`].
    Files(Vec<std::path::PathBuf>),
    /// Sharper (0) to smoother (10); none lets Kagaz follow the link.
    Picture(Option<u8>),
    /// The lock keys as they are on this keyboard: a device that can sets
    /// its own to match, so that capitals here are capitals there.
    Locks { caps: bool, num: bool, scroll: bool },
    /// The device has its mouse buttons swapped, or not after all.
    SwappedButtons(bool),
    /// Play the device's sound here, or not, for now (a session that
    /// carries sound; see [`Info::sound`]).
    Sound(bool),
    /// The viewer is this size now, in pixels: a device that can redraws
    /// its screen to fit.
    Resize { width: u16, height: u16 },
    Close,
}

/// Where what the person does is handed in; it can be held anywhere.
#[derive(Clone)]
pub struct InputSender {
    tx: mpsc::Sender<Input>,
    /// Rouses a session that waits on its connection, not on this.
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl InputSender {
    pub(crate) fn new(tx: mpsc::Sender<Input>, wake: Option<Arc<dyn Fn() + Send + Sync>>) -> InputSender {
        InputSender { tx, wake }
    }

    /// Hand something on to the device; quietly nothing once it has ended.
    pub fn send(&self, input: Input) {
        let _ = self.tx.send(input);
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

/// The viewer the screen will be shown in: its size in pixels and how
/// many of them make one point of its screen, in percent (200 on a
/// screen twice as fine). A device that can draws its screen to fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub width: u16,
    pub height: u16,
    pub scale: u32,
}

/// What is known of the device's certificate before a session opens.
#[derive(Debug, Clone, Default)]
pub struct Trust {
    /// The fingerprint of the certificate it showed before, if any.
    pub known: Option<String>,
    /// The person has seen that it changed and wants the new one.
    pub accept_new: bool,
}

/// With the device's buttons swapped, send the other one, so that a left
/// click here stays a left click there.
pub fn buttons_for(buttons: u8, swapped: bool) -> u8 {
    if !swapped {
        return buttons;
    }
    let mut out = buttons & !(LEFT | RIGHT);
    if buttons & LEFT != 0 {
        out |= RIGHT;
    }
    if buttons & RIGHT != 0 {
        out |= LEFT;
    }
    out
}

/// Who signs in.
#[derive(Clone, Default)]
pub struct SignIn {
    pub user: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for SignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the password, not even in a log line.
        f.debug_struct("SignIn")
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "…"))
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The device asks for a password and none was given.
    #[error("it asks for a password")]
    NeedsPassword,
    /// It wants to know who signs in.
    #[error("it asks for a user name and a password")]
    NeedsUser,
    /// It shows another certificate than the one remembered: renewed, or
    /// another device in its place.
    #[error("it shows another certificate than before")]
    CertificateChanged,
    #[error("it did not accept the password")]
    WrongPassword,
    /// It turned the connection down, in its own words.
    #[error("it refused: {0}")]
    Refused(String),
    /// It wants something Kagaz does not have yet.
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for OpenError {
    fn from(e: std::io::Error) -> OpenError {
        use std::io::ErrorKind::*;
        OpenError::Other(match e.kind() {
            TimedOut | WouldBlock => "it stopped answering".into(),
            UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe => "it closed the connection".into(),
            ConnectionRefused => "nothing answers there".into(),
            _ => e.to_string(),
        })
    }
}

/// What is known once a session is open.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Info {
    pub speaks: Speaks,
    /// The device's own name for its screen, when it gives one.
    pub name: String,
    /// The session protects itself (apart from the path it travels).
    pub encrypted: bool,
    /// Whether these travel in this session.
    pub sound: bool,
    pub files: bool,
    pub monitors: bool,
    /// The device redraws at the viewer's size when asked to.
    pub resizes: bool,
    /// Sharper or smoother can be chosen here (else the device decides).
    pub picture_setting: bool,
    /// Who the device's certificate says it is, where the session has one:
    /// to remember, and to compare the next time.
    #[serde(skip)]
    pub identity: Option<Identity>,
}

/// The session in numbers, kept up to date as it runs.
#[derive(Debug)]
pub struct Numbers {
    started: Instant,
    /// Milliseconds there and back; `u32::MAX` until measured.
    delay: AtomicU32,
    received: AtomicU64,
    sent: AtomicU64,
    updates: AtomicU64,
    /// The picture setting in use: 0 sharpest, 10 smoothest.
    picture: AtomicU32,
    /// Microseconds spent on what the device sent (its picture, mostly).
    work: AtomicU64,
    /// Bytes of the device's sound that arrived.
    sound: AtomicU64,
    /// How the picture travels, in a few words.
    via: std::sync::Mutex<String>,
    /// And the device's sound.
    sound_via: std::sync::Mutex<String>,
}

impl Default for Numbers {
    fn default() -> Numbers {
        Numbers {
            started: Instant::now(),
            delay: AtomicU32::new(u32::MAX),
            received: AtomicU64::new(0),
            sent: AtomicU64::new(0),
            updates: AtomicU64::new(0),
            picture: AtomicU32::new(0),
            work: AtomicU64::new(0),
            sound: AtomicU64::new(0),
            via: std::sync::Mutex::new(String::new()),
            sound_via: std::sync::Mutex::new(String::new()),
        }
    }
}

/// The numbers at one moment.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Reading {
    pub seconds: f64,
    pub delay: Option<u32>,
    pub received: u64,
    pub sent: u64,
    /// Times the picture was brought up to date.
    pub updates: u64,
    pub picture: u8,
    /// Milliseconds this device spent on what arrived, since the start.
    pub work: u64,
    /// Bytes of the device's sound that arrived, as it travelled.
    pub sound: u64,
}

impl Numbers {
    pub fn read(&self) -> Reading {
        let delay = self.delay.load(Ordering::Relaxed);
        Reading {
            seconds: self.started.elapsed().as_secs_f64(),
            delay: (delay != u32::MAX).then_some(delay),
            received: self.received.load(Ordering::Relaxed),
            sent: self.sent.load(Ordering::Relaxed),
            updates: self.updates.load(Ordering::Relaxed),
            picture: self.picture.load(Ordering::Relaxed) as u8,
            work: self.work.load(Ordering::Relaxed) / 1000,
            sound: self.sound.load(Ordering::Relaxed),
        }
    }

    /// How the picture travels ("H.264 12, text 40"), where the session
    /// knows; empty where it does not.
    pub fn via(&self) -> String {
        self.via.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn set_delay(&self, millis: u32) {
        self.delay.store(millis, Ordering::Relaxed);
    }
    pub(crate) fn add_received(&self, bytes: usize) {
        self.received.fetch_add(bytes as u64, Ordering::Relaxed);
    }
    pub(crate) fn add_sent(&self, bytes: usize) {
        self.sent.fetch_add(bytes as u64, Ordering::Relaxed);
    }
    pub(crate) fn add_update(&self) {
        self.updates.fetch_add(1, Ordering::Relaxed);
    }
    /// How the device's sound travels (which channel, which format, how
    /// many blocks), where the session knows; empty where it does not.
    pub fn sound_via(&self) -> String {
        self.sound_via.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn set_sound_via(&self, via: String) {
        *self.sound_via.lock().unwrap_or_else(|e| e.into_inner()) = via;
    }
    pub(crate) fn add_work(&self, spent: Duration) {
        self.work.fetch_add(spent.as_micros() as u64, Ordering::Relaxed);
    }
    pub(crate) fn add_sound(&self, bytes: usize) {
        self.sound.fetch_add(bytes as u64, Ordering::Relaxed);
    }
    pub(crate) fn set_via(&self, via: String) {
        *self.via.lock().unwrap_or_else(|e| e.into_inner()) = via;
    }
    pub(crate) fn set_picture(&self, picture: u8) {
        self.picture.store(picture as u32, Ordering::Relaxed);
    }
}

/// An open session. Dropping it closes it.
pub struct Session {
    pub info: Info,
    pub screen: Arc<Screen>,
    pub numbers: Arc<Numbers>,
    pub(crate) input: InputSender,
}

impl Session {
    /// Hand something on to the device; quietly nothing once it has ended.
    pub fn send(&self, input: Input) {
        self.input.send(input);
    }

    /// A handle that sends to this session from elsewhere.
    pub fn sender(&self) -> InputSender {
        self.input.clone()
    }

    pub fn close(&self) {
        self.send(Input::Close);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// The way into a device that a session opens through: Remote Desktop
/// where the device speaks it (it carries more and protects itself), else
/// VNC. A phone or tablet has no Remote Desktop sessions yet, so there it
/// is VNC first.
pub fn way_for(device: &Device) -> Option<Way> {
    let order = if RDP_HERE { [Speaks::Rdp, Speaks::Vnc] } else { [Speaks::Vnc, Speaks::Rdp] };
    order.iter().find_map(|s| device.way(*s))
}

/// Does this build open Remote Desktop sessions?
pub const RDP_HERE: bool = cfg!(not(any(target_os = "android", target_os = "ios")));

/// Open a session at an address that answered (see [`super::reach`]).
pub fn open(
    addr: SocketAddr,
    way: Way,
    sign_in: &SignIn,
    settings: &Settings,
    trust: &Trust,
    view: Option<View>,
    timeout: Duration,
) -> Result<Session, OpenError> {
    match way.speaks {
        Speaks::Vnc => super::vnc::open(addr, sign_in, settings, timeout),
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        Speaks::Rdp => super::rdp::open(addr, sign_in, settings, trust, view, timeout),
        #[cfg(any(target_os = "android", target_os = "ios"))]
        Speaks::Rdp => {
            let _ = (trust, view);
            Err(OpenError::Unsupported(
                "Remote Desktop sessions are not on phones yet; a device that speaks VNC can be connected to".into(),
            ))
        }
        Speaks::Ssh => Err(OpenError::Unsupported("SSH shows no screen".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_stays_left() {
        assert_eq!(buttons_for(LEFT, false), LEFT);
        assert_eq!(buttons_for(LEFT, true), RIGHT);
        assert_eq!(buttons_for(RIGHT, true), LEFT);
        assert_eq!(buttons_for(LEFT | RIGHT | MIDDLE, true), LEFT | RIGHT | MIDDLE);
        assert_eq!(buttons_for(MIDDLE, true), MIDDLE);
    }

    #[test]
    fn a_session_goes_through_remote_desktop_where_the_device_speaks_it() {
        let mut d = Device {
            ways: vec![Way::new(Speaks::Ssh), Way { speaks: Speaks::Vnc, port: 5901 }],
            ..Device::default()
        };
        assert_eq!(way_for(&d), Some(Way { speaks: Speaks::Vnc, port: 5901 }));
        d.ways.push(Way::new(Speaks::Rdp));
        let first = if RDP_HERE { Speaks::Rdp } else { Speaks::Vnc };
        assert_eq!(way_for(&d).map(|w| w.speaks), Some(first));
        d.ways.truncate(1);
        assert_eq!(way_for(&d), None);
    }

    #[test]
    fn a_password_never_shows_in_debug_output() {
        let s = SignIn {
            user: Some("asha".into()),
            password: Some("sesame".into()),
        };
        assert!(!format!("{s:?}").contains("sesame"));
    }
}
