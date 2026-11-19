//! A computer's session switches: whether the TV gets the screen or
//! frosted glass in its place, and whether the sound plays on the TV
//! (the computer quiet meanwhile) or on the computer. One state, which
//! the TV's card, the strip on top, the shortcuts and the tray menu all
//! turn and all hear about. A phone keeps its own (KagazTv.kt).
//!
//! A session starts hidden with the sound on the computer: nothing
//! reaches the TV until the owner chooses Show.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static LIVE: AtomicBool = AtomicBool::new(false);
static HIDDEN: AtomicBool = AtomicBool::new(false);
static SOUND_ON_TV: AtomicBool = AtomicBool::new(false);
/// This computer can send its sound (it has a way to take it).
static CAN_SOUND: AtomicBool = AtomicBool::new(false);
/// The TV took the sound's stream too.
static SOUND_AGREED: AtomicBool = AtomicBool::new(false);

type Watcher = Box<dyn Fn(State) + Send + Sync>;
static WATCHERS: Mutex<Vec<Watcher>> = Mutex::new(Vec::new());

/// How the switches stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// A session is on.
    pub live: bool,
    /// The TV shows frosted glass in place of the screen.
    pub screen_hidden: bool,
    /// The sound plays on the TV (and not on the computer).
    pub sound_on_tv: bool,
    /// The sound can go to the TV in this session at all.
    pub sound: bool,
}

pub fn state() -> State {
    State {
        live: LIVE.load(Ordering::SeqCst),
        screen_hidden: HIDDEN.load(Ordering::SeqCst),
        sound_on_tv: SOUND_ON_TV.load(Ordering::SeqCst),
        sound: SOUND_AGREED.load(Ordering::SeqCst),
    }
}

/// `f` hears every change (on the thread that made it).
pub fn watch(f: impl Fn(State) + Send + Sync + 'static) {
    WATCHERS.lock().unwrap().push(Box::new(f));
}

fn changed() {
    let s = state();
    for w in WATCHERS.lock().unwrap().iter() {
        w(s);
    }
}

/// A session starts: hidden, the sound on the computer. `can_sound`:
/// this computer has a way to send its sound.
pub fn begin(can_sound: bool) {
    CAN_SOUND.store(can_sound, Ordering::SeqCst);
    SOUND_AGREED.store(false, Ordering::SeqCst);
    HIDDEN.store(true, Ordering::SeqCst);
    SOUND_ON_TV.store(false, Ordering::SeqCst);
    LIVE.store(true, Ordering::SeqCst);
    changed();
}

/// The session ended: everything back on the computer.
pub fn end() {
    LIVE.store(false, Ordering::SeqCst);
    HIDDEN.store(false, Ordering::SeqCst);
    SOUND_ON_TV.store(false, Ordering::SeqCst);
    SOUND_AGREED.store(false, Ordering::SeqCst);
    CAN_SOUND.store(false, Ordering::SeqCst);
    changed();
}

/// Whether a session of this computer's offers the TV its sound.
pub fn can_sound() -> bool {
    CAN_SOUND.load(Ordering::SeqCst)
}

/// The TV took (or refused) the sound's stream.
pub fn sound_agreed(yes: bool) {
    if LIVE.load(Ordering::SeqCst) && SOUND_AGREED.swap(yes, Ordering::SeqCst) != yes {
        changed();
    }
}

pub fn screen_hidden() -> bool {
    LIVE.load(Ordering::SeqCst) && HIDDEN.load(Ordering::SeqCst)
}

/// Whether the computer's sound should go to the TV now.
pub fn sound_to_tv() -> bool {
    LIVE.load(Ordering::SeqCst) && SOUND_AGREED.load(Ordering::SeqCst) && SOUND_ON_TV.load(Ordering::SeqCst)
}

/// "screen" (on: shown), "sound" (on: on the TV), "all" (on: both).
pub fn turn(what: &str, on: bool) -> Result<State, String> {
    if !LIVE.load(Ordering::SeqCst) {
        return Err("nothing is on a TV now".into());
    }
    match what {
        "screen" => HIDDEN.store(!on, Ordering::SeqCst),
        "sound" => SOUND_ON_TV.store(on, Ordering::SeqCst),
        "all" => {
            HIDDEN.store(!on, Ordering::SeqCst);
            SOUND_ON_TV.store(on, Ordering::SeqCst);
        }
        other => return Err(format!("no switch called {other}")),
    }
    changed();
    Ok(state())
}

/// "Hide everything" when anything shows, else "Show everything".
pub fn toggle_all() -> Result<State, String> {
    let s = state();
    let anything_shown = !s.screen_hidden || (s.sound && s.sound_on_tv);
    turn("all", !anything_shown)
}

/// A computer's pictures with the switches applied: frosted glass while
/// the screen is hidden (pictures in graphics memory then go through the
/// processor, where the frosting is made).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub struct Guarded<'a> {
    inner: &'a mut dyn super::miracast::Pictures,
    frost: super::picture::Frost,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl<'a> Guarded<'a> {
    pub fn new(inner: &'a mut dyn super::miracast::Pictures) -> Self {
        Guarded { inner, frost: super::picture::Frost::default() }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl super::miracast::Pictures for Guarded<'_> {
    fn picture(&mut self, due: std::time::Duration, width: usize, height: usize) -> &[u8] {
        if !screen_hidden() {
            return self.inner.picture(due, width, height);
        }
        let p = self.inner.picture(due, width, height);
        self.frost.apply(p, width, height)
    }

    fn delivered(&self) -> u64 {
        self.inner.delivered()
    }

    #[cfg(target_os = "linux")]
    fn gpu_picture(&mut self) -> Option<Option<std::sync::Arc<super::capture::GpuPicture>>> {
        if screen_hidden() {
            None
        } else {
            self.inner.gpu_picture()
        }
    }
}
