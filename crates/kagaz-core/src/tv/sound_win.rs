//! A computer's sound on the TV (Windows). While the session's Sound on
//! TV is on, Windows' loopback recording takes what the default output
//! plays, and the output is muted so the computer goes quiet; the mute
//! goes back as it was when the sound comes back to the computer or the
//! session ends.
//!
//! The sound goes to the TV as AAC (Windows' own encoder, in Media
//! Foundation; libopus does not build for Windows from Linux), 1024
//! samples a packet, through the same queue as a phone's. Silence fills
//! the gaps, so the TV's player never waits for sound that is not coming.
//!
//! Not yet run on Windows: written to Microsoft's documentation and
//! checked by building for Windows from Linux. Whether a muted output's
//! loopback still carries the sound is to be seen on a real computer.

use super::miracast::external;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::GUID;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

const RATE: u32 = 48_000;
/// An AAC frame: 1024 samples a channel, two channels.
const FRAME: usize = 1024 * 2;

fn err(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("Windows' sound {what}: {}", e.message())
}

/// Whether this computer can send its sound: Windows' AAC encoder is
/// there (not on the N editions without the Media Feature Pack).
pub fn available() -> bool {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, MFSTARTUP_LITE).is_err() {
            return false;
        }
        CoCreateInstance::<_, IMFTransform>(&AACMFTEncoder, None, CLSCTX_INPROC_SERVER).is_ok()
    })
    .join()
    .unwrap_or(false)
}

/// The sound's side of a session; dropping it puts everything back.
pub struct Sound {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Sound {
    pub fn start(_tv: &str) -> Sound {
        external::sound_is_aac(true);
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("kagaz-tv-sound".into())
            .spawn(move || {
                if let Err(e) = follow(&s) {
                    eprintln!("kagaz: the TV's sound: {e}");
                }
            })
            .ok();
        Sound { stop, thread }
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        external::sound_is_aac(false);
    }
}

fn follow(stop: &AtomicBool) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(err("encoder"))?;
    }
    let mut aac = Aac::new()?;
    let mut taking: Option<Loopback> = None;
    let mut samples: VecDeque<i16> = VecDeque::new();
    let start = Instant::now();
    let mut made: u64 = 0;
    let mut frame = vec![0i16; FRAME];
    while !stop.load(Ordering::SeqCst) {
        let want = super::switches::sound_to_tv();
        if want && taking.is_none() {
            match Loopback::open() {
                Ok(l) => taking = Some(l),
                Err(e) => eprintln!("kagaz: the computer's sound could not be taken: {e}"),
            }
        } else if !want && taking.is_some() {
            taking = None;
            samples.clear();
        }
        if let Some(l) = &mut taking {
            if let Err(e) = l.read(&mut samples) {
                eprintln!("kagaz: the computer's sound stopped: {e}");
                taking = None;
            }
            // Half a second waiting at most.
            while samples.len() > RATE as usize {
                samples.pop_front();
            }
        }
        let due = start.elapsed().as_micros() as u64 * RATE as u64 / 1_000_000 * 2;
        loop {
            let behind = due.saturating_sub(made);
            if samples.len() >= FRAME {
                for (f, s) in frame.iter_mut().zip(samples.drain(..FRAME)) {
                    *f = s;
                }
            } else if behind >= FRAME as u64 * 3 {
                frame.iter_mut().for_each(|s| *s = 0);
            } else {
                break;
            }
            for packet in aac.encode(&frame, made / 2)? {
                external::push_sound(packet);
            }
            made += FRAME as u64;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// The default output's sound as it plays, the output muted meanwhile.
struct Loopback {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    volume: Option<IAudioEndpointVolume>,
    /// The mute as it was, to put back.
    was_muted: Option<bool>,
    channels: usize,
    rate: u32,
    float: bool,
    bits: u16,
    /// Where the next 48 kHz sample falls between the output's samples.
    phase: f64,
    last: [f32; 2],
}

/// IEEE float in WAVEFORMATEXTENSIBLE's SubFormat.
const FLOAT_FORMAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
/// Format tags (mmreg.h).
const WAVE_FORMAT_IEEE_FLOAT: u32 = 3;
const WAVE_FORMAT_EXTENSIBLE: u32 = 0xfffe;

impl Loopback {
    fn open() -> Result<Loopback, String> {
        unsafe {
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err("devices"))?;
            let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).map_err(err("output"))?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(err("output"))?;
            let mix = client.GetMixFormat().map_err(err("format"))?;
            let f = &*mix;
            let channels = f.nChannels.max(1) as usize;
            let rate = f.nSamplesPerSec;
            let bits = f.wBitsPerSample;
            let tag = f.wFormatTag as u32;
            let float = tag == WAVE_FORMAT_IEEE_FLOAT
                || (tag == WAVE_FORMAT_EXTENSIBLE && {
                    let x = &*(mix as *const WAVEFORMATEXTENSIBLE);
                    let sub = std::ptr::addr_of!(x.SubFormat).read_unaligned();
                    sub == FLOAT_FORMAT
                });
            let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, mix, None);
            CoTaskMemFree(Some(mix as *const _));
            r.map_err(err("loopback"))?;
            let capture: IAudioCaptureClient = client.GetService().map_err(err("loopback"))?;
            client.Start().map_err(err("loopback"))?;
            // The computer goes quiet: its output muted, the mute as it
            // was kept to put back.
            let volume: Option<IAudioEndpointVolume> = device.Activate(CLSCTX_ALL, None).ok();
            let mut was_muted = None;
            if let Some(v) = &volume {
                if let Ok(m) = v.GetMute() {
                    was_muted = Some(m.as_bool());
                    let _ = v.SetMute(true, std::ptr::null());
                }
            }
            Ok(Loopback { client, capture, volume, was_muted, channels, rate, float, bits, phase: 0.0, last: [0.0; 2] })
        }
    }

    /// What has played since, as 16-bit 48 kHz stereo.
    fn read(&mut self, out: &mut VecDeque<i16>) -> Result<(), String> {
        unsafe {
            loop {
                let n = self.capture.GetNextPacketSize().map_err(err("loopback"))?;
                if n == 0 {
                    return Ok(());
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).map_err(err("loopback"))?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let step = (self.bits as usize / 8).max(1);
                for i in 0..frames as usize {
                    let mut lr = [0f32; 2];
                    if !silent && !data.is_null() {
                        for (c, v) in lr.iter_mut().enumerate() {
                            let ch = c.min(self.channels - 1);
                            let at = data.add((i * self.channels + ch) * step);
                            *v = if self.float && self.bits == 32 {
                                (at as *const f32).read_unaligned()
                            } else if self.bits == 16 {
                                (at as *const i16).read_unaligned() as f32 / 32768.0
                            } else if self.bits == 32 {
                                (at as *const i32).read_unaligned() as f32 / 2_147_483_648.0
                            } else {
                                0.0
                            };
                        }
                    }
                    self.push(lr, out);
                }
                self.capture.ReleaseBuffer(frames).map_err(err("loopback"))?;
            }
        }
    }

    /// One frame of the output's rate, as many 48 kHz frames as fall by it
    /// (straight-line in between).
    fn push(&mut self, lr: [f32; 2], out: &mut VecDeque<i16>) {
        let step = self.rate as f64 / RATE as f64;
        while self.phase < 1.0 {
            for c in 0..2 {
                let v = self.last[c] + (lr[c] - self.last[c]) * self.phase as f32;
                out.push_back((v.clamp(-1.0, 1.0) * 32767.0) as i16);
            }
            self.phase += step;
        }
        self.phase -= 1.0;
        self.last = lr;
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            if let (Some(v), Some(m)) = (&self.volume, self.was_muted) {
                let _ = v.SetMute(m, std::ptr::null());
            }
        }
    }
}

/// Windows' AAC encoder: 16-bit 48 kHz stereo in, raw AAC frames out.
struct Aac {
    t: IMFTransform,
}

impl Aac {
    fn new() -> Result<Aac, String> {
        unsafe {
            let t: IMFTransform = CoCreateInstance(&AACMFTEncoder, None, CLSCTX_INPROC_SERVER).map_err(err("encoder"))?;
            let input = MFCreateMediaType().map_err(err("encoder"))?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(err("encoder"))?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).map_err(err("encoder"))?;
            input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(err("encoder"))?;
            input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE).map_err(err("encoder"))?;
            input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2).map_err(err("encoder"))?;
            input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4).map_err(err("encoder"))?;
            input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * 4).map_err(err("encoder"))?;
            let output = MFCreateMediaType().map_err(err("encoder"))?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(err("encoder"))?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).map_err(err("encoder"))?;
            output.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(err("encoder"))?;
            output.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE).map_err(err("encoder"))?;
            output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2).map_err(err("encoder"))?;
            // 192 kbit/s; raw frames (no ADTS headers); AAC-LC.
            output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000).map_err(err("encoder"))?;
            output.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0).map_err(err("encoder"))?;
            output.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29).map_err(err("encoder"))?;
            // Windows' AAC encoder takes its input type first.
            if t.SetInputType(0, &input, 0).is_err() {
                t.SetOutputType(0, &output, 0).map_err(err("AAC"))?;
                t.SetInputType(0, &input, 0).map_err(err("PCM"))?;
            } else {
                t.SetOutputType(0, &output, 0).map_err(err("AAC"))?;
            }
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(err("encoder"))?;
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(err("encoder"))?;
            Ok(Aac { t })
        }
    }

    /// One frame of sound in; the AAC frames that come out.
    fn encode(&mut self, pcm: &[i16], at_sample: u64) -> Result<Vec<Vec<u8>>, String> {
        unsafe {
            let size = (pcm.len() * 2) as u32;
            let buffer = MFCreateMemoryBuffer(size).map_err(err("encoding"))?;
            let mut p = std::ptr::null_mut();
            buffer.Lock(&mut p, None, None).map_err(err("encoding"))?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, p, size as usize);
            buffer.Unlock().map_err(err("encoding"))?;
            buffer.SetCurrentLength(size).map_err(err("encoding"))?;
            let sample = MFCreateSample().map_err(err("encoding"))?;
            sample.AddBuffer(&buffer).map_err(err("encoding"))?;
            // In 100 ns units.
            sample.SetSampleTime((at_sample * 10_000_000 / RATE as u64) as i64).map_err(err("encoding"))?;
            sample.SetSampleDuration((pcm.len() as u64 / 2 * 10_000_000 / RATE as u64) as i64).map_err(err("encoding"))?;
            self.t.ProcessInput(0, &sample, 0).map_err(err("encoding"))?;
            let mut out = Vec::new();
            loop {
                let info = self.t.GetOutputStreamInfo(0).map_err(err("encoding"))?;
                let s = MFCreateSample().map_err(err("encoding"))?;
                let b = MFCreateMemoryBuffer(info.cbSize.max(8192)).map_err(err("encoding"))?;
                s.AddBuffer(&b).map_err(err("encoding"))?;
                let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(s)),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let r = self.t.ProcessOutput(0, &mut buffers, &mut status);
                let sample = std::mem::ManuallyDrop::take(&mut buffers[0].pSample);
                drop(std::mem::ManuallyDrop::take(&mut buffers[0].pEvents));
                match r {
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(out),
                    Err(e) => return Err(err("encoding")(e)),
                    Ok(()) => {}
                }
                let Some(sample) = sample else { return Ok(out) };
                let buffer = sample.ConvertToContiguousBuffer().map_err(err("encoding"))?;
                let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
                buffer.Lock(&mut p, None, Some(&mut len)).map_err(err("encoding"))?;
                if len > 0 {
                    out.push(std::slice::from_raw_parts(p, len as usize).to_vec());
                }
                buffer.Unlock().map_err(err("encoding"))?;
            }
        }
    }
}
