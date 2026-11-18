//! H.264 through Windows' Media Foundation: the graphics chip's encoder
//! (NVIDIA, AMD or Intel, whichever the computer has) where there is one,
//! else Microsoft's own. Same stream as the other encoders: Constrained
//! Baseline, no B pictures, low latency, a whole picture when the TV asks;
//! Kagaz turns the pictures into NV12 itself (BT.601 video range).
//!
//! Hardware encoders in Media Foundation work asynchronously (they say
//! when they want a picture and when one is ready); Microsoft's works in
//! step. Both are driven here.
//!
//! Not yet run on Windows: written to Microsoft's documentation and checked
//! by building for Windows from Linux.

use super::video::Priority as Power;
use crate::tv::picture::rgba_to_nv12;
use std::time::{Duration, Instant};
use windows::core::{Interface, GUID};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::VARIANT;

fn err(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("Media Foundation {what}: {}", e.message())
}

/// Two 32-bit values packed as Media Foundation packs sizes and rates.
fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

/// One H.264 encoder.
pub struct MfEncoder {
    transform: IMFTransform,
    /// Set for an asynchronous (hardware) encoder.
    events: Option<IMFMediaEventGenerator>,
    codec: Option<ICodecAPI>,
    input_id: u32,
    output_id: u32,
    width: usize,
    height: usize,
    fps: u32,
    frame: u64,
    key_next: bool,
    /// Pictures the asynchronous encoder asked for and did not get yet.
    wanted: u32,
    /// For the record: which encoder.
    pub described: String,
}

// Media Foundation objects here are used from one thread at a time.
unsafe impl Send for MfEncoder {}

impl MfEncoder {
    /// Opens the best H.264 encoder Windows has at this size.
    pub fn open(width: usize, height: usize, fps: u32, bits_per_second: u32, power: Power) -> Result<MfEncoder, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(err("start"))?;
            let mut last = String::from("no H.264 encoder found");
            for hardware in [true, false] {
                for (activate, name) in encoders(hardware)? {
                    match MfEncoder::try_one(&activate, &name, hardware, width, height, fps, bits_per_second, power) {
                        Ok(e) => return Ok(e),
                        Err(e) => last = e,
                    }
                }
            }
            Err(last)
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn try_one(
        activate: &IMFActivate,
        name: &str,
        hardware: bool,
        width: usize,
        height: usize,
        fps: u32,
        bits_per_second: u32,
        power: Power,
    ) -> Result<MfEncoder, String> {
        let transform: IMFTransform = activate.ActivateObject().map_err(err("encoder"))?;
        let mut events = None;
        if let Ok(attrs) = transform.GetAttributes() {
            if attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0 {
                attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).map_err(err("unlocking the encoder"))?;
                events = Some(transform.cast::<IMFMediaEventGenerator>().map_err(err("encoder events"))?);
            }
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
        }
        let (mut input_id, mut output_id) = (0u32, 0u32);
        let _ = transform.GetStreamIDs(std::slice::from_mut(&mut input_id), std::slice::from_mut(&mut output_id));

        // The output first (encoders want it before the input).
        let out = MFCreateMediaType().map_err(err("output type"))?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(err("output type"))?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(err("output type"))?;
        out.SetUINT32(&MF_MT_AVG_BITRATE, bits_per_second).map_err(err("output type"))?;
        out.SetUINT64(&MF_MT_FRAME_SIZE, pack(width as u32, height as u32)).map_err(err("output type"))?;
        out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(err("output type"))?;
        out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(err("output type"))?;
        out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(err("output type"))?;
        let mut set = Err(String::new());
        for profile in [eAVEncH264VProfile_ConstrainedBase, eAVEncH264VProfile_Base] {
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, profile.0 as u32).map_err(err("output type"))?;
            set = transform.SetOutputType(output_id, &out, 0).map_err(err("baseline H.264"));
            if set.is_ok() {
                break;
            }
        }
        set?;
        let input = MFCreateMediaType().map_err(err("input type"))?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(err("input type"))?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(err("input type"))?;
        input.SetUINT64(&MF_MT_FRAME_SIZE, pack(width as u32, height as u32)).map_err(err("input type"))?;
        input.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(err("input type"))?;
        input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(err("input type"))?;
        input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(err("input type"))?;
        transform.SetInputType(input_id, &input, 0).map_err(err("NV12 pictures"))?;

        // Low delay, no B pictures, a whole picture every ten seconds,
        // variable bitrate; each is a wish, as not every encoder takes all.
        let codec = transform.cast::<ICodecAPI>().ok();
        if let Some(c) = &codec {
            let set = |key: &GUID, v: VARIANT| {
                let _ = c.SetValue(key, &v);
            };
            set(&CODECAPI_AVLowLatencyMode, VARIANT::from(true));
            set(&CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
            set(&CODECAPI_AVEncMPVGOPSize, VARIANT::from(fps * 10));
            set(&CODECAPI_AVEncCommonRateControlMode, VARIANT::from(eAVEncCommonRateControlMode_PeakConstrainedVBR.0 as u32));
            set(&CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(bits_per_second));
            set(&CODECAPI_AVEncCommonMaxBitRate, VARIANT::from(bits_per_second / 2 * 3));
            set(
                &CODECAPI_AVEncCommonQualityVsSpeed,
                VARIANT::from(match power {
                    Power::Battery => 0u32,
                    Power::Performance => 50,
                }),
            );
        }
        transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0).map_err(err("start"))?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(err("start"))?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(err("start"))?;
        Ok(MfEncoder {
            transform,
            events,
            codec,
            input_id,
            output_id,
            width,
            height,
            fps,
            frame: 0,
            key_next: true,
            wanted: 0,
            described: format!(
                "Media Foundation, {name}{}",
                if hardware { " (the graphics chip)" } else { "" }
            ),
        })
    }

    pub fn key_next(&mut self) {
        self.key_next = true;
    }

    /// Encodes one RGBA picture; Annex B bytes (empty when the encoder
    /// holds it back a moment), and whether a whole (key) picture came out.
    pub fn encode(&mut self, rgba: &[u8]) -> Result<(Vec<u8>, bool), String> {
        let sample = self.sample(rgba)?;
        if std::mem::take(&mut self.key_next) {
            if let Some(c) = &self.codec {
                unsafe {
                    let _ = c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32));
                }
            }
        }
        match self.events.clone() {
            None => self.step(&sample),
            Some(events) => self.asynchronous(&events, &sample),
        }
    }

    /// The picture as an NV12 sample, stamped with its time.
    fn sample(&mut self, rgba: &[u8]) -> Result<IMFSample, String> {
        let (w, h) = (self.width, self.height);
        let size = (w * h * 3 / 2) as u32;
        unsafe {
            let buffer = MFCreateMemoryBuffer(size).map_err(err("picture"))?;
            let mut p = std::ptr::null_mut();
            buffer.Lock(&mut p, None, None).map_err(err("picture"))?;
            let all = std::slice::from_raw_parts_mut(p, size as usize);
            let (y, uv) = all.split_at_mut(w * h);
            rgba_to_nv12(rgba, w, h, y, w, uv, w);
            buffer.Unlock().map_err(err("picture"))?;
            buffer.SetCurrentLength(size).map_err(err("picture"))?;
            let sample = MFCreateSample().map_err(err("picture"))?;
            sample.AddBuffer(&buffer).map_err(err("picture"))?;
            // In 100 ns units.
            let step = 10_000_000 / self.fps.max(1) as i64;
            sample.SetSampleTime(self.frame as i64 * step).map_err(err("picture"))?;
            sample.SetSampleDuration(step).map_err(err("picture"))?;
            self.frame += 1;
            Ok(sample)
        }
    }

    /// Microsoft's encoder: give the picture, take what comes out.
    fn step(&mut self, sample: &IMFSample) -> Result<(Vec<u8>, bool), String> {
        unsafe {
            self.transform.ProcessInput(self.input_id, sample, 0).map_err(err("encoding"))?;
        }
        let mut out = (Vec::new(), false);
        while let Some((bytes, key)) = self.output()? {
            out.0.extend(bytes);
            out.1 |= key;
        }
        Ok(out)
    }

    /// A hardware encoder: wait for its call for a picture, give it, then
    /// wait a little for the result (it usually comes within a few ms).
    fn asynchronous(&mut self, events: &IMFMediaEventGenerator, sample: &IMFSample) -> Result<(Vec<u8>, bool), String> {
        let mut given = false;
        let mut out = (Vec::new(), false);
        let end = Instant::now() + Duration::from_millis(50);
        loop {
            if !given && self.wanted > 0 {
                unsafe { self.transform.ProcessInput(self.input_id, sample, 0).map_err(err("encoding"))? };
                self.wanted -= 1;
                given = true;
            }
            let event = unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) };
            match event {
                Ok(e) => {
                    let kind = unsafe { e.GetType().map_err(err("encoder event"))? };
                    if kind == METransformNeedInput.0 as u32 {
                        self.wanted += 1;
                    } else if kind == METransformHaveOutput.0 as u32 {
                        if let Some((bytes, key)) = self.output()? {
                            out.0.extend(bytes);
                            out.1 |= key;
                            if given {
                                return Ok(out);
                            }
                        }
                    }
                }
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() > end {
                        // Too slow this time: the picture comes out with a
                        // later one (or, not given, is skipped).
                        return Ok(out);
                    }
                    std::thread::sleep(Duration::from_micros(500));
                }
                Err(e) => return Err(err("encoder event")(e)),
            }
        }
    }

    /// One encoded picture if the encoder has one ready.
    fn output(&mut self) -> Result<Option<(Vec<u8>, bool)>, String> {
        unsafe {
            let info = self.transform.GetOutputStreamInfo(self.output_id).map_err(err("output"))?;
            let provides = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
            let mut own = None;
            if !provides {
                let s = MFCreateSample().map_err(err("output"))?;
                let b = MFCreateMemoryBuffer(info.cbSize.max(1 << 20)).map_err(err("output"))?;
                s.AddBuffer(&b).map_err(err("output"))?;
                own = Some(s);
            }
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: self.output_id,
                pSample: std::mem::ManuallyDrop::new(own),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = self.transform.ProcessOutput(0, &mut buffers, &mut status);
            let sample = std::mem::ManuallyDrop::take(&mut buffers[0].pSample);
            drop(std::mem::ManuallyDrop::take(&mut buffers[0].pEvents));
            match r {
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // The encoder settled its output type: take it again.
                    if let Ok(t) = self.transform.GetOutputAvailableType(self.output_id, 0) {
                        let _ = self.transform.SetOutputType(self.output_id, &t, 0);
                    }
                    return Ok(None);
                }
                Err(e) => return Err(err("output")(e)),
                Ok(()) => {}
            }
            let Some(sample) = sample else { return Ok(None) };
            let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let buffer = sample.ConvertToContiguousBuffer().map_err(err("output"))?;
            let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
            buffer.Lock(&mut p, None, Some(&mut len)).map_err(err("output"))?;
            let bytes = std::slice::from_raw_parts(p, len as usize).to_vec();
            buffer.Unlock().map_err(err("output"))?;
            Ok(Some((bytes, key)))
        }
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

/// The H.264 encoders Windows lists, best first: (how to make it, name).
unsafe fn encoders(hardware: bool) -> Result<Vec<(IMFActivate, String)>, String> {
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let flags = if hardware {
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
    } else {
        MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER
    };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &mut list, &mut count)
        .map_err(err("listing encoders"))?;
    let mut found = Vec::new();
    if !list.is_null() {
        for i in 0..count as usize {
            if let Some(a) = (*list.add(i)).take() {
                let name = friendly_name(&a).unwrap_or_else(|| "an H.264 encoder".into());
                found.push((a, name));
            }
        }
        CoTaskMemFree(Some(list as *const _));
    }
    Ok(found)
}

unsafe fn friendly_name(a: &IMFActivate) -> Option<String> {
    let mut p = windows::core::PWSTR::null();
    let mut len = 0u32;
    a.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut len).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}
