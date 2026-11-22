//! The computer's sound on Android, through AAudio, looked up when a
//! session starts (`dlopen`) rather than linked: Android 8.0 and later have
//! it, and a session there plays the sound; on Android 7, which has none,
//! the session goes on without it and Kagaz still installs (the owner's
//! choice, 2026-11-22). cpal links AAudio, which would keep Kagaz off
//! Android 7 altogether, so the phone has this small way of its own.
//!
//! Only what playing needs: a 16-bit output stream at the computer's rate
//! and channels, low latency, filled from the same [`Fill`] as on a
//! computer in AAudio's own callback, and opened again when the output goes
//! (earphones pulled out, Bluetooth gone).

use super::{Fill, Shared};
use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// What the stream is opened with.
#[derive(Debug, Clone, Copy)]
pub(super) struct OutConfig {
    pub channels: u16,
    pub sample_rate: u32,
}

type Builder = c_void;
type Stream = c_void;
type DataCallback = extern "C" fn(*mut Stream, *mut c_void, *mut c_void, i32) -> i32;
type ErrorCallback = extern "C" fn(*mut Stream, *mut c_void, i32);

const DIRECTION_OUTPUT: i32 = 0;
const FORMAT_PCM_I16: i32 = 1;
const PERFORMANCE_LOW_LATENCY: i32 = 12;
const CALLBACK_CONTINUE: i32 = 0;

/// AAudio's calls, from the system's library.
struct Api {
    create_builder: unsafe extern "C" fn(*mut *mut Builder) -> i32,
    set_direction: unsafe extern "C" fn(*mut Builder, i32),
    set_rate: unsafe extern "C" fn(*mut Builder, i32),
    set_channels: unsafe extern "C" fn(*mut Builder, i32),
    set_format: unsafe extern "C" fn(*mut Builder, i32),
    set_performance: unsafe extern "C" fn(*mut Builder, i32),
    set_data_callback: unsafe extern "C" fn(*mut Builder, DataCallback, *mut c_void),
    set_error_callback: unsafe extern "C" fn(*mut Builder, ErrorCallback, *mut c_void),
    open: unsafe extern "C" fn(*mut Builder, *mut *mut Stream) -> i32,
    delete_builder: unsafe extern "C" fn(*mut Builder) -> i32,
    start: unsafe extern "C" fn(*mut Stream) -> i32,
    stop: unsafe extern "C" fn(*mut Stream) -> i32,
    close: unsafe extern "C" fn(*mut Stream) -> i32,
    buffer_frames: unsafe extern "C" fn(*mut Stream) -> i32,
    rate: unsafe extern "C" fn(*mut Stream) -> i32,
}

/// AAudio, where this Android has it (8.0 and later).
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let lib = libc::dlopen(c"libaaudio.so".as_ptr(), libc::RTLD_NOW);
        if lib.is_null() {
            return None;
        }
        let find = |name: &CStr| {
            let f = libc::dlsym(lib, name.as_ptr());
            (!f.is_null()).then_some(f)
        };
        macro_rules! get {
            ($name:literal) => {
                std::mem::transmute(find($name)?)
            };
        }
        Some(Api {
            create_builder: get!(c"AAudio_createStreamBuilder"),
            set_direction: get!(c"AAudioStreamBuilder_setDirection"),
            set_rate: get!(c"AAudioStreamBuilder_setSampleRate"),
            set_channels: get!(c"AAudioStreamBuilder_setChannelCount"),
            set_format: get!(c"AAudioStreamBuilder_setFormat"),
            set_performance: get!(c"AAudioStreamBuilder_setPerformanceMode"),
            set_data_callback: get!(c"AAudioStreamBuilder_setDataCallback"),
            set_error_callback: get!(c"AAudioStreamBuilder_setErrorCallback"),
            open: get!(c"AAudioStreamBuilder_openStream"),
            delete_builder: get!(c"AAudioStreamBuilder_delete"),
            start: get!(c"AAudioStream_requestStart"),
            stop: get!(c"AAudioStream_requestStop"),
            close: get!(c"AAudioStream_close"),
            buffer_frames: get!(c"AAudioStream_getBufferSizeInFrames"),
            rate: get!(c"AAudioStream_getSampleRate"),
        })
    })
    .as_ref()
}

/// What AAudio's callbacks reach, for as long as the stream is open.
struct Turn {
    fill: Arc<Mutex<Fill>>,
    volume: Arc<AtomicU32>,
    shared: Arc<Shared>,
    channels: usize,
    me: thread::Thread,
}

extern "C" fn on_data(_stream: *mut Stream, user: *mut c_void, data: *mut c_void, frames: i32) -> i32 {
    // SAFETY: `user` is the Turn kept alive until after the stream closes;
    // `data` holds `frames` frames of 16-bit samples, `channels` each.
    let turn = unsafe { &*(user as *const Turn) };
    let out = unsafe { std::slice::from_raw_parts_mut(data as *mut i16, frames.max(0) as usize * turn.channels) };
    let Ok(mut fill) = turn.fill.try_lock() else {
        out.fill(0);
        return CALLBACK_CONTINUE;
    };
    fill.turn(out);
    let both = turn.volume.load(Ordering::Relaxed);
    let (left, right) = ((both >> 16) as i32, (both & 0xFFFF) as i32);
    if left != 0xFFFF || right != 0xFFFF {
        for frame in out.chunks_mut(turn.channels.max(1)) {
            for (i, s) in frame.iter_mut().enumerate() {
                *s = (i32::from(*s) * if i == 1 { right } else { left } / 0xFFFF) as i16;
            }
        }
    }
    CALLBACK_CONTINUE
}

extern "C" fn on_error(_stream: *mut Stream, user: *mut c_void, _error: i32) {
    // The output went (disconnected): the sound's thread opens it again.
    // Closing it here is not allowed; the thread does that.
    let turn = unsafe { &*(user as *const Turn) };
    turn.shared.lost.store(true, Ordering::Relaxed);
    turn.me.unpark();
}

/// An open, started stream; closed when dropped.
struct Open {
    api: &'static Api,
    stream: *mut Stream,
    turn: *mut Turn,
}

impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            (self.api.stop)(self.stream);
            (self.api.close)(self.stream);
            drop(Box::from_raw(self.turn));
        }
    }
}

fn open(api: &'static Api, config: OutConfig, turn: Turn) -> Option<Open> {
    let turn = Box::into_raw(Box::new(turn));
    unsafe {
        let mut builder: *mut Builder = std::ptr::null_mut();
        if (api.create_builder)(&mut builder) != 0 || builder.is_null() {
            drop(Box::from_raw(turn));
            return None;
        }
        (api.set_direction)(builder, DIRECTION_OUTPUT);
        (api.set_rate)(builder, config.sample_rate as i32);
        (api.set_channels)(builder, i32::from(config.channels));
        (api.set_format)(builder, FORMAT_PCM_I16);
        (api.set_performance)(builder, PERFORMANCE_LOW_LATENCY);
        (api.set_data_callback)(builder, on_data, turn as *mut c_void);
        (api.set_error_callback)(builder, on_error, turn as *mut c_void);
        let mut stream: *mut Stream = std::ptr::null_mut();
        let opened = (api.open)(builder, &mut stream);
        (api.delete_builder)(builder);
        if opened != 0 || stream.is_null() {
            drop(Box::from_raw(turn));
            return None;
        }
        let open = Open { api, stream, turn };
        if (api.start)(stream) != 0 {
            return None; // dropped: closed and freed
        }
        Some(open)
    }
}

/// Holds the way to the speakers for as long as the sound lasts, and finds
/// it again when it goes. Without AAudio (Android 7) it says it is silent
/// and waits for the end, so the session goes on without sound.
pub(super) fn speakers(config: &OutConfig, fill: Fill, shared: &Arc<Shared>, volume: &Arc<AtomicU32>) {
    let fill = Arc::new(Mutex::new(fill));
    let Some(api) = api() else {
        shared.silent.store(true, Ordering::Relaxed);
        while !shared.ended.load(Ordering::Relaxed) {
            thread::park_timeout(Duration::from_millis(500));
        }
        return;
    };
    while !shared.ended.load(Ordering::Relaxed) {
        shared.lost.store(false, Ordering::Relaxed);
        let turn = Turn {
            fill: fill.clone(),
            volume: volume.clone(),
            shared: shared.clone(),
            channels: usize::from(config.channels),
            me: thread::current(),
        };
        let stream = open(api, *config, turn);
        shared.silent.store(stream.is_none(), Ordering::Relaxed);
        let Some(stream) = stream else {
            thread::park_timeout(Duration::from_secs(1));
            continue;
        };
        // What AAudio holds of its own: its buffer, at its rate.
        let (frames, rate) = unsafe { ((api.buffer_frames)(stream.stream), (api.rate)(stream.stream)) };
        if frames > 0 && rate > 0 {
            shared.device_us.store(frames as u64 * 1_000_000 / rate as u64, Ordering::Relaxed);
        }
        while !shared.ended.load(Ordering::Relaxed) && !shared.lost.load(Ordering::Relaxed) {
            thread::park_timeout(Duration::from_millis(500));
        }
        drop(stream);
    }
}
