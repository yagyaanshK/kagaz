//! A computer's sound on the TV (macOS 13 and later). The screen's
//! capture carries what the computer plays (capture_mac.rs); while the
//! session's Sound on TV is on, that goes to the TV as Opus and the
//! default output is muted so the computer goes quiet; the mute goes back
//! as it was when the sound comes back or the session ends. Silence fills
//! the gaps, so the TV's player never waits for sound that is not coming.
//!
//! Not yet run on a Mac: written to Apple's documentation and type-checked
//! for macOS from Linux. Whether ScreenCaptureKit's sound still comes with
//! the output muted is to be seen on a real Mac.

use super::capture_mac::{HAS_SOUND, SOUND_IN};
use super::miracast::external;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const RATE: usize = 48_000;
/// 20 ms of stereo sound, in samples (both channels).
const PACKET: usize = RATE / 50 * 2;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
    fn AudioObjectSetPropertyData(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const c_void, size: u32, data: *const c_void) -> i32;
}

/// AudioObjectPropertyAddress.
#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn four(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}

/// The default output device (AudioHardware.h).
fn output_device() -> Option<u32> {
    let a = Address { selector: four(b"dOut"), scope: four(b"glob"), element: 0 };
    let (mut id, mut size) = (0u32, 4u32);
    // SAFETY: the system object (1) and a four-byte answer.
    let r = unsafe { AudioObjectGetPropertyData(1, &a, 0, std::ptr::null(), &mut size, &mut id as *mut u32 as *mut c_void) };
    (r == 0 && id != 0).then_some(id)
}

fn mute_address() -> Address {
    Address { selector: four(b"mute"), scope: four(b"outp"), element: 0 }
}

fn muted(device: u32) -> Option<bool> {
    let (mut v, mut size) = (0u32, 4u32);
    // SAFETY: as above.
    let r = unsafe { AudioObjectGetPropertyData(device, &mute_address(), 0, std::ptr::null(), &mut size, &mut v as *mut u32 as *mut c_void) };
    (r == 0).then_some(v != 0)
}

fn set_muted(device: u32, on: bool) {
    let v = on as u32;
    // SAFETY: as above.
    unsafe { AudioObjectSetPropertyData(device, &mute_address(), 0, std::ptr::null(), 4, &v as *const u32 as *const c_void) };
}

/// Whether this session's capture carries the computer's sound.
pub fn available() -> bool {
    HAS_SOUND.load(Ordering::SeqCst)
}

/// The sound's side of a session; dropping it puts everything back.
pub struct Sound {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Sound {
    pub fn start(_tv: &str) -> Sound {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new().name("kagaz-tv-sound".into()).spawn(move || follow(&s)).ok();
        Sound { stop, thread }
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn follow(stop: &AtomicBool) {
    let mut encoder = match opus2::Encoder::new(RATE as u32, opus2::Channels::Stereo, opus2::Application::Audio) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("kagaz: no Opus encoder for the sound: {e}");
            return;
        }
    };
    let _ = encoder.set_bitrate(opus2::Bitrate::Bits(160_000));
    // The output muted while the sound is on the TV: (device, as it was).
    let mut quiet: Option<(u32, bool)> = None;
    let start = Instant::now();
    let mut made: u64 = 0;
    let mut packet = vec![0u8; 4000];
    let mut frame = vec![0i16; PACKET];
    while !stop.load(Ordering::SeqCst) {
        let want = super::switches::sound_to_tv();
        if want && quiet.is_none() {
            if let Some(d) = output_device() {
                let was = muted(d).unwrap_or(false);
                set_muted(d, true);
                quiet = Some((d, was));
            }
        } else if !want {
            if let Some((d, was)) = quiet.take() {
                set_muted(d, was);
            }
            SOUND_IN.lock().unwrap().clear();
        }
        let due = (start.elapsed().as_micros() as u64 * RATE as u64 / 1_000_000) * 2;
        loop {
            let behind = due.saturating_sub(made);
            {
                let mut q = SOUND_IN.lock().unwrap();
                if q.len() >= PACKET {
                    for (f, s) in frame.iter_mut().zip(q.drain(..PACKET)) {
                        *f = s;
                    }
                } else if behind >= PACKET as u64 * 3 {
                    frame.iter_mut().for_each(|s| *s = 0);
                } else {
                    break;
                }
            }
            match encoder.encode(&frame, &mut packet) {
                Ok(n) => external::push_sound(packet[..n].to_vec()),
                Err(e) => eprintln!("kagaz: a sound packet could not be made: {e}"),
            }
            made += PACKET as u64;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some((d, was)) = quiet.take() {
        set_muted(d, was);
    }
}
