//! PipeWire, opened when screen capture starts rather than linked in, so
//! Kagaz starts on a computer without it. Only what capture needs: a
//! thread loop, a connection, one video stream, and the format offer and
//! answer, written and read by hand. Struct layouts and numbers follow
//! PipeWire's own headers (MIT).

use std::ffi::{c_char, c_int, c_void};
use std::os::fd::{IntoRawFd, OwnedFd};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::null_mut;
use std::sync::OnceLock;

type Ptr = *mut c_void;

/// The functions Kagaz takes from libpipewire.
struct Lib {
    thread_loop_new: unsafe extern "C" fn(*const c_char, *const c_void) -> Ptr,
    thread_loop_destroy: unsafe extern "C" fn(Ptr),
    thread_loop_get_loop: unsafe extern "C" fn(Ptr) -> Ptr,
    thread_loop_start: unsafe extern "C" fn(Ptr) -> c_int,
    thread_loop_stop: unsafe extern "C" fn(Ptr),
    thread_loop_lock: unsafe extern "C" fn(Ptr),
    thread_loop_unlock: unsafe extern "C" fn(Ptr),
    context_new: unsafe extern "C" fn(Ptr, Ptr, usize) -> Ptr,
    context_destroy: unsafe extern "C" fn(Ptr),
    context_connect: unsafe extern "C" fn(Ptr, Ptr, usize) -> Ptr,
    context_connect_fd: unsafe extern "C" fn(Ptr, c_int, Ptr, usize) -> Ptr,
    core_disconnect: unsafe extern "C" fn(Ptr) -> c_int,
    properties_new_string: unsafe extern "C" fn(*const c_char) -> Ptr,
    stream_new: unsafe extern "C" fn(Ptr, *const c_char, Ptr) -> Ptr,
    stream_add_listener: unsafe extern "C" fn(Ptr, *mut Hook, *const StreamEvents, Ptr),
    stream_connect: unsafe extern "C" fn(Ptr, u32, u32, u32, *mut *const c_void, u32) -> c_int,
    stream_dequeue_buffer: unsafe extern "C" fn(Ptr) -> *mut PwBuffer,
    stream_queue_buffer: unsafe extern "C" fn(Ptr, *mut PwBuffer) -> c_int,
    stream_destroy: unsafe extern "C" fn(Ptr),
}

static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();

/// PipeWire's library, opened the first time it is wanted and kept open.
fn lib() -> Result<&'static Lib, String> {
    LIB.get_or_init(load).as_ref().map_err(Clone::clone)
}

// Each symbol takes the type of the field it fills.
#[allow(clippy::missing_transmute_annotations)]
fn load() -> Result<Lib, String> {
    // SAFETY: plain dlopen/dlsym with nul-ended names. Each symbol is cast
    // to the signature its header gives; the library is never closed.
    unsafe {
        let h = libc::dlopen(c"libpipewire-0.3.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if h.is_null() {
            return Err("screen capture needs PipeWire, which this computer does not have".into());
        }
        macro_rules! f {
            ($name:literal) => {{
                let p = libc::dlsym(h, concat!($name, "\0").as_ptr().cast());
                if p.is_null() {
                    return Err(format!("this computer's PipeWire is too old for screen capture (it has no {})", $name));
                }
                std::mem::transmute::<*mut c_void, _>(p)
            }};
        }
        let init: unsafe extern "C" fn(*mut c_int, *mut c_void) = f!("pw_init");
        let lib = Lib {
            thread_loop_new: f!("pw_thread_loop_new"),
            thread_loop_destroy: f!("pw_thread_loop_destroy"),
            thread_loop_get_loop: f!("pw_thread_loop_get_loop"),
            thread_loop_start: f!("pw_thread_loop_start"),
            thread_loop_stop: f!("pw_thread_loop_stop"),
            thread_loop_lock: f!("pw_thread_loop_lock"),
            thread_loop_unlock: f!("pw_thread_loop_unlock"),
            context_new: f!("pw_context_new"),
            context_destroy: f!("pw_context_destroy"),
            context_connect: f!("pw_context_connect"),
            context_connect_fd: f!("pw_context_connect_fd"),
            core_disconnect: f!("pw_core_disconnect"),
            properties_new_string: f!("pw_properties_new_string"),
            stream_new: f!("pw_stream_new"),
            stream_add_listener: f!("pw_stream_add_listener"),
            stream_connect: f!("pw_stream_connect"),
            stream_dequeue_buffer: f!("pw_stream_dequeue_buffer"),
            stream_queue_buffer: f!("pw_stream_queue_buffer"),
            stream_destroy: f!("pw_stream_destroy"),
        };
        init(null_mut(), null_mut());
        Ok(lib)
    }
}

/// `struct spa_hook`: list links, callbacks, removed, priv. PipeWire fills
/// it; it starts zeroed and must not move while the listener lives.
#[repr(C)]
struct Hook([Ptr; 6]);

/// `struct pw_stream_events`, version 2.
#[repr(C)]
struct StreamEvents {
    version: u32,
    destroy: Option<unsafe extern "C" fn(Ptr)>,
    state_changed: Option<unsafe extern "C" fn(Ptr, c_int, c_int, *const c_char)>,
    control_info: Option<unsafe extern "C" fn(Ptr, u32, *const c_void)>,
    io_changed: Option<unsafe extern "C" fn(Ptr, u32, Ptr, u32)>,
    param_changed: Option<unsafe extern "C" fn(Ptr, u32, *const c_void)>,
    add_buffer: Option<unsafe extern "C" fn(Ptr, *mut PwBuffer)>,
    remove_buffer: Option<unsafe extern "C" fn(Ptr, *mut PwBuffer)>,
    process: Option<unsafe extern "C" fn(Ptr)>,
    drained: Option<unsafe extern "C" fn(Ptr)>,
    command: Option<unsafe extern "C" fn(Ptr, *const c_void)>,
    trigger_done: Option<unsafe extern "C" fn(Ptr)>,
}

static EVENTS: StreamEvents = StreamEvents {
    version: 2,
    destroy: None,
    state_changed: None,
    control_info: None,
    io_changed: None,
    param_changed: Some(on_param_changed),
    add_buffer: None,
    remove_buffer: None,
    process: Some(on_process),
    drained: None,
    command: None,
    trigger_done: None,
};

/// The start of `struct pw_buffer` (only read, never made here).
#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
}

#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: Ptr,
    datas: *mut SpaData,
}

#[repr(C)]
struct SpaData {
    kind: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: Ptr,
    chunk: *mut SpaChunk,
}

#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

const DIRECTION_INPUT: u32 = 0;
const FLAG_AUTOCONNECT: u32 = 1 << 0;
const FLAG_MAP_BUFFERS: u32 = 1 << 2;

// Pod types (spa/utils/type.h).
const TYPE_ID: u32 = 3;
const TYPE_RECTANGLE: u32 = 10;
const TYPE_FRACTION: u32 = 11;
const TYPE_OBJECT: u32 = 15;
const TYPE_CHOICE: u32 = 19;
const OBJECT_FORMAT: u32 = 0x40003;
// Param ids (spa/param/param.h).
const PARAM_ENUM_FORMAT: u32 = 3;
const PARAM_FORMAT: u32 = 4;
// Format keys (spa/param/format.h) and their values.
const FORMAT_MEDIA_TYPE: u32 = 1;
const FORMAT_MEDIA_SUBTYPE: u32 = 2;
const FORMAT_VIDEO_FORMAT: u32 = 0x20001;
const FORMAT_VIDEO_SIZE: u32 = 0x20003;
const FORMAT_VIDEO_FRAMERATE: u32 = 0x20004;
const MEDIA_TYPE_VIDEO: u32 = 2;
const MEDIA_SUBTYPE_RAW: u32 = 1;
// Choices (spa/pod/pod.h).
const CHOICE_RANGE: u32 = 1;
const CHOICE_ENUM: u32 = 3;

// Video formats (spa/param/video/raw.h).
pub const RGBX: u32 = 7;
pub const BGRX: u32 = 8;
pub const RGBA: u32 = 11;
pub const BGRA: u32 = 12;

/// A pod: body size in bytes, type, body, padded to eight bytes.
fn pod(kind: u32, body: &[u32]) -> Vec<u32> {
    let mut p = vec![(body.len() * 4) as u32, kind];
    p.extend_from_slice(body);
    if p.len() % 2 == 1 {
        p.push(0);
    }
    p
}

/// A choice pod: how to choose, then values of one type, `each` words long,
/// packed one after another (the first is the default).
fn choice(how: u32, kind: u32, each: usize, values: &[u32]) -> Vec<u32> {
    let mut body = vec![how, 0, (each * 4) as u32, kind];
    body.extend_from_slice(values);
    pod(TYPE_CHOICE, &body)
}

/// What Kagaz asks for: raw video in a four-byte RGB layout; any size up
/// to 8192x8192 (1920x1080 preferred) or exactly `size`; up to 1000
/// pictures a second (60 preferred). Words in this machine's byte order.
pub fn video_offer(size: Option<(u32, u32)>) -> Vec<u32> {
    let mut body = vec![OBJECT_FORMAT, PARAM_ENUM_FORMAT];
    let mut prop = |key: u32, value: Vec<u32>| {
        body.extend_from_slice(&[key, 0]);
        body.extend(value);
    };
    prop(FORMAT_MEDIA_TYPE, pod(TYPE_ID, &[MEDIA_TYPE_VIDEO]));
    prop(FORMAT_MEDIA_SUBTYPE, pod(TYPE_ID, &[MEDIA_SUBTYPE_RAW]));
    prop(FORMAT_VIDEO_FORMAT, choice(CHOICE_ENUM, TYPE_ID, 1, &[BGRX, BGRX, RGBX, BGRA, RGBA]));
    prop(
        FORMAT_VIDEO_SIZE,
        match size {
            Some((w, h)) => pod(TYPE_RECTANGLE, &[w, h]),
            None => choice(CHOICE_RANGE, TYPE_RECTANGLE, 2, &[1920, 1080, 1, 1, 8192, 8192]),
        },
    );
    prop(FORMAT_VIDEO_FRAMERATE, choice(CHOICE_RANGE, TYPE_FRACTION, 2, &[60, 1, 0, 1, 1000, 1]));
    pod(TYPE_OBJECT, &body)
}

/// The format both sides settled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFormat {
    /// One of [`RGBX`], [`BGRX`], [`RGBA`], [`BGRA`], or another layout.
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

/// Reads the video format and size out of a Format pod. A value may come
/// as a choice; then its first (preferred) value counts.
pub fn video_format(pod: &[u8]) -> Option<VideoFormat> {
    let word = |b: &[u8], at: usize| b.get(at..at + 4).map(|w| u32::from_ne_bytes(w.try_into().unwrap()));
    let size = word(pod, 0)? as usize;
    if word(pod, 4)? != TYPE_OBJECT || word(pod, 8)? != OBJECT_FORMAT {
        return None;
    }
    let pod = pod.get(..8 + size)?;
    let (mut format, mut dims) = (None, None);
    // Properties: key, flags, then the value pod.
    let mut at = 16;
    while at + 16 <= pod.len() {
        let key = word(pod, at)?;
        let len = word(pod, at + 8)? as usize;
        let mut kind = word(pod, at + 12)?;
        let mut value = pod.get(at + 16..at + 16 + len)?;
        if kind == TYPE_CHOICE {
            // How, flags, then the values' own size and type.
            let each = word(value, 8)? as usize;
            kind = word(value, 12)?;
            value = value.get(16..16 + each)?;
        }
        match (key, kind) {
            (FORMAT_VIDEO_FORMAT, TYPE_ID) => format = word(value, 0),
            (FORMAT_VIDEO_SIZE, TYPE_RECTANGLE) => dims = word(value, 0).zip(word(value, 4)),
            _ => {}
        }
        at += 16 + len.div_ceil(8) * 8;
    }
    let (width, height) = dims?;
    Some(VideoFormat { format: format?, width, height })
}

/// A picture as it lies in PipeWire's buffer: `bytes` is all the mapped
/// memory; the picture starts at `offset`.
pub struct Frame<'a> {
    pub bytes: &'a [u8],
    pub offset: usize,
    pub size: usize,
    pub stride: i32,
}

/// What the stream's events reach; lives in a box the stream points at.
struct Callbacks {
    lib: &'static Lib,
    stream: Ptr,
    format: Box<dyn FnMut(VideoFormat) + Send>,
    frame: Box<dyn FnMut(Frame<'_>) + Send>,
}

unsafe extern "C" fn on_param_changed(data: Ptr, id: u32, param: *const c_void) {
    if param.is_null() || id != PARAM_FORMAT {
        return;
    }
    // SAFETY: `data` is the Callbacks box given with the listener; `param`
    // is a whole pod whose first word is its body size.
    let (cb, pod) = unsafe {
        let size = *(param as *const u32) as usize;
        (&mut *(data as *mut Callbacks), std::slice::from_raw_parts(param as *const u8, 8 + size))
    };
    if let Some(f) = video_format(pod) {
        let _ = catch_unwind(AssertUnwindSafe(|| (cb.format)(f)));
    }
}

unsafe extern "C" fn on_process(data: Ptr) {
    // SAFETY: as above; the buffer's pointers come from PipeWire and stay
    // good until it is queued back, which is the last thing done here.
    unsafe {
        let cb = &mut *(data as *mut Callbacks);
        let b = (cb.lib.stream_dequeue_buffer)(cb.stream);
        if b.is_null() {
            return;
        }
        let sb = (*b).buffer;
        if !sb.is_null() && (*sb).n_datas > 0 && !(*sb).datas.is_null() {
            let d = &*(*sb).datas;
            if !d.data.is_null() && !d.chunk.is_null() {
                let c = &*d.chunk;
                let frame = Frame {
                    bytes: std::slice::from_raw_parts(d.data as *const u8, d.maxsize as usize),
                    offset: c.offset as usize,
                    size: c.size as usize,
                    stride: c.stride,
                };
                let _ = catch_unwind(AssertUnwindSafe(|| (cb.frame)(frame)));
            }
        }
        (cb.lib.stream_queue_buffer)(cb.stream, b);
    }
}

/// A video stream being taken in on PipeWire's own thread. Dropping it
/// stops the thread and closes the stream and connection.
pub struct VideoStream {
    lib: &'static Lib,
    thread_loop: Ptr,
    context: Ptr,
    core: Ptr,
    stream: Ptr,
    hook: Box<Hook>,
    callbacks: Box<Callbacks>,
}

// SAFETY: the pointers are used only when opening and in Drop, which stops
// PipeWire's thread before touching anything; the callbacks are Send.
unsafe impl Send for VideoStream {}

impl VideoStream {
    /// Connects over `fd` (from the portal), or to this session's PipeWire
    /// when `None`, and takes in `node`. `format` hears the settled format,
    /// `frame` each picture; both run on PipeWire's thread.
    pub fn open(
        fd: Option<OwnedFd>,
        node: u32,
        size: Option<(u32, u32)>,
        format: impl FnMut(VideoFormat) + Send + 'static,
        frame: impl FnMut(Frame<'_>) + Send + 'static,
    ) -> Result<VideoStream, String> {
        let lib = lib()?;
        let err = |what: &str| format!("PipeWire {what}: {}", std::io::Error::last_os_error());
        // SAFETY: each call follows its header; whatever was made so far is
        // undone by Drop on an early return.
        unsafe {
            let thread_loop = (lib.thread_loop_new)(c"kagaz-screen".as_ptr(), std::ptr::null());
            if thread_loop.is_null() {
                return Err(err("could not start"));
            }
            let mut s = VideoStream {
                lib,
                thread_loop,
                context: null_mut(),
                core: null_mut(),
                stream: null_mut(),
                hook: Box::new(Hook([null_mut(); 6])),
                callbacks: Box::new(Callbacks {
                    lib,
                    stream: null_mut(),
                    format: Box::new(format),
                    frame: Box::new(frame),
                }),
            };
            s.context = (lib.context_new)((lib.thread_loop_get_loop)(thread_loop), null_mut(), 0);
            if s.context.is_null() {
                return Err(err("could not start"));
            }
            if (lib.thread_loop_start)(thread_loop) < 0 {
                return Err(err("could not start its thread"));
            }
            (lib.thread_loop_lock)(thread_loop);
            let r = s.connect(fd, node, size);
            (lib.thread_loop_unlock)(thread_loop);
            r.map(|()| s)
        }
    }

    /// Called with the loop locked.
    unsafe fn connect(&mut self, fd: Option<OwnedFd>, node: u32, size: Option<(u32, u32)>) -> Result<(), String> {
        let lib = self.lib;
        // SAFETY: see `open`; the hook and callbacks are boxed, so they stay
        // put for as long as the stream.
        unsafe {
            self.core = match fd {
                // PipeWire takes the descriptor over.
                Some(fd) => (lib.context_connect_fd)(self.context, fd.into_raw_fd(), null_mut(), 0),
                None => (lib.context_connect)(self.context, null_mut(), 0),
            };
            if self.core.is_null() {
                return Err(format!("PipeWire could not be reached: {}", std::io::Error::last_os_error()));
            }
            let props = (lib.properties_new_string)(c"media.type=Video media.category=Capture media.role=Screen".as_ptr());
            self.stream = (lib.stream_new)(self.core, c"kagaz-screen".as_ptr(), props);
            if self.stream.is_null() {
                return Err(format!("PipeWire made no stream: {}", std::io::Error::last_os_error()));
            }
            self.callbacks.stream = self.stream;
            let data = &mut *self.callbacks as *mut Callbacks as Ptr;
            (lib.stream_add_listener)(self.stream, &mut *self.hook, &EVENTS, data);
            // Pods are read in place, so keep the offer on an 8-byte boundary.
            let words = video_offer(size);
            let mut offer = vec![0u64; words.len().div_ceil(2)];
            std::ptr::copy_nonoverlapping(words.as_ptr(), offer.as_mut_ptr() as *mut u32, words.len());
            let mut params = [offer.as_ptr() as *const c_void];
            let r = (lib.stream_connect)(
                self.stream,
                DIRECTION_INPUT,
                node,
                FLAG_AUTOCONNECT | FLAG_MAP_BUFFERS,
                params.as_mut_ptr(),
                1,
            );
            if r < 0 {
                return Err(format!("PipeWire refused the stream: {}", std::io::Error::from_raw_os_error(-r)));
            }
        }
        Ok(())
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        let lib = self.lib;
        // SAFETY: the thread is stopped (and joined) first, so no event
        // runs while the stream, connection and context are taken down.
        unsafe {
            (lib.thread_loop_stop)(self.thread_loop);
            if !self.stream.is_null() {
                (lib.stream_destroy)(self.stream);
            }
            if !self.core.is_null() {
                (lib.core_disconnect)(self.core);
            }
            if !self.context.is_null() {
                (lib.context_destroy)(self.context);
            }
            (lib.thread_loop_destroy)(self.thread_loop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_ne_bytes()).collect()
    }

    /// The offer, word by word, as libspa's own pod builder writes it
    /// (checked once against the `pipewire` crate's serializer).
    #[test]
    fn offer_matches_spa() {
        #[rustfmt::skip]
        let spa: [u32; 58] = [
            0xe0, TYPE_OBJECT, OBJECT_FORMAT, PARAM_ENUM_FORMAT,
            // media type: Id video, padded
            1, 0, 4, TYPE_ID, 2, 0,
            // media subtype: Id raw, padded
            2, 0, 4, TYPE_ID, 1, 0,
            // video format: Enum of Ids, default BGRx; 36 bytes then padding
            0x20001, 0, 0x24, TYPE_CHOICE, CHOICE_ENUM, 0, 4, TYPE_ID, 8, 8, 7, 12, 11, 0,
            // size: Range of Rectangles
            0x20003, 0, 0x28, TYPE_CHOICE, CHOICE_RANGE, 0, 8, TYPE_RECTANGLE, 1920, 1080, 1, 1, 8192, 8192,
            // framerate: Range of Fractions
            0x20004, 0, 0x28, TYPE_CHOICE, CHOICE_RANGE, 0, 8, TYPE_FRACTION, 60, 1, 0, 1, 1000, 1,
        ];
        assert_eq!(video_offer(None), spa);
    }

    #[test]
    fn offer_for_a_fixed_size() {
        let o = video_offer(Some((1280, 720)));
        // Body size counts everything after the 8-byte header.
        assert_eq!(o[0] as usize, (o.len() - 2) * 4);
        let at = o.windows(2).position(|w| w == [FORMAT_VIDEO_SIZE, 0]).unwrap();
        assert_eq!(&o[at + 2..at + 6], &[8, TYPE_RECTANGLE, 1280, 720]);
        assert_eq!(o.len() % 2, 0);
    }

    #[test]
    fn reads_a_format() {
        // As the `pipewire` crate wrote it: video, RGBA, 1280x800.
        #[rustfmt::skip]
        let pod = bytes(&[
            0x50, TYPE_OBJECT, OBJECT_FORMAT, PARAM_FORMAT,
            1, 0, 4, TYPE_ID, 2, 0,
            0x20001, 0, 4, TYPE_ID, 11, 0,
            0x20003, 0, 8, TYPE_RECTANGLE, 1280, 800,
        ]);
        assert_eq!(video_format(&pod), Some(VideoFormat { format: RGBA, width: 1280, height: 800 }));
    }

    #[test]
    fn reads_choices_and_its_own_offer() {
        // The offer itself reads as its preferred values.
        let offer = bytes(&video_offer(None));
        assert_eq!(video_format(&offer), Some(VideoFormat { format: BGRX, width: 1920, height: 1080 }));
        let fixed = bytes(&video_offer(Some((1920, 1080))));
        assert_eq!(video_format(&fixed).map(|f| (f.width, f.height)), Some((1920, 1080)));
    }

    #[test]
    fn refuses_broken_pods() {
        let offer = bytes(&video_offer(None));
        assert_eq!(video_format(&offer[..40]), None);
        assert_eq!(video_format(&[]), None);
        let mut other = offer.clone();
        other[4] = TYPE_CHOICE as u8;
        assert_eq!(video_format(&other), None);
        // A property running past the end.
        let mut long = offer.clone();
        long[24] = 0xff;
        assert_eq!(video_format(&long), None);
    }

    /// Sizes and offsets as a C compiler lays out PipeWire 1.6's headers
    /// on a 64-bit machine.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn layouts_match_the_headers() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<Hook>(), 48);
        assert_eq!(size_of::<StreamEvents>(), 96);
        assert_eq!(offset_of!(StreamEvents, param_changed), 40);
        assert_eq!(offset_of!(StreamEvents, process), 64);
        assert_eq!(size_of::<SpaBuffer>(), 24);
        assert_eq!(offset_of!(SpaBuffer, datas), 16);
        assert_eq!(size_of::<SpaData>(), 40);
        assert_eq!(offset_of!(SpaData, maxsize), 20);
        assert_eq!(offset_of!(SpaData, data), 24);
        assert_eq!(offset_of!(SpaData, chunk), 32);
        assert_eq!(size_of::<SpaChunk>(), 16);
        assert_eq!(offset_of!(SpaChunk, stride), 8);
    }
}
