//! This computer's screen on macOS, for mirroring to a TV: the main
//! display through ScreenCaptureKit (macOS 12.3 or newer), pointer
//! included. macOS asks the owner once for Screen Recording permission.
//! Kagaz keeps the newest picture and scales it to the TV's size. From
//! macOS 13 the same stream also carries what the computer plays (not
//! Kagaz's own sound), taken only while the sound is to go to the TV.
//!
//! Not yet run on a Mac: written to Apple's documentation and type-checked
//! for macOS from Linux.

use super::picture::{scale, Layout};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType,
};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// CoreVideo and CoreMedia's C functions for reading a picture.
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferLockBaseAddress(pb: *const c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: *const c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: *const c_void) -> *const u8;
    fn CVPixelBufferGetBytesPerRow(pb: *const c_void) -> usize;
    fn CVPixelBufferGetWidth(pb: *const c_void) -> usize;
    fn CVPixelBufferGetHeight(pb: *const c_void) -> usize;
}
#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetImageBuffer(sample: *const c_void) -> *const c_void;
    fn CMSampleBufferGetFormatDescription(sample: *const c_void) -> *const c_void;
    fn CMAudioFormatDescriptionGetStreamBasicDescription(desc: *const c_void) -> *const StreamDescription;
    fn CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sample: *const c_void,
        size_needed: *mut usize,
        list: *mut c_void,
        list_size: usize,
        allocator: *const c_void,
        memory_allocator: *const c_void,
        flags: u32,
        block: *mut *const c_void,
    ) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// AudioStreamBasicDescription (CoreAudioTypes.h).
#[repr(C)]
struct StreamDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

/// AudioBuffer (CoreAudioTypes.h).
#[repr(C)]
#[derive(Clone, Copy)]
struct AudioBuffer {
    channels: u32,
    size: u32,
    data: *const c_void,
}

const FLOAT: u32 = 1;
const NON_INTERLEAVED: u32 = 1 << 5;

/// The computer's sound as it played, 16-bit 48 kHz stereo, taken while
/// the sound is to go to the TV (sound_mac.rs takes it from here).
pub static SOUND_IN: Mutex<std::collections::VecDeque<i16>> = Mutex::new(std::collections::VecDeque::new());

/// The capture carries sound (macOS 13 and later).
pub static HAS_SOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// One sample buffer of sound into [`SOUND_IN`].
///
/// # Safety
/// `sample` is a live CMSampleBuffer of sound.
unsafe fn take_sound(sample: *const c_void) {
    unsafe {
        let desc = CMSampleBufferGetFormatDescription(sample);
        if desc.is_null() {
            return;
        }
        let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(desc);
        if asbd.is_null() {
            return;
        }
        let d = &*asbd;
        if d.format_flags & FLOAT == 0 || d.bits_per_channel != 32 {
            return;
        }
        // Room for a list of up to eight buffers.
        let mut list = [0u64; 1 + 8 * 2];
        let mut block: *const c_void = std::ptr::null();
        let r = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample,
            std::ptr::null_mut(),
            list.as_mut_ptr() as *mut c_void,
            std::mem::size_of_val(&list),
            std::ptr::null(),
            std::ptr::null(),
            1,
            &mut block,
        );
        if r != 0 {
            return;
        }
        let count = (list.as_ptr() as *const u32).read() as usize;
        let buffers = std::slice::from_raw_parts((list.as_ptr() as *const u8).add(8) as *const AudioBuffer, count.min(8));
        let planar = d.format_flags & NON_INTERLEAVED != 0;
        let channels = d.channels_per_frame.max(1) as usize;
        let at = |c: usize, i: usize| -> f32 {
            let c = c.min(channels - 1);
            if planar {
                let b = buffers[c.min(buffers.len() - 1)];
                *(b.data as *const f32).add(i)
            } else {
                *(buffers[0].data as *const f32).add(i * channels + c)
            }
        };
        let frames = if planar {
            buffers.first().map(|b| b.size as usize / 4).unwrap_or(0)
        } else {
            buffers.first().map(|b| b.size as usize / 4 / channels).unwrap_or(0)
        };
        let mut q = SOUND_IN.lock().unwrap();
        for i in 0..frames {
            for c in 0..2 {
                q.push_back((at(c, i).clamp(-1.0, 1.0) * 32767.0) as i16);
            }
        }
        // Half a second waiting at most.
        while q.len() > 48_000 {
            q.pop_front();
        }
        if !block.is_null() {
            CFRelease(block);
        }
    }
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
}
const LOCK_READ_ONLY: u64 = 1;

/// The newest picture, as it came (BGRA).
#[derive(Default)]
struct Latest {
    width: usize,
    height: usize,
    stride: usize,
    data: Vec<u8>,
    seq: u64,
}

struct Shared {
    latest: Mutex<Latest>,
    arrived: AtomicU64,
}

define_class!(
    // Hears each picture ScreenCaptureKit makes.
    #[unsafe(super(NSObject))]
    #[name = "KagazScreenOutput"]
    #[ivars = Arc<Shared>]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Audio {
                if super::switches::sound_to_tv() {
                    // SAFETY: a sound sample buffer, live through this call.
                    unsafe { take_sound(sample as *const CMSampleBuffer as *const c_void) };
                }
                return;
            }
            if kind != SCStreamOutputType::Screen {
                return;
            }
            // SAFETY: the sample and its picture live through this call;
            // the picture is locked while read.
            unsafe {
                let pb = CMSampleBufferGetImageBuffer(sample as *const CMSampleBuffer as *const c_void);
                // No picture: the screen did not change.
                if pb.is_null() || CVPixelBufferLockBaseAddress(pb, LOCK_READ_ONLY) != 0 {
                    return;
                }
                let (w, h, stride) = (CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb), CVPixelBufferGetBytesPerRow(pb));
                let base = CVPixelBufferGetBaseAddress(pb);
                if !base.is_null() {
                    let bytes = std::slice::from_raw_parts(base, stride * h);
                    let shared = self.ivars();
                    let mut l = shared.latest.lock().unwrap();
                    l.width = w;
                    l.height = h;
                    l.stride = stride;
                    l.data.clear();
                    l.data.extend_from_slice(bytes);
                    l.seq += 1;
                    shared.arrived.fetch_add(1, Ordering::Relaxed);
                }
                CVPixelBufferUnlockBaseAddress(pb, LOCK_READ_ONLY);
            }
        }
    }
);

impl Output {
    fn new(shared: Arc<Shared>) -> Retained<Output> {
        let this = Output::alloc().set_ivars(shared);
        // SAFETY: NSObject's init on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// The main display being captured. Dropping it ends the capture.
pub struct Screen {
    shared: Arc<Shared>,
    stream: Retained<SCStream>,
    _output: Retained<Output>,
    last_seq: u64,
    taken: Vec<u8>,
    scaled: Vec<u8>,
}

// The stream is only started and stopped from here; pictures arrive on
// ScreenCaptureKit's own queue into the shared state.
unsafe impl Send for Screen {}

/// A completion handler's answer, once it comes.
type Answer<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// Waits for a completion handler's answer.
fn wait<T>(slot: &Mutex<Option<T>>, limit: Duration) -> Option<T> {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if let Some(v) = slot.lock().unwrap().take() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn describe(e: *mut NSError) -> String {
    // SAFETY: a non-null NSError handed to a completion handler.
    unsafe { e.as_ref().map(|e| e.localizedDescription().to_string()).unwrap_or_default() }
}

impl Screen {
    /// Starts capturing the main display (macOS asks for Screen Recording
    /// permission the first time).
    pub fn main() -> Result<Screen, String> {
        unsafe {
            // What can be captured. The answer comes on another thread;
            // Objective-C objects may cross threads (their counts are atomic).
            #[allow(clippy::arc_with_non_send_sync)]
            let answer: Answer<Retained<SCShareableContent>> = Arc::new(Mutex::new(None));
            let a = answer.clone();
            let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
                let r = match Retained::retain(content) {
                    Some(c) => Ok(c),
                    None => Err(describe(error)),
                };
                *a.lock().unwrap() = Some(r);
            });
            SCShareableContent::getShareableContentWithCompletionHandler(&block);
            let content = wait(&answer, Duration::from_secs(60)).ok_or("macOS did not say which screens can be captured")?
                .map_err(|e| format!("macOS did not allow screen capture (System Settings, Privacy & Security, Screen Recording): {e}"))?;
            let main = CGMainDisplayID();
            let displays = content.displays();
            let display: Retained<SCDisplay> = displays
                .iter()
                .find(|d| d.displayID() == main)
                .or_else(|| displays.firstObject())
                .ok_or("macOS lists no display")?;

            let filter = SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::new());
            let config = SCStreamConfiguration::new();
            let id = display.displayID();
            config.setWidth(CGDisplayPixelsWide(id));
            config.setHeight(CGDisplayPixelsHigh(id));
            config.setPixelFormat(u32::from_be_bytes(*b"BGRA"));
            config.setMinimumFrameInterval(CMTime { value: 1, timescale: 60, flags: CMTimeFlags::Valid, epoch: 0 });
            config.setShowsCursor(true);
            config.setQueueDepth(5);
            // The sound too, from macOS 13 (asked first: older ones lack it).
            let can_sound: bool = msg_send![&*config, respondsToSelector: objc2::sel!(setCapturesAudio:)];
            if can_sound {
                let _: () = msg_send![&*config, setCapturesAudio: true];
                let _: () = msg_send![&*config, setSampleRate: 48_000isize];
                let _: () = msg_send![&*config, setChannelCount: 2isize];
                let _: () = msg_send![&*config, setExcludesCurrentProcessAudio: true];
            }

            let shared = Arc::new(Shared { latest: Mutex::new(Latest::default()), arrived: AtomicU64::new(0) });
            let output = Output::new(shared.clone());
            let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None);
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), SCStreamOutputType::Screen, None)
                .map_err(|e| format!("the capture: {}", e.localizedDescription()))?;
            HAS_SOUND.store(
                can_sound
                    && stream
                        .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), SCStreamOutputType::Audio, None)
                        .is_ok(),
                std::sync::atomic::Ordering::SeqCst,
            );
            let started: Answer<()> = Arc::new(Mutex::new(None));
            let s = started.clone();
            let block = RcBlock::new(move |error: *mut NSError| {
                *s.lock().unwrap() = Some(if error.is_null() { Ok(()) } else { Err(describe(error)) });
            });
            stream.startCaptureWithCompletionHandler(Some(&block));
            wait(&started, Duration::from_secs(30))
                .ok_or("macOS did not start the capture")?
                .map_err(|e| format!("macOS did not start the capture: {e}"))?;
            Ok(Screen { shared, stream, _output: output, last_seq: 0, taken: Vec::new(), scaled: Vec::new() })
        }
    }

    /// Pictures that arrived from macOS.
    pub fn arrived(&self) -> u64 {
        self.shared.arrived.load(Ordering::Relaxed)
    }

    /// Waits up to `wait` for the first picture; its size.
    pub fn first_picture(&self, wait: Duration) -> Option<(usize, usize)> {
        let end = Instant::now() + wait;
        while Instant::now() < end {
            {
                let l = self.shared.latest.lock().unwrap();
                if l.seq > 0 {
                    return Some((l.width, l.height));
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// The newest picture scaled to `width` x `height` RGBA, with black
    /// bars where the shapes differ.
    pub fn picture(&mut self, width: usize, height: usize) -> &[u8] {
        if self.scaled.len() != width * height * 4 {
            self.scaled = vec![0; width * height * 4];
            for px in self.scaled.chunks_exact_mut(4) {
                px[3] = 255;
            }
            self.last_seq = 0;
        }
        let (w, h, stride) = {
            let mut l = self.shared.latest.lock().unwrap();
            if l.seq == self.last_seq || l.seq == 0 {
                return &self.scaled;
            }
            std::mem::swap(&mut self.taken, &mut l.data);
            self.last_seq = l.seq;
            (l.width, l.height, l.stride)
        };
        if self.taken.len() >= stride * h {
            scale(&self.taken, w, h, stride, Layout::Bgrx, &mut self.scaled, width, height);
        }
        &self.scaled
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // SAFETY: stopping a stream this Screen started.
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
    }
}
