//! This computer's screen on macOS, for mirroring to a TV: the main
//! display through ScreenCaptureKit (macOS 12.3 or newer), pointer
//! included. macOS asks the owner once for Screen Recording permission.
//! Kagaz keeps the newest picture and scales it to the TV's size.
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

            let shared = Arc::new(Shared { latest: Mutex::new(Latest::default()), arrived: AtomicU64::new(0) });
            let output = Output::new(shared.clone());
            let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None);
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), SCStreamOutputType::Screen, None)
                .map_err(|e| format!("the capture: {}", e.localizedDescription()))?;
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
