//! Pictures encoded outside Kagaz's Rust: on a phone, Android's own
//! encoder (MediaCodec) takes the screen straight from a virtual display
//! and hands each encoded picture over here. The app connects the phone's
//! side once ([`connect`]); a TV session then asks it to start at the size
//! the TV chose, asks for whole pictures when the TV wants one, and stops
//! it at the end.

use super::video::Encoded;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// What the TV settled on, for the phone's encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bits_per_second: u32,
    /// The stream may change shape as the phone turns (Google Cast may;
    /// Miracast fixes it at the start).
    pub reshape: bool,
}

/// Set for the length of a session whose stream may change shape.
static RESHAPE: AtomicBool = AtomicBool::new(false);

/// While it lives, the phone may send its pictures at its own shape.
pub struct MayReshape;

impl MayReshape {
    pub fn hold() -> MayReshape {
        RESHAPE.store(true, Ordering::SeqCst);
        MayReshape
    }
}

impl Drop for MayReshape {
    fn drop(&mut self) {
        RESHAPE.store(false, Ordering::SeqCst);
    }
}

struct Hooks {
    start: Box<dyn Fn(Wanted) -> Result<(), String> + Send + Sync>,
    key: Box<dyn Fn() + Send + Sync>,
    stop: Box<dyn Fn() + Send + Sync>,
    sound: Box<dyn Fn() -> bool + Send + Sync>,
    /// A new rate for the running encoder (where the phone's side took it).
    bits: Box<dyn Fn(u32) -> bool + Send + Sync>,
}

/// The phone's sound, encoded: Opus packets (48 kHz, stereo) in order.
/// They come all the time the encoder runs, silence included, so the TV's
/// player never waits for sound that is not coming.
static SOUND: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());

/// About two seconds of sound; more waiting means nobody is taking it.
const MOST_SOUND_WAITING: usize = 100;

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// Encoded pictures waiting for the session: (Annex B bytes, whole picture).
static QUEUE: Mutex<VecDeque<(Vec<u8>, bool)>> = Mutex::new(VecDeque::new());

/// Pictures the phone's encoder has handed over.
static DELIVERED: AtomicU64 = AtomicU64::new(0);

/// After pictures were dropped: those that build on them are no use, so
/// nothing is kept until a whole picture comes.
static WHOLE_NEXT: AtomicBool = AtomicBool::new(false);

/// More than this waiting means the session stalled: start again from the
/// next whole picture rather than send old ones late.
const MOST_WAITING: usize = 30;

/// Connects the phone's encoder (once, when the app starts): how to start
/// it, ask it for a whole picture, and stop it.
/// `sound` says whether sound will come with the pictures (the phone has
/// an Opus encoder).
pub fn connect(
    start: impl Fn(Wanted) -> Result<(), String> + Send + Sync + 'static,
    key: impl Fn() + Send + Sync + 'static,
    stop: impl Fn() + Send + Sync + 'static,
    sound: impl Fn() -> bool + Send + Sync + 'static,
    bits: impl Fn(u32) -> bool + Send + Sync + 'static,
) {
    let _ = HOOKS.set(Hooks {
        start: Box::new(start),
        key: Box::new(key),
        stop: Box::new(stop),
        sound: Box::new(sound),
        bits: Box::new(bits),
    });
}

/// A new rate for the phone's running encoder.
pub fn set_bit_rate(bits_per_second: u32) -> bool {
    HOOKS.get().is_some_and(|h| (h.bits)(bits_per_second))
}

/// Whether sound comes with the pictures: the phone's encoder sends it, or
/// this computer's session can (its sound comes through the same queue).
pub fn has_sound() -> bool {
    HOOKS.get().is_some_and(|h| (h.sound)()) || super::super::switches::can_sound()
}

/// One Opus packet of the phone's (or the computer's) sound.
pub fn push_sound(packet: Vec<u8>) {
    let mut q = SOUND.lock().unwrap();
    if q.len() >= MOST_SOUND_WAITING {
        q.pop_front();
    }
    q.push_back(packet);
}

/// The next Opus packet, if one waits.
pub fn take_sound() -> Option<Vec<u8>> {
    SOUND.lock().unwrap().pop_front()
}

/// How long an Opus packet plays, in samples at 48 kHz (from its first
/// byte: the mode and frame size, and how many frames it holds).
pub fn opus_samples(packet: &[u8]) -> u32 {
    let Some(&toc) = packet.first() else { return 0 };
    let one = match toc >> 3 {
        c @ 0..=11 => [480, 960, 1920, 2880][(c % 4) as usize],
        c @ 12..=15 => [480, 960][(c % 2) as usize],
        c => [120, 240, 480, 960][(c % 4) as usize],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => packet.get(1).map(|b| (b & 0x3f) as u32).unwrap_or(0),
    };
    one * frames
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
        WHOLE_NEXT.store(true, Ordering::Relaxed);
        if let Some(h) = HOOKS.get() {
            (h.key)();
        }
        return;
    }
    if WHOLE_NEXT.load(Ordering::Relaxed) {
        if !key {
            return;
        }
        WHOLE_NEXT.store(false, Ordering::Relaxed);
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
        SOUND.lock().unwrap().clear();
        WHOLE_NEXT.store(false, Ordering::Relaxed);
        (h.start)(Wanted { width: width as u32, height: height as u32, fps, bits_per_second, reshape: RESHAPE.load(Ordering::SeqCst) })?;
        Ok(External { _private: () })
    }

    pub fn key_next(&mut self) {
        if let Some(h) = HOOKS.get() {
            (h.key)();
        }
    }

    /// The oldest picture waiting, by itself (for a stream that sends each
    /// picture as its own unit).
    pub fn next(&mut self) -> Option<Encoded> {
        QUEUE.lock().unwrap().pop_front().map(|(data, key)| Encoded { data, key })
    }

    /// Drops what waits and asks for a whole picture: after a stall, the
    /// stream starts again from now, not from old pictures.
    pub fn flush(&mut self) {
        QUEUE.lock().unwrap().clear();
        WHOLE_NEXT.store(true, Ordering::Relaxed);
        self.key_next();
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
        SOUND.lock().unwrap().clear();
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
