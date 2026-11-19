//! A computer's sound on the TV (Linux, PipeWire). While the session's
//! Sound on TV is on, Kagaz is a sound output of its own, named for the
//! TV and listed beside the speakers, and the system's default output:
//! apps play into it, not into the speakers, so the computer goes quiet
//! and the volume keys set the TV's level as they would any output's.
//! Turned off, the output goes and the default goes back to what it was.
//! If Kagaz ends unexpectedly its output goes with it and the system
//! falls back to the speakers by itself.
//!
//! The sound goes to the TV as Opus, 20 ms a packet, through the same
//! queue as a phone's ([`super::miracast::external::push_sound`]). Silence
//! fills the gaps (sound off, or nothing playing), so the TV's player
//! never waits for sound that is not coming.
//!
//! The default output is set through `pw-metadata`, PipeWire's own tool
//! (with WirePlumber, which every desktop with PipeWire runs).

use super::miracast::external;
use super::pw::VideoStream;
use std::collections::VecDeque;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const RATE: usize = 48_000;
/// 20 ms of stereo sound, in samples (both channels).
const PACKET: usize = RATE / 50 * 2;
/// The output's name in PipeWire.
const NODE: &str = "kagaz-tv";
const KEY: &str = "default.configured.audio.sink";

/// Whether this computer can send its sound: PipeWire and its tool.
pub fn available() -> bool {
    super::pw::loads()
        && Command::new("pw-metadata").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// The sound's side of a session; dropping it puts everything back.
pub struct Sound {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Sound {
    /// Starts following the session's switches for the TV called `tv`.
    pub fn start(tv: &str) -> Sound {
        let stop = Arc::new(AtomicBool::new(false));
        let description = format!("{tv} (Kagaz)");
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("kagaz-tv-sound".into())
            .spawn(move || follow(&description, &s))
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
    }
}

/// The output while it is on, and the default it took over from.
struct Output {
    _stream: VideoStream,
    before: Option<String>,
}

fn follow(description: &str, stop: &AtomicBool) {
    let samples: Arc<Mutex<VecDeque<i16>>> = Arc::default();
    let mut encoder = match opus2::Encoder::new(RATE as u32, opus2::Channels::Stereo, opus2::Application::Audio) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("kagaz: no Opus encoder for the sound: {e}");
            return;
        }
    };
    let _ = encoder.set_bitrate(opus2::Bitrate::Bits(160_000));
    let mut output: Option<Output> = None;
    let start = Instant::now();
    // Samples handed to the encoder so far (both channels).
    let mut made: u64 = 0;
    let mut packet = vec![0u8; 4000];
    let mut frame = vec![0i16; PACKET];
    while !stop.load(Ordering::SeqCst) {
        let want = super::switches::sound_to_tv();
        if want && output.is_none() {
            let q = samples.clone();
            match VideoStream::sound_sink(NODE, description, move |s| {
                let mut q = q.lock().unwrap();
                q.extend(s.iter().copied());
                // More than half a second waiting: nobody is taking it.
                let most = RATE;
                if q.len() > most {
                    let extra = q.len() - most;
                    q.drain(..extra);
                }
            }) {
                Ok(stream) => {
                    // The output has to be there before it can be the default.
                    std::thread::sleep(Duration::from_millis(150));
                    let before = configured();
                    set_configured(Some(&format!("{{ \"name\": \"{NODE}\" }}")));
                    output = Some(Output { _stream: stream, before });
                }
                Err(e) => eprintln!("kagaz: the TV's sound output could not be made: {e}"),
            }
        } else if !want && output.is_some() {
            put_back(output.take());
            samples.lock().unwrap().clear();
        }
        // Packets for the time gone by: what the output took in, and
        // silence where it took in nothing (nothing playing, or the sound
        // on the computer), so the TV hears time pass evenly.
        let due = (start.elapsed().as_micros() as u64 * RATE as u64 / 1_000_000) * 2;
        loop {
            let have = samples.lock().unwrap().len();
            let behind = due.saturating_sub(made);
            if have >= PACKET {
                let mut q = samples.lock().unwrap();
                for (f, s) in frame.iter_mut().zip(q.drain(..PACKET)) {
                    *f = s;
                }
            } else if behind >= PACKET as u64 * 3 {
                // 60 ms behind with nothing come: silence.
                frame.iter_mut().for_each(|s| *s = 0);
            } else {
                break;
            }
            match encoder.encode(&frame, &mut packet) {
                Ok(n) => external::push_sound(packet[..n].to_vec()),
                Err(e) => eprintln!("kagaz: a sound packet could not be made: {e}"),
            }
            made += PACKET as u64;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    put_back(output.take());
}

/// The output goes and the default goes back as it was.
fn put_back(output: Option<Output>) {
    let Some(o) = output else { return };
    // A default left on Kagaz's output by an earlier, cut-short session
    // is no default to go back to.
    let before = o.before.filter(|b| !b.contains(&format!("\"{NODE}\"")));
    set_configured(before.as_deref());
    drop(o._stream);
}

/// The default output the owner chose, as WirePlumber keeps it (JSON).
fn configured() -> Option<String> {
    let out = Command::new("pw-metadata").args(["0", KEY]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains(&format!("key:'{KEY}'")))?;
    let v = line.split("value:'").nth(1)?;
    Some(v[..v.find('\'')?].to_string())
}

fn set_configured(value: Option<&str>) {
    let r = match value {
        Some(v) => Command::new("pw-metadata").args(["0", KEY, v, "Spa:String:JSON"]).output(),
        None => Command::new("pw-metadata").args(["-d", "0", KEY]).output(),
    };
    if let Err(e) = r {
        eprintln!("kagaz: the default sound output could not be set: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kagaz's output on this computer's PipeWire: a quiet tone played
    /// into it by name comes out as samples. The default output is left
    /// alone. Run by hand: `cargo test -p kagaz-core sound -- --ignored`.
    #[test]
    #[ignore]
    fn the_output_takes_sound_in() {
        let got: Arc<Mutex<(usize, i16)>> = Arc::default();
        let g = got.clone();
        let stream = VideoStream::sound_sink("kagaz-tv-test", "Test TV (Kagaz)", move |s| {
            let mut g = g.lock().unwrap();
            g.0 += s.len();
            g.1 = g.1.max(s.iter().map(|v| v.saturating_abs()).max().unwrap_or(0));
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let dir = std::env::temp_dir().join(format!("kagaz-sound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("tone.wav");
        // One second of 440 Hz, about -40 dB.
        let mut data = Vec::new();
        for i in 0..RATE {
            let v = (300.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / RATE as f64).sin()) as i16;
            data.extend_from_slice(&v.to_le_bytes());
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        file.extend_from_slice(b"WAVEfmt ");
        for v in [16u32, 0x0002_0001, RATE as u32, RATE as u32 * 4, 0x0010_0004] {
            file.extend_from_slice(&v.to_le_bytes());
        }
        file.extend_from_slice(b"data");
        file.extend_from_slice(&(data.len() as u32).to_le_bytes());
        file.extend_from_slice(&data);
        std::fs::write(&wav, file).unwrap();
        let played = Command::new("pw-play").args(["--target", "kagaz-tv-test"]).arg(&wav).status().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        drop(stream);
        assert!(played.success());
        let (n, peak) = *got.lock().unwrap();
        assert!(n >= RATE, "only {n} samples came");
        assert!((280..=310).contains(&peak), "the loudest was {peak}");
    }
}
