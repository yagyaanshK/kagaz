//! Windows: what plays on the default speakers, taken as it plays
//! (WASAPI's loopback). The speakers go on playing it.

use crate::sound::Sound;
use std::time::Duration;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};

/// How its samples are kept.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Float32,
    Int16,
    Int32,
    Int24,
}

pub struct Loopback {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    rate: u32,
    channels: usize,
    kind: Kind,
}

// Made and used on the sound's own thread only.
unsafe impl Send for Loopback {}

impl Drop for Loopback {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// The rate the speakers play at, to offer it first.
pub fn native_rate() -> u32 {
    open().map(|l| l.rate).unwrap_or(48000)
}

pub fn open() -> anyhow::Result<Loopback> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let speakers = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = speakers.Activate(CLSCTX_ALL, None)?;
        let mix = client.GetMixFormat()?;
        let (rate, channels, kind) = {
            let f: &WAVEFORMATEX = &*mix;
            let bits = f.wBitsPerSample;
            // 3 is floating point; 0xFFFE says so in its sub-format, whose
            // first number is 3 for floating point and 1 for whole numbers.
            let float = match f.wFormatTag {
                3 => true,
                0xFFFE => {
                    let ext: &WAVEFORMATEXTENSIBLE = &*(mix as *const WAVEFORMATEXTENSIBLE);
                    let sub = ext.SubFormat;
                    sub.data1 == 3
                }
                _ => false,
            };
            let kind = match (float, bits) {
                (true, 32) => Kind::Float32,
                (false, 16) => Kind::Int16,
                (false, 24) => Kind::Int24,
                (false, 32) => Kind::Int32,
                _ => {
                    CoTaskMemFree(Some(mix as *const _));
                    anyhow::bail!("the speakers play a form of sound not known here ({bits} bits)");
                }
            };
            (f.nSamplesPerSec, f.nChannels as usize, kind)
        };
        // Two tenths of a second of room: taken every hundredth.
        let made = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, mix, None);
        CoTaskMemFree(Some(mix as *const _));
        made?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok(Loopback { client, capture, rate, channels, kind })
    }
}

impl Sound for Loopback {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn take(&mut self) -> anyhow::Result<Vec<[i16; 2]>> {
        std::thread::sleep(Duration::from_millis(10));
        let mut out = Vec::new();
        unsafe {
            loop {
                let waiting = self.capture.GetNextPacketSize()?;
                if waiting == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize;
                if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                    out.extend(std::iter::repeat_n([0i16, 0], n));
                } else {
                    let ch = self.channels.max(1);
                    let pick = |i: usize, c: usize| -> f32 {
                        let c = c.min(ch - 1);
                        let at = i * ch + c;
                        match self.kind {
                            Kind::Float32 => *(data as *const f32).add(at),
                            Kind::Int16 => *(data as *const i16).add(at) as f32 / 32768.0,
                            Kind::Int32 => *(data as *const i32).add(at) as f32 / 2147483648.0,
                            Kind::Int24 => {
                                let b = (data as *const u8).add(at * 3);
                                let v = (*b as i32) | ((*b.add(1) as i32) << 8) | ((*b.add(2) as i8 as i32) << 16);
                                v as f32 / 8388608.0
                            }
                        }
                    };
                    let to16 = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                    for i in 0..n {
                        // Left and right; a single channel goes to both.
                        out.push([to16(pick(i, 0)), to16(pick(i, 1))]);
                    }
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(out)
    }
}
