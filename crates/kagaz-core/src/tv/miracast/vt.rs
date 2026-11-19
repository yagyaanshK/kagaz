//! H.264 through macOS's VideoToolbox: the Mac's own hardware encoder
//! (Apple silicon and Intel Macs alike). Same stream as the other encoders:
//! Constrained Baseline, no reordering, real-time, a whole picture when the
//! TV asks; Kagaz turns the pictures into NV12 itself (BT.601 video range).
//! VideoToolbox hands out length-prefixed pictures with the sequence and
//! picture headers kept aside; Kagaz turns them into the start-code form
//! the TV takes, headers before each whole picture.
//!
//! Not yet run on a Mac: written to Apple's documentation and type-checked
//! for macOS from Linux.

use super::video::Priority as Power;
use crate::tv::picture::rgba_to_nv12;
use std::ffi::c_void;
use std::ptr::null;

type CFTypeRef = *const c_void;
type OSStatus = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

fn time(value: i64, timescale: i32) -> CMTime {
    CMTime { value, timescale, flags: 1, epoch: 0 }
}

#[repr(C)]
struct DictionaryCallBacks([usize; 6]);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: DictionaryCallBacks;
    static kCFTypeDictionaryValueCallBacks: DictionaryCallBacks;
    fn CFRelease(cf: CFTypeRef);
    fn CFNumberCreate(alloc: CFTypeRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        n: isize,
        key_callbacks: *const DictionaryCallBacks,
        value_callbacks: *const DictionaryCallBacks,
    ) -> CFTypeRef;
    fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFArrayGetCount(array: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, i: isize) -> CFTypeRef;
}
const CF_NUMBER_SINT32: isize = 3;

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferCreate(alloc: CFTypeRef, w: usize, h: usize, format: u32, attrs: CFTypeRef, out: *mut CFTypeRef) -> i32;
    fn CVPixelBufferLockBaseAddress(pb: CFTypeRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: CFTypeRef, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddressOfPlane(pb: CFTypeRef, plane: usize) -> *mut u8;
    fn CVPixelBufferGetBytesPerRowOfPlane(pb: CFTypeRef, plane: usize) -> usize;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    static kCMSampleAttachmentKey_NotSync: CFTypeRef;
    fn CMSampleBufferGetDataBuffer(sample: CFTypeRef) -> CFTypeRef;
    fn CMSampleBufferGetFormatDescription(sample: CFTypeRef) -> CFTypeRef;
    fn CMSampleBufferGetSampleAttachmentsArray(sample: CFTypeRef, create: u8) -> CFTypeRef;
    fn CMBlockBufferGetDataLength(buffer: CFTypeRef) -> usize;
    fn CMBlockBufferCopyDataBytes(buffer: CFTypeRef, offset: usize, length: usize, dest: *mut c_void) -> OSStatus;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        desc: CFTypeRef,
        index: usize,
        set: *mut *const u8,
        size: *mut usize,
        count: *mut usize,
        header_length: *mut i32,
    ) -> OSStatus;
}

type OutputCallback = unsafe extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CFTypeRef);

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    static kVTCompressionPropertyKey_RealTime: CFTypeRef;
    static kVTCompressionPropertyKey_ProfileLevel: CFTypeRef;
    static kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel: CFTypeRef;
    static kVTProfileLevel_H264_Baseline_AutoLevel: CFTypeRef;
    static kVTCompressionPropertyKey_AllowFrameReordering: CFTypeRef;
    static kVTCompressionPropertyKey_AverageBitRate: CFTypeRef;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFTypeRef;
    static kVTCompressionPropertyKey_ExpectedFrameRate: CFTypeRef;
    static kVTCompressionPropertyKey_ColorPrimaries: CFTypeRef;
    static kVTCompressionPropertyKey_TransferFunction: CFTypeRef;
    static kVTCompressionPropertyKey_YCbCrMatrix: CFTypeRef;
    static kVTEncodeFrameOptionKey_ForceKeyFrame: CFTypeRef;
    static kVTVideoEncoderSpecification_EnableLowLatencyRateControl: CFTypeRef;
    fn VTCompressionSessionCreate(
        alloc: CFTypeRef,
        width: i32,
        height: i32,
        codec: u32,
        encoder_spec: CFTypeRef,
        source_attrs: CFTypeRef,
        data_alloc: CFTypeRef,
        callback: OutputCallback,
        refcon: *mut c_void,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn VTSessionSetProperty(session: CFTypeRef, key: CFTypeRef, value: CFTypeRef) -> OSStatus;
    fn VTCompressionSessionPrepareToEncodeFrames(session: CFTypeRef) -> OSStatus;
    fn VTCompressionSessionEncodeFrame(
        session: CFTypeRef,
        image: CFTypeRef,
        pts: CMTime,
        duration: CMTime,
        frame_props: CFTypeRef,
        frame_refcon: *mut c_void,
        info_out: *mut u32,
    ) -> OSStatus;
    fn VTCompressionSessionCompleteFrames(session: CFTypeRef, until: CMTime) -> OSStatus;
    fn VTCompressionSessionInvalidate(session: CFTypeRef);
}
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    static kCVImageBufferColorPrimaries_SMPTE_C: CFTypeRef;
    static kCVImageBufferTransferFunction_ITU_R_709_2: CFTypeRef;
    static kCVImageBufferYCbCrMatrix_ITU_R_601_4: CFTypeRef;
}

const H264: u32 = u32::from_be_bytes(*b"avc1");
/// NV12, video range.
const NV12_VIDEO: u32 = u32::from_be_bytes(*b"420v");

/// What the output callback gathers for one picture.
#[derive(Default)]
struct Out {
    bytes: Vec<u8>,
    key: bool,
    error: Option<OSStatus>,
}

/// One H.264 encoder on the Mac.
pub struct VtEncoder {
    session: CFTypeRef,
    picture: CFTypeRef,
    out: Box<Out>,
    width: usize,
    height: usize,
    fps: u32,
    frame: i64,
    key_next: bool,
    /// For the record: which encoder.
    pub described: String,
}

// The session is used from one thread at a time.
unsafe impl Send for VtEncoder {}

unsafe extern "C" fn on_output(refcon: *mut c_void, _frame: *mut c_void, status: OSStatus, _flags: u32, sample: CFTypeRef) {
    // SAFETY: `refcon` is the encoder's Out box, alive while it encodes;
    // the sample lives through the call.
    unsafe {
        let out = &mut *(refcon as *mut Out);
        if status != 0 {
            out.error = Some(status);
            return;
        }
        if sample.is_null() {
            return; // dropped by the encoder
        }
        // A whole picture unless marked "not sync".
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, 0);
        let mut key = true;
        if !attachments.is_null() && CFArrayGetCount(attachments) > 0 {
            let first = CFArrayGetValueAtIndex(attachments, 0);
            let not_sync = CFDictionaryGetValue(first, kCMSampleAttachmentKey_NotSync);
            key = not_sync.is_null() || not_sync == kCFBooleanFalse;
        }
        if key {
            // The sequence and picture headers, each with a start code.
            let desc = CMSampleBufferGetFormatDescription(sample);
            let mut count = 0usize;
            let _ = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, 0, std::ptr::null_mut(), std::ptr::null_mut(), &mut count, std::ptr::null_mut());
            for i in 0..count {
                let (mut p, mut n) = (null(), 0usize);
                if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, i, &mut p, &mut n, std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
                    out.bytes.extend_from_slice(&[0, 0, 0, 1]);
                    out.bytes.extend_from_slice(std::slice::from_raw_parts(p, n));
                }
            }
        }
        let block = CMSampleBufferGetDataBuffer(sample);
        if block.is_null() {
            return;
        }
        let len = CMBlockBufferGetDataLength(block);
        let mut data = vec![0u8; len];
        if CMBlockBufferCopyDataBytes(block, 0, len, data.as_mut_ptr() as *mut c_void) != 0 {
            return;
        }
        out.bytes.extend(avcc_to_annex_b(&data));
        out.key |= key;
    }
}

/// Four-byte lengths before each unit, as VideoToolbox writes them, into
/// start codes.
fn avcc_to_annex_b(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut at = 0;
    while at + 4 <= data.len() {
        let n = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        at += 4;
        let end = (at + n).min(data.len());
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[at..end]);
        at = end;
    }
    out
}

impl VtEncoder {
    /// Opens the Mac's H.264 encoder at this size.
    pub fn open(width: usize, height: usize, fps: u32, bits_per_second: u32, power: Power) -> Result<VtEncoder, String> {
        unsafe {
            let mut out = Box::<Out>::default();
            // Low-latency rate control (macOS 11.3 and newer).
            let keys = [kVTVideoEncoderSpecification_EnableLowLatencyRateControl];
            let values = [kCFBooleanTrue];
            let spec = CFDictionaryCreate(
                null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let mut session = null();
            let s = VTCompressionSessionCreate(
                null(),
                width as i32,
                height as i32,
                H264,
                spec,
                null(),
                null(),
                on_output,
                &mut *out as *mut Out as *mut c_void,
                &mut session,
            );
            CFRelease(spec);
            if s != 0 || session.is_null() {
                return Err(format!("VideoToolbox could not start an H.264 encoder (error {s})"));
            }
            let number = |v: i32| CFNumberCreate(null(), CF_NUMBER_SINT32, &v as *const i32 as *const c_void);
            let set = |key: CFTypeRef, value: CFTypeRef| VTSessionSetProperty(session, key, value);
            set(kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
            if set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel) != 0 {
                set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_Baseline_AutoLevel);
            }
            set(kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
            for (key, v) in [
                (kVTCompressionPropertyKey_AverageBitRate, bits_per_second as i32),
                (kVTCompressionPropertyKey_MaxKeyFrameInterval, (fps * 10) as i32),
                (kVTCompressionPropertyKey_ExpectedFrameRate, fps as i32),
            ] {
                let n = number(v);
                set(key, n);
                CFRelease(n);
            }
            set(kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_SMPTE_C);
            set(kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
            set(kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_601_4);
            VTCompressionSessionPrepareToEncodeFrames(session);
            let mut picture = null();
            if CVPixelBufferCreate(null(), width, height, NV12_VIDEO, null(), &mut picture) != 0 {
                VTCompressionSessionInvalidate(session);
                CFRelease(session);
                return Err("VideoToolbox: no picture buffer".into());
            }
            let _ = power; // macOS chooses the encoder's power use itself.
            Ok(VtEncoder {
                session,
                picture,
                out,
                width,
                height,
                fps,
                frame: 0,
                key_next: true,
                described: "VideoToolbox (the Mac's own encoder)".into(),
            })
        }
    }

    pub fn key_next(&mut self) {
        self.key_next = true;
    }

    /// A new rate; VideoToolbox takes it while running.
    pub fn set_bit_rate(&mut self, bits_per_second: u32) -> bool {
        // SAFETY: the session is live; the number is released after.
        unsafe {
            let v = bits_per_second as i32;
            let n = CFNumberCreate(null(), CF_NUMBER_SINT32, &v as *const i32 as *const c_void);
            let ok = VTSessionSetProperty(self.session, kVTCompressionPropertyKey_AverageBitRate, n) == 0;
            CFRelease(n);
            ok
        }
    }

    /// Encodes one RGBA picture; Annex B bytes, and whether it is a whole
    /// (key) picture.
    pub fn encode(&mut self, rgba: &[u8]) -> Result<(Vec<u8>, bool), String> {
        let (w, h) = (self.width, self.height);
        unsafe {
            if CVPixelBufferLockBaseAddress(self.picture, 0) != 0 {
                return Err("VideoToolbox: the picture buffer could not be written".into());
            }
            let (yp, uvp) = (CVPixelBufferGetBytesPerRowOfPlane(self.picture, 0), CVPixelBufferGetBytesPerRowOfPlane(self.picture, 1));
            let y = std::slice::from_raw_parts_mut(CVPixelBufferGetBaseAddressOfPlane(self.picture, 0), yp * h);
            let uv = std::slice::from_raw_parts_mut(CVPixelBufferGetBaseAddressOfPlane(self.picture, 1), uvp * h / 2);
            rgba_to_nv12(rgba, w, h, y, yp, uv, uvp);
            CVPixelBufferUnlockBaseAddress(self.picture, 0);

            let mut props = null();
            if std::mem::take(&mut self.key_next) {
                let keys = [kVTEncodeFrameOptionKey_ForceKeyFrame];
                let values = [kCFBooleanTrue];
                props = CFDictionaryCreate(
                    null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks,
                    &kCFTypeDictionaryValueCallBacks,
                );
            }
            *self.out = Out::default();
            let pts = time(self.frame, self.fps as i32);
            self.frame += 1;
            let s = VTCompressionSessionEncodeFrame(self.session, self.picture, pts, time(1, self.fps as i32), props, std::ptr::null_mut(), std::ptr::null_mut());
            if !props.is_null() {
                CFRelease(props);
            }
            if s != 0 {
                return Err(format!("VideoToolbox could not encode (error {s})"));
            }
            // Wait for this picture (the callback fills `out`).
            VTCompressionSessionCompleteFrames(self.session, pts);
            if let Some(e) = self.out.error {
                return Err(format!("VideoToolbox could not encode (error {e})"));
            }
            let out = std::mem::take(&mut *self.out);
            Ok((out.bytes, out.key))
        }
    }
}

impl Drop for VtEncoder {
    fn drop(&mut self) {
        unsafe {
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session);
            CFRelease(self.picture);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::avcc_to_annex_b;

    #[test]
    fn lengths_become_start_codes() {
        let avcc = [0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 1, 0x41];
        assert_eq!(avcc_to_annex_b(&avcc), vec![0, 0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0x41]);
    }
}
