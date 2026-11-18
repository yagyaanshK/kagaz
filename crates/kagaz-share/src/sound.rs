//! This computer's sound, to the viewer: what plays here, taken as it
//! goes to the speakers (they go on playing it; the person here may be
//! listening too), as plain 16-bit stereo in blocks of a fiftieth of a
//! second. The test card's run has a quiet made tone instead, when asked.

use ironrdp_rdpsnd::pdu::{AudioFormat, WaveFormat};
use ironrdp_rdpsnd::server::{NegotiatedFormat, RdpsndError, RdpsndServerHandler, RdpsndServerMessage};
use ironrdp_server::{ServerEvent, ServerEventSender, SoundServerFactory};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

use crate::say;

/// Where sound comes from: frames of interleaved stereo at `rate`.
pub trait Sound: Send {
    fn rate(&self) -> u32;
    /// What has played since the last call, as stereo pairs; empty when
    /// nothing did. Waits a little when there is nothing yet.
    fn take(&mut self) -> anyhow::Result<Vec<[i16; 2]>>;
}

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: 2,
        n_samples_per_sec: rate,
        n_avg_bytes_per_sec: rate * 4,
        n_block_align: 4,
        bits_per_sample: 16,
        data: None,
    }
}

/// The sound channel's side here: offers plain stereo at the two usual
/// rates, and sends while the viewer has it started.
#[derive(Clone)]
pub struct Speaker {
    formats: Vec<AudioFormat>,
    to_viewer: Arc<Mutex<Option<UnboundedSender<ServerEvent>>>>,
    /// The rate agreed with the viewer, 0 while stopped.
    agreed: Arc<AtomicU32>,
    /// The rate this computer's sound plays at, for choosing.
    native: u32,
    /// The device connected may hear it.
    allowed: Arc<std::sync::atomic::AtomicBool>,
}

impl Speaker {
    /// Starts the thread that takes the sound while a viewer listens.
    pub fn new(
        mut open: impl FnMut() -> anyhow::Result<Box<dyn Sound>> + Send + 'static,
        native: u32,
        allowed: Arc<std::sync::atomic::AtomicBool>,
    ) -> Speaker {
        let speaker = Speaker {
            formats: vec![pcm(48000), pcm(44100)],
            to_viewer: Arc::default(),
            agreed: Arc::default(),
            native,
            allowed,
        };
        let me = speaker.clone();
        std::thread::spawn(move || me.run(&mut open));
        speaker
    }

    fn run(&self, open: &mut dyn FnMut() -> anyhow::Result<Box<dyn Sound>>) {
        let began = Instant::now();
        let mut source: Option<Box<dyn Sound>> = None;
        let mut failed = String::new();
        let mut block: Vec<u8> = Vec::new();
        let mut resample = Resample::default();
        // For the log: blocks and how much of them was sent, every so often.
        let (mut blocks, mut since) = (0u32, Instant::now());
        loop {
            let to = self.agreed.load(Ordering::Acquire);
            if to == 0 || !self.allowed.load(Ordering::Acquire) {
                // Nobody listens: let the sound go (it holds the device).
                source = None;
                block.clear();
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            let Some(from) = source.as_mut() else {
                match open() {
                    Ok(s) => {
                        say(&format!("sound from this computer at {} a second", s.rate()));
                        failed.clear();
                        source = Some(s);
                    }
                    Err(e) => {
                        let line = format!("this computer's sound could not be taken: {e:#}");
                        if line != failed {
                            say(&line);
                            failed = line;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                continue;
            };
            let frames = match from.take() {
                Ok(f) => f,
                Err(e) => {
                    let line = format!("the sound was lost for a moment: {e:#}");
                    if line != failed {
                        say(&line);
                        failed = line;
                    }
                    source = None;
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            };
            if frames.is_empty() {
                continue;
            }
            for [l, r] in resample.run(&frames, from.rate(), to) {
                block.extend_from_slice(&l.to_le_bytes());
                block.extend_from_slice(&r.to_le_bytes());
            }
            // A fiftieth of a second a block: small enough to keep the
            // delay down, never near the 64 KB a block may hold.
            let each = (to as usize / 50) * 4;
            while block.len() >= each {
                let rest = block.split_off(each);
                let data = std::mem::replace(&mut block, rest);
                if let Some(sender) = self.to_viewer.lock().unwrap().as_ref() {
                    let _ = sender.send(ServerEvent::Rdpsnd(RdpsndServerMessage::Wave(data, began.elapsed().as_millis() as u32)));
                }
                blocks += 1;
            }
            if since.elapsed() >= Duration::from_secs(10) && blocks > 0 {
                say(&format!("sound: {blocks} blocks in {} s", since.elapsed().as_secs()));
                (blocks, since) = (0, Instant::now());
            }
        }
    }
}

impl ServerEventSender for Speaker {
    fn set_sender(&mut self, sender: UnboundedSender<ServerEvent>) {
        *self.to_viewer.lock().unwrap() = Some(sender);
    }
}

impl SoundServerFactory for Speaker {
    fn build_backend(&self) -> Box<dyn RdpsndServerHandler> {
        Box::new(self.clone())
    }
}

impl std::fmt::Debug for Speaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Speaker")
    }
}

impl RdpsndServerHandler for Speaker {
    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    // This computer's own rate where the viewer takes it: nothing to
    // convert then.
    fn choose_format<'a>(&mut self, common: &'a [NegotiatedFormat]) -> Option<&'a NegotiatedFormat> {
        common.iter().find(|f| f.format().n_samples_per_sec == self.native).or(common.first())
    }

    fn start(&mut self, format: &NegotiatedFormat) -> Result<(), Box<dyn RdpsndError>> {
        let rate = format.format().n_samples_per_sec;
        say(&format!("the viewer takes the sound at {rate} a second"));
        self.agreed.store(rate, Ordering::Release);
        Ok(())
    }

    fn stop(&mut self) {
        self.agreed.store(0, Ordering::Release);
    }
}

/// From one rate to another, in a straight line between neighbours: the
/// usual rates are close, and the ear does not mind.
#[derive(Default)]
struct Resample {
    /// Where the next output falls between the last frame and the next.
    at: f64,
    last: Option<[i16; 2]>,
}

impl Resample {
    fn run(&mut self, frames: &[[i16; 2]], from: u32, to: u32) -> Vec<[i16; 2]> {
        if from == to || from == 0 {
            return frames.to_vec();
        }
        let step = from as f64 / to as f64;
        let mut out = Vec::with_capacity((frames.len() as f64 / step) as usize + 2);
        // Frames with the one before them, so that a block's start joins
        // the last one's end.
        let all: Vec<[i16; 2]> = self.last.into_iter().chain(frames.iter().copied()).collect();
        let mut at = self.at;
        while (at as usize) + 1 < all.len() {
            let i = at as usize;
            let t = at - i as f64;
            let (a, b) = (all[i], all[i + 1]);
            out.push([
                (a[0] as f64 + (b[0] as f64 - a[0] as f64) * t).round() as i16,
                (a[1] as f64 + (b[1] as f64 - a[1] as f64) * t).round() as i16,
            ]);
            at += step;
        }
        self.at = at - (all.len() - 1) as f64;
        self.last = all.last().copied();
        out
    }
}

/// A quiet 440 Hz tone, for the test card's run: no one's sound.
pub struct Tone {
    began: Instant,
    sent: u64,
}

impl Tone {
    pub fn new() -> Tone {
        Tone { began: Instant::now(), sent: 0 }
    }
}

impl Sound for Tone {
    fn rate(&self) -> u32 {
        48000
    }
    fn take(&mut self) -> anyhow::Result<Vec<[i16; 2]>> {
        std::thread::sleep(Duration::from_millis(10));
        let due = self.began.elapsed().as_micros() as u64 * 48 / 1000;
        let mut out = Vec::new();
        while self.sent < due {
            let v = ((self.sent as f64 * 440.0 * std::f64::consts::TAU / 48000.0).sin() * 3000.0) as i16;
            out.push([v, v]);
            self.sent += 1;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_rate_is_left_alone() {
        let mut r = Resample::default();
        let frames = vec![[1, 2], [3, 4]];
        assert_eq!(r.run(&frames, 48000, 48000), frames);
    }

    #[test]
    fn a_second_at_one_rate_is_a_second_at_another() {
        let mut r = Resample::default();
        let mut made = 0;
        // A second of 48000, in blocks of 480, comes out as about 44100.
        for b in 0..100 {
            let block: Vec<[i16; 2]> = (0..480).map(|i| [((b * 480 + i) % 1000) as i16, 0]).collect();
            made += r.run(&block, 48000, 44100).len();
        }
        assert!((44090..=44110).contains(&made), "{made}");
    }

    #[test]
    fn a_slope_stays_a_slope() {
        let mut r = Resample::default();
        let block: Vec<[i16; 2]> = (0..100).map(|i| [i * 10, -i * 10]).collect();
        let out = r.run(&block, 44100, 48000);
        for w in out.windows(2) {
            assert!(w[1][0] >= w[0][0] && w[1][1] <= w[0][1]);
        }
    }
}
