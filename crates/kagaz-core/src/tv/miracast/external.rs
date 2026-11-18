//! Pictures encoded outside Kagaz's Rust: on a phone, Android's own
//! encoder (MediaCodec) takes the screen straight from a virtual display
//! and hands each encoded picture over here. The app connects the phone's
//! side once ([`connect`]); a TV session then asks it to start at the size
//! the TV chose, asks for whole pictures when the TV wants one, and stops
//! it at the end.

use super::video::Encoded;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// What the TV settled on, for the phone's encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bits_per_second: u32,
}

struct Hooks {
    start: Box<dyn Fn(Wanted) -> Result<(), String> + Send + Sync>,
    key: Box<dyn Fn() + Send + Sync>,
    stop: Box<dyn Fn() + Send + Sync>,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// Encoded pictures waiting for the session: (Annex B bytes, whole picture).
static QUEUE: Mutex<VecDeque<(Vec<u8>, bool)>> = Mutex::new(VecDeque::new());

/// Pictures the phone's encoder has handed over.
static DELIVERED: AtomicU64 = AtomicU64::new(0);

/// More than this waiting means the session stalled: start again from the
/// next whole picture rather than send old ones late.
const MOST_WAITING: usize = 30;

/// Connects the phone's encoder (once, when the app starts): how to start
/// it, ask it for a whole picture, and stop it.
pub fn connect(
    start: impl Fn(Wanted) -> Result<(), String> + Send + Sync + 'static,
    key: impl Fn() + Send + Sync + 'static,
    stop: impl Fn() + Send + Sync + 'static,
) {
    let _ = HOOKS.set(Hooks { start: Box::new(start), key: Box::new(key), stop: Box::new(stop) });
}

/// Whether an outside encoder is connected.
pub fn connected() -> bool {
    HOOKS.get().is_some()
}

/// An encoded picture from the phone's encoder.
pub fn push(bytes: Vec<u8>, key: bool) {
    DELIVERED.fetch_add(1, Ordering::Relaxed);
    let mut q = QUEUE.lock().unwrap();
    if q.len() >= MOST_WAITING {
        q.clear();
        drop(q);
        if let Some(h) = HOOKS.get() {
            (h.key)();
        }
        return;
    }
    q.push_back((bytes, key));
}

/// The session's side: started at the TV's size, stopped when dropped.
pub struct External {
    _private: (),
}

impl External {
    pub fn open(width: usize, height: usize, fps: u32, bits_per_second: u32) -> Result<External, String> {
        let h = HOOKS.get().ok_or("no encoder outside Kagaz is connected")?;
        QUEUE.lock().unwrap().clear();
        (h.start)(Wanted { width: width as u32, height: height as u32, fps, bits_per_second })?;
        Ok(External { _private: () })
    }

    pub fn key_next(&mut self) {
        if let Some(h) = HOOKS.get() {
            (h.key)();
        }
    }

    /// What arrived since the last call, as one picture's worth (empty when
    /// nothing came; the session then skips this slot).
    pub fn encode(&mut self) -> Encoded {
        let mut q = QUEUE.lock().unwrap();
        let mut out = Encoded { data: Vec::new(), key: false };
        for (bytes, key) in q.drain(..) {
            out.data.extend(bytes);
            out.key |= key;
        }
        out
    }
}

impl Drop for External {
    fn drop(&mut self) {
        if let Some(h) = HOOKS.get() {
            (h.stop)();
        }
        QUEUE.lock().unwrap().clear();
    }
}

/// The phone's screen as the session's picture source: the pictures come
/// encoded through [`External`], so there is nothing to draw here.
pub struct PhoneScreen {
    nothing: Vec<u8>,
}

impl PhoneScreen {
    pub fn new() -> PhoneScreen {
        PhoneScreen { nothing: Vec::new() }
    }
}

impl Default for PhoneScreen {
    fn default() -> Self {
        PhoneScreen::new()
    }
}

impl super::Pictures for PhoneScreen {
    fn picture(&mut self, _due: std::time::Duration, _width: usize, _height: usize) -> &[u8] {
        &self.nothing
    }

    fn delivered(&self) -> u64 {
        DELIVERED.load(Ordering::Relaxed)
    }
}
