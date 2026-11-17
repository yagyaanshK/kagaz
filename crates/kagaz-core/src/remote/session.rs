//! A session with a device: its screen arriving, the mouse and keyboard
//! going there. The same for every protocol; what speaks it is in `vnc`
//! (and Remote Desktop next).

use super::screen::Screen;
use super::{Settings, Speaks, Way};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Which mouse buttons are held, as bits.
pub const LEFT: u8 = 1;
pub const MIDDLE: u8 = 2;
pub const RIGHT: u8 = 4;

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
    /// Text was copied here: offer it to the device.
    Clipboard(String),
    /// Sharper (0) to smoother (10); none lets Kagaz follow the link.
    Picture(Option<u8>),
    /// The device has its mouse buttons swapped, or not after all.
    SwappedButtons(bool),
    Close,
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
        }
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
    pub(crate) fn set_picture(&self, picture: u8) {
        self.picture.store(picture as u32, Ordering::Relaxed);
    }
}

/// An open session. Dropping it closes it.
pub struct Session {
    pub info: Info,
    pub screen: Arc<Screen>,
    pub numbers: Arc<Numbers>,
    pub(crate) input: mpsc::Sender<Input>,
}

impl Session {
    /// Hand something on to the device; quietly nothing once it has ended.
    pub fn send(&self, input: Input) {
        let _ = self.input.send(input);
    }

    /// A handle that sends to this session from elsewhere.
    pub fn sender(&self) -> mpsc::Sender<Input> {
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

/// Open a session at an address that answered (see [`super::reach`]).
pub fn open(
    addr: SocketAddr,
    way: Way,
    sign_in: &SignIn,
    settings: &Settings,
    timeout: Duration,
) -> Result<Session, OpenError> {
    match way.speaks {
        Speaks::Vnc => super::vnc::open(addr, sign_in, settings, timeout),
        Speaks::Rdp => Err(OpenError::Unsupported(
            "Remote Desktop sessions are not built yet; VNC is".into(),
        )),
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
    fn a_password_never_shows_in_debug_output() {
        let s = SignIn {
            user: Some("asha".into()),
            password: Some("sesame".into()),
        };
        assert!(!format!("{s:?}").contains("sesame"));
    }
}
