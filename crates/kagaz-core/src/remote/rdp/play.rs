//! Playing a computer's sound here, as soon as it can be played without
//! gaps.
//!
//! The sound arrives in blocks, on the computer's clock and by the
//! network's leave; the speakers take it steadily, on their own clock. In
//! between a little is kept ready. How little is the whole matter: every
//! millisecond kept is a millisecond the sound plays late, and too few
//! leave a gap whenever a block is late.
//!
//! What counts is the *lowest* the store falls, just before a block
//! arrives, not how full it is on the whole: a computer that sends half a
//! second at a time fills it by half a second each time, and that half
//! second is no reserve at all. So [`Playout`] watches the lowest level
//! over a few seconds and steers that to a small reserve, by playing one
//! part in a hundred faster or slower (not heard). The reserve doubles
//! when the sound did run out, and comes back down after a calm minute:
//! on a steady link the sound plays as late as the link makes it and
//! little more.
//!
//! [`Player`] is the rest: decoding what the computer packed, the ring
//! the blocks wait in, and the system's own sound (through cpal; on Android
//! AAudio, play_android.rs) taking from it on a thread of its own.

use ironrdp_rdpsnd::codec::AudioDecoder;
use ironrdp_rdpsnd::pdu::AudioFormat;
#[cfg(not(windows))]
use ironrdp_rdpsnd::pdu::WaveFormat;
use ironrdp_rdpsnd::playout::stretch;
use ringbuf::traits::{Consumer as _, Observer as _, Producer as _, Split as _};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(not(target_os = "android"))]
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// What the speakers' turn is filled with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    /// Nothing: no sound yet, or the reserve is being kept back.
    Silence,
    /// Let `skip` frames go unplayed, then play `take` frames over the
    /// turn (a few more or fewer than it holds, fitted to it).
    Play { skip: usize, take: usize },
    /// The sound ran out: the `take` frames left, then nothing.
    Last { take: usize },
}

/// The reserve to begin with, and the least it comes down to.
const LEAST: Duration = Duration::from_millis(60);
/// The most the reserve grows to on a link that keeps stalling.
const MOST: Duration = Duration::from_millis(480);
/// How long the lowest level is watched before anything is done about it.
const WATCH: Duration = Duration::from_secs(3);
/// More than this above the reserve at its lowest is a pile-up (the link
/// stood still and then delivered all at once): let go at once, where less
/// is played off unheard.
const PILE: Duration = Duration::from_millis(250);
/// Silence shorter than this in the middle of sound is a gap; longer, the
/// sound had ended and another began.
const ENDED: Duration = Duration::from_secs(1);
/// After this long without a gap the reserve comes down a step.
const CALM: Duration = Duration::from_secs(60);

/// When to play how much: arithmetic on counts of frames only, so that it
/// can be tried against any pattern of arrival without a speaker.
#[derive(Debug, Clone)]
pub struct Playout {
    rate: usize,
    least: usize,
    most: usize,
    /// What the lowest level is steered to.
    reserve: usize,
    playing: bool,
    /// While not playing: how long sound has been there, kept back.
    waited: usize,
    /// How long ago the sound ran out, until what comes next is judged.
    dry: Option<usize>,
    /// The lowest level since the watch began, and how long that is.
    low: usize,
    seen: usize,
    /// Frames still to play off (above zero) or to win back (below).
    owed: isize,
    /// How long since the last gap.
    calm: usize,
    gaps: u64,
    thrown: u64,
}

impl Playout {
    pub fn new(rate: u32) -> Playout {
        let rate = rate.max(1) as usize;
        let frames = |d: Duration| (d.as_millis() as usize * rate / 1000).max(1);
        Playout {
            rate,
            least: frames(LEAST),
            most: frames(MOST),
            reserve: frames(LEAST),
            playing: false,
            waited: 0,
            dry: None,
            low: usize::MAX,
            seen: 0,
            owed: 0,
            calm: 0,
            gaps: 0,
            thrown: 0,
        }
    }

    fn frames(&self, d: Duration) -> usize {
        d.as_millis() as usize * self.rate / 1000
    }

    /// Times the sound ran out in the middle of playing.
    pub fn gaps(&self) -> u64 {
        self.gaps
    }
    /// Frames let go unplayed after pile-ups.
    pub fn thrown(&self) -> u64 {
        self.thrown
    }
    /// The reserve now, in frames.
    pub fn reserve(&self) -> usize {
        self.reserve
    }

    /// What to do with a turn of `asked` frames when `buffered` are ready.
    /// Never more than `buffered` in all.
    pub fn next(&mut self, buffered: usize, asked: usize) -> Period {
        if !self.playing {
            if buffered == 0 {
                if let Some(dry) = &mut self.dry {
                    *dry = dry.saturating_add(asked);
                }
                self.waited = 0;
                return Period::Silence;
            }
            if let Some(dry) = self.dry.take() {
                // Sound again so soon: it had not ended, a block was late.
                if dry < self.frames(ENDED) {
                    self.gaps += 1;
                    self.reserve = (self.reserve * 2).min(self.most);
                    self.calm = 0;
                }
            }
            // The reserve is kept back by waiting, not by counting what is
            // ready: one long block is ready at once and is no reserve.
            if self.waited < self.reserve {
                self.waited += asked;
                return Period::Silence;
            }
            self.playing = true;
            self.waited = 0;
            self.low = usize::MAX;
            self.seen = 0;
            self.owed = 0;
        }
        if buffered < asked {
            self.playing = false;
            self.dry = Some(0);
            return Period::Last { take: buffered };
        }

        let after = buffered - asked;
        self.low = self.low.min(after);
        self.seen += asked;
        self.calm = self.calm.saturating_add(asked);
        if self.seen >= self.frames(WATCH) {
            if self.calm >= self.frames(CALM) && self.reserve > self.least {
                self.reserve = (self.reserve * 3 / 4).max(self.least);
                self.calm = 0;
            }
            let low = self.low;
            self.low = usize::MAX;
            self.seen = 0;
            let slack = (self.reserve / 2).max(self.rate / 100);
            if low > self.reserve + self.frames(PILE) {
                let skip = (low - self.reserve).min(after);
                self.thrown += skip as u64;
                self.owed = 0;
                return Period::Play { skip, take: asked };
            }
            self.owed = if low > self.reserve + slack {
                (low - self.reserve) as isize
            } else if low < self.reserve / 2 {
                -((self.reserve - low) as isize)
            } else {
                0
            };
        }
        // One part in a hundred faster or slower.
        let step = (asked / 100).max(1);
        let take = if self.owed > 0 {
            let more = step.min(self.owed as usize).min(after);
            self.owed -= more as isize;
            asked + more
        } else if self.owed < 0 && step < asked {
            let less = step.min(self.owed.unsigned_abs());
            self.owed += less as isize;
            asked - less
        } else {
            asked
        };
        Period::Play { skip: 0, take }
    }
}

/// How the player is doing, for the log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Doing {
    pub gaps: u64,
    /// Milliseconds let go unplayed after pile-ups.
    pub thrown: u64,
    /// The reserve, in milliseconds.
    pub reserve: u64,
    /// How far ahead of the speakers the sound is held, in milliseconds:
    /// what waits here and what the system's own sound holds.
    pub ahead: u64,
    /// Of those, what the system's own sound holds.
    pub system: u64,
    /// No way to the speakers was found (yet).
    pub silent: bool,
}

/// The sound a wave block holds, as 16-bit samples.
enum Decoder {
    Plain(AudioDecoder),
    #[cfg(not(windows))]
    Opus(opus2::Decoder, usize),
}

impl Decoder {
    fn new(format: &AudioFormat) -> Option<Decoder> {
        if !Decoder::can(format) {
            return None;
        }
        match format.format {
            #[cfg(not(windows))]
            WaveFormat::OPUS => {
                let channels = if format.n_channels == 1 { opus2::Channels::Mono } else { opus2::Channels::Stereo };
                opus2::Decoder::new(format.n_samples_per_sec, channels).ok().map(|d| Decoder::Opus(d, usize::from(format.n_channels)))
            }
            _ => AudioDecoder::new(format).ok().map(Decoder::Plain),
        }
    }

    /// One or two channels, at a rate the system's sound takes.
    fn can(format: &AudioFormat) -> bool {
        if !(1..=2).contains(&format.n_channels) || !(8000..=48000).contains(&format.n_samples_per_sec) {
            return false;
        }
        match format.format {
            #[cfg(not(windows))]
            WaveFormat::OPUS => matches!(format.n_samples_per_sec, 8000 | 12000 | 16000 | 24000 | 48000),
            _ => AudioDecoder::supports(format),
        }
    }

    fn decode(&mut self, data: &[u8], out: &mut Vec<i16>) {
        match self {
            // What decoded before a fault is kept; the rest of the block is lost.
            Decoder::Plain(decoder) => {
                let _ = decoder.decode(data, out);
            }
            #[cfg(not(windows))]
            Decoder::Opus(decoder, channels) => {
                let Ok(frames) = decoder.get_nb_samples(data) else { return };
                let start = out.len();
                out.resize(start + frames * *channels, 0);
                match decoder.decode(data, &mut out[start..], false) {
                    Ok(written) => out.truncate(start + written * *channels),
                    Err(_) => out.truncate(start),
                }
            }
        }
    }
}

/// Shared between the player, the thread that holds the way to the
/// speakers, and the system's own sound thread that fills each turn.
#[derive(Debug, Default)]
struct Shared {
    ended: AtomicBool,
    lost: AtomicBool,
    silent: AtomicBool,
    gaps: AtomicU64,
    thrown: AtomicU64,
    reserve: AtomicU64,
    /// From a turn being filled to its sound leaving the speakers.
    device_us: AtomicU64,
}

/// What fills each of the speakers' turns. On the system's sound thread:
/// nothing here waits, and nothing is set aside but on a turn longer than
/// any before.
struct Fill {
    consumer: HeapCons<i16>,
    playout: Playout,
    channels: usize,
    scratch: Vec<i16>,
    /// The last turn ended in silence: this one comes in softly.
    quiet: bool,
    shared: Arc<Shared>,
}

/// Frames over which sound comes in or goes out, so that a gap does not
/// click as well.
const SOFTEN: usize = 96;

impl Fill {
    fn turn(&mut self, out: &mut [i16]) {
        let channels = self.channels;
        let asked = out.len() / channels;
        let buffered = self.consumer.occupied_len() / channels;
        match self.playout.next(buffered, asked) {
            Period::Silence => {
                out.fill(0);
                self.quiet = true;
            }
            Period::Play { skip, take } => {
                if skip != 0 {
                    self.consumer.skip(skip * channels);
                }
                self.scratch.resize(take * channels, 0);
                let got = self.consumer.pop_slice(&mut self.scratch);
                self.scratch.truncate(got);
                stretch(&self.scratch, channels, out);
                if self.quiet {
                    let over = asked.min(SOFTEN);
                    for (i, frame) in out.chunks_mut(channels).take(over).enumerate() {
                        for s in frame {
                            *s = (i32::from(*s) * i as i32 / over as i32) as i16;
                        }
                    }
                    self.quiet = false;
                }
            }
            Period::Last { take } => {
                let (played, rest) = out.split_at_mut(take * channels);
                let got = self.consumer.pop_slice(played);
                played[got..].fill(0);
                rest.fill(0);
                let over = take.min(SOFTEN);
                for (i, frame) in played.chunks_mut(channels).rev().take(over).enumerate() {
                    for s in frame {
                        *s = (i32::from(*s) * i as i32 / over as i32) as i16;
                    }
                }
                self.quiet = true;
            }
        }
        self.shared.gaps.store(self.playout.gaps(), Ordering::Relaxed);
        self.shared.thrown.store(self.playout.thrown(), Ordering::Relaxed);
        self.shared.reserve.store(self.playout.reserve() as u64, Ordering::Relaxed);
    }
}

struct Active {
    format: AudioFormat,
    decoder: Decoder,
    producer: HeapProd<i16>,
    channels: usize,
    rate: u32,
    shared: Arc<Shared>,
    thread: JoinHandle<()>,
    decoded: Vec<i16>,
}

/// The ring holds this much sound: longer than any block a computer sends.
const RING: Duration = Duration::from_secs(4);

/// Plays what a computer sends, in whatever form the two agreed on.
pub struct Player {
    stream: Option<Active>,
    /// Left and right, each out of 0xFFFF, as the computer last set them.
    volume: Arc<AtomicU32>,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player").field("playing", &self.stream.is_some()).finish()
    }
}

impl Default for Player {
    fn default() -> Self {
        Player::new()
    }
}

impl Player {
    pub fn new() -> Player {
        Player { stream: None, volume: Arc::new(AtomicU32::new(u32::MAX)) }
    }

    /// Of the forms a computer offers, those that can be played here, in
    /// its order.
    pub fn playable(offered: &[AudioFormat]) -> Vec<AudioFormat> {
        offered.iter().filter(|f| Decoder::can(f)).cloned().collect()
    }

    fn open(&mut self, format: &AudioFormat) -> bool {
        let Some(decoder) = Decoder::new(format) else { return false };
        let channels = usize::from(format.n_channels);
        let rate = format.n_samples_per_sec;
        let room = (RING.as_secs() as usize) * rate as usize * channels;
        let (producer, consumer) = HeapRb::<i16>::new(room).split();
        let shared = Arc::new(Shared::default());
        let fill = Fill {
            consumer,
            playout: Playout::new(rate),
            channels,
            // A quarter of a second covers any turn in practice.
            scratch: Vec::with_capacity(rate as usize / 4 * channels),
            quiet: true,
            shared: shared.clone(),
        };
        // Short turns (a fiftieth of a second): what the system's sound
        // holds of its own plays late too, and left to itself it holds a
        // tenth of a second and more.
        #[cfg(not(target_os = "android"))]
        let config =
            cpal::StreamConfig { channels: format.n_channels, sample_rate: rate, buffer_size: cpal::BufferSize::Fixed(rate / 50) };
        #[cfg(target_os = "android")]
        let config = android::OutConfig { channels: format.n_channels, sample_rate: rate };
        let thread = {
            let shared = shared.clone();
            let volume = self.volume.clone();
            thread::Builder::new().name("kagaz-remote-sound".into()).spawn(move || speakers(&config, fill, &shared, &volume))
        };
        let Ok(thread) = thread else { return false };
        self.stream = Some(Active { format: format.clone(), decoder, producer, channels, rate, shared, thread, decoded: Vec::new() });
        true
    }

    /// One block of sound. Says how long it is, in milliseconds.
    pub fn wave(&mut self, format: &AudioFormat, data: &[u8]) -> u64 {
        if self.stream.as_ref().is_some_and(|s| s.format != *format) {
            self.close();
        }
        if self.stream.is_none() && !self.open(format) {
            return 0;
        }
        let Some(stream) = self.stream.as_mut() else { return 0 };
        stream.decoded.clear();
        stream.decoder.decode(data, &mut stream.decoded);
        // A ring already full holds seconds of sound nobody is taking
        // (no speakers): what does not fit is let go.
        stream.producer.push_slice(&stream.decoded);
        (stream.decoded.len() / stream.channels.max(1)) as u64 * 1000 / u64::from(stream.rate.max(1))
    }

    /// How far ahead of the speakers the newest sound is.
    pub fn latency(&self) -> Duration {
        let Some(stream) = &self.stream else { return Duration::ZERO };
        let frames = (stream.producer.occupied_len() / stream.channels.max(1)) as u64;
        Duration::from_micros(frames * 1_000_000 / u64::from(stream.rate.max(1)))
            + Duration::from_micros(stream.shared.device_us.load(Ordering::Relaxed))
    }

    pub fn doing(&self) -> Doing {
        let Some(stream) = &self.stream else { return Doing::default() };
        let ms = |frames: u64| frames * 1000 / u64::from(stream.rate.max(1));
        Doing {
            gaps: stream.shared.gaps.load(Ordering::Relaxed),
            thrown: ms(stream.shared.thrown.load(Ordering::Relaxed)),
            reserve: ms(stream.shared.reserve.load(Ordering::Relaxed)),
            ahead: self.latency().as_millis() as u64,
            system: stream.shared.device_us.load(Ordering::Relaxed) / 1000,
            silent: stream.shared.silent.load(Ordering::Relaxed),
        }
    }

    pub fn set_volume(&mut self, left: u16, right: u16) {
        self.volume.store((u32::from(left) << 16) | u32::from(right), Ordering::Relaxed);
    }

    /// Stop playing; what was waiting is let go.
    pub fn close(&mut self) {
        if let Some(stream) = self.stream.take() {
            stream.shared.ended.store(true, Ordering::Relaxed);
            stream.thread.thread().unpark();
            let _ = stream.thread.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.close();
    }
}

// On Android, AAudio looked up when needed (Android 7 has none).
#[cfg(target_os = "android")]
#[path = "play_android.rs"]
mod android;
#[cfg(target_os = "android")]
use android::speakers;

/// Holds the way to the speakers for as long as the sound lasts (it cannot
/// be handed between threads), and finds it again when it goes: earphones
/// pulled out, the system's sound restarted.
#[cfg(not(target_os = "android"))]
fn speakers(config: &cpal::StreamConfig, fill: Fill, shared: &Arc<Shared>, volume: &Arc<AtomicU32>) {
    use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
    let fill = Arc::new(Mutex::new(fill));
    let me = thread::current();
    // Where the system's sound will not take turns that short, its own.
    let roomy = cpal::StreamConfig { buffer_size: cpal::BufferSize::Default, ..config.clone() };
    while !shared.ended.load(Ordering::Relaxed) {
        shared.lost.store(false, Ordering::Relaxed);
        let open = |config: &cpal::StreamConfig| cpal::default_host().default_output_device().and_then(|device| {
            let (fill, shared, volume) = (fill.clone(), shared.clone(), volume.clone());
            let (lost, me) = (shared.clone(), me.clone());
            let channels = usize::from(config.channels);
            let stream = device
                .build_output_stream_raw(
                    config,
                    cpal::SampleFormat::I16,
                    move |data, info: &cpal::OutputCallbackInfo| {
                        let Some(out) = data.as_slice_mut::<i16>() else { return };
                        let Ok(mut fill) = fill.try_lock() else {
                            out.fill(0);
                            return;
                        };
                        fill.turn(out);
                        let both = volume.load(Ordering::Relaxed);
                        let (left, right) = ((both >> 16) as i32, (both & 0xFFFF) as i32);
                        if left != 0xFFFF || right != 0xFFFF {
                            for frame in out.chunks_mut(channels) {
                                for (i, s) in frame.iter_mut().enumerate() {
                                    *s = (i32::from(*s) * if i == 1 { right } else { left } / 0xFFFF) as i16;
                                }
                            }
                        }
                        let stamp = info.timestamp();
                        if let Some(ahead) = stamp.playback.duration_since(&stamp.callback) {
                            shared.device_us.store(ahead.as_micros() as u64, Ordering::Relaxed);
                        }
                    },
                    move |error| {
                        if matches!(error, cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated) {
                            lost.lost.store(true, Ordering::Relaxed);
                            me.unpark();
                        }
                    },
                    None,
                )
                .ok()?;
            stream.play().ok()?;
            Some(stream)
        });
        let stream = open(config).or_else(|| open(&roomy));
        shared.silent.store(stream.is_none(), Ordering::Relaxed);
        if stream.is_none() {
            thread::park_timeout(Duration::from_secs(1));
            continue;
        }
        while !shared.ended.load(Ordering::Relaxed) && !shared.lost.load(Ordering::Relaxed) {
            thread::park_timeout(Duration::from_millis(500));
        }
        drop(stream);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u64 = 44100;
    /// What the speakers ask for at a time, in frames.
    const TURN: u64 = 1024;

    struct Heard {
        /// Gaps in all, and those after the first twenty seconds.
        gaps: u64,
        late_gaps: u64,
        thrown_ms: u64,
        /// The most held ready at the lowest point between two blocks,
        /// over the last twenty seconds: how late the sound plays.
        low_ms: u64,
    }

    /// Sound in blocks of `block_ms`, each up to `late_ms` late (the same
    /// pattern every time, no dice), for `seconds`; `stall` is a moment
    /// the link stands still, as (when, for how long) in milliseconds.
    fn listen(block_ms: u64, late_ms: u64, seconds: u64, stall: Option<(u64, u64)>) -> Heard {
        let mut playout = Playout::new(RATE as u32);
        let block = block_ms * RATE / 1000;
        let (mut buffered, mut now, mut n) = (0u64, 0u64, 0u64);
        let mut seed = 12345u64;
        // The first block arrives once the computer has that much to send.
        let mut due = block;
        let mut heard = Heard { gaps: 0, late_gaps: 0, thrown_ms: 0, low_ms: 0 };
        let mut low = u64::MAX;
        let mut held_back = 0u64;
        while now < seconds * RATE {
            if let Some((at, long)) = stall {
                let (from, to) = (at * RATE / 1000, (at + long) * RATE / 1000);
                if now >= from && now < to {
                    held_back = to;
                }
            }
            while now >= due.max(held_back) {
                buffered += block;
                n += 1;
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let late = if late_ms == 0 { 0 } else { (seed >> 33) % (late_ms * RATE / 1000 + 1) };
                due = (n + 1) * block + late;
            }
            let before = playout.gaps();
            match playout.next(buffered as usize, TURN as usize) {
                Period::Silence => {}
                Period::Play { skip, take } => {
                    assert!((skip + take) as u64 <= buffered);
                    buffered -= (skip + take) as u64;
                    if now >= (seconds - 20) * RATE {
                        low = low.min(buffered);
                    }
                }
                Period::Last { take } => {
                    assert!(take as u64 <= buffered);
                    buffered -= take as u64;
                }
            }
            if playout.gaps() > before && now >= 20 * RATE {
                heard.late_gaps += 1;
            }
            now += TURN;
        }
        heard.gaps = playout.gaps();
        heard.thrown_ms = playout.thrown() * 1000 / RATE;
        heard.low_ms = if low == u64::MAX { 0 } else { low * 1000 / RATE };
        heard
    }

    #[test]
    fn short_and_long_blocks_play_without_gaps() {
        // What made the library's player stutter: blocks of a quarter of
        // a second and more, as Windows sends some forms of sound.
        for (block, late) in [(20, 5), (46, 10), (100, 10), (250, 10), (500, 10), (500, 30), (1000, 40)] {
            let heard = listen(block, late, 120, None);
            assert_eq!(heard.gaps, 0, "blocks of {block} ms, up to {late} ms late");
            assert_eq!(heard.thrown_ms, 0, "blocks of {block} ms: nothing is let go");
            // And no later than need be: the reserve and little more.
            assert!(heard.low_ms <= 110, "blocks of {block} ms play {} ms late", heard.low_ms);
        }
    }

    #[test]
    fn a_jumpy_link_costs_a_few_gaps_then_none() {
        // Blocks up to 150 ms late: more than the reserve to begin with.
        let heard = listen(100, 150, 120, None);
        assert!(heard.gaps >= 1 && heard.gaps <= 4, "{} gaps", heard.gaps);
        assert_eq!(heard.late_gaps, 0, "the reserve grew to cover it");
        assert!(heard.low_ms <= 300, "{} ms late", heard.low_ms);
    }

    #[test]
    fn what_piles_up_in_a_stall_is_let_go() {
        // The link stands still for 0.7 s, half a minute in, and then
        // delivers all of it at once.
        let heard = listen(20, 5, 90, Some((30_000, 700)));
        assert_eq!(heard.gaps, 1);
        assert!(heard.thrown_ms >= 300, "{} ms let go", heard.thrown_ms);
        assert!(heard.low_ms <= 200, "{} ms late afterwards", heard.low_ms);
    }

    #[test]
    fn a_sound_that_ends_is_not_a_gap() {
        let mut playout = Playout::new(RATE as u32);
        let turn = TURN as usize;
        // A second of sound, three seconds of nothing, a second of sound.
        let mut buffered = RATE as usize;
        for _ in 0..(5 * RATE as usize / turn) {
            match playout.next(buffered, turn) {
                Period::Silence => {}
                Period::Play { skip, take } => buffered -= skip + take,
                Period::Last { take } => buffered -= take,
            }
            if buffered == 0 && playout.dry.is_some_and(|dry| dry > 3 * RATE as usize) {
                buffered = RATE as usize;
            }
        }
        assert_eq!(playout.gaps(), 0);
        assert_eq!(playout.reserve(), playout.least);
    }

    #[test]
    fn a_short_sound_is_played_whole() {
        // 30 ms: less than the reserve, and nothing follows it.
        let mut playout = Playout::new(RATE as u32);
        let turn = TURN as usize;
        let mut buffered = (30 * RATE / 1000) as usize;
        let whole = buffered;
        let mut played = 0;
        for _ in 0..20 {
            match playout.next(buffered, turn) {
                Period::Silence => {}
                Period::Play { skip, take } => {
                    buffered -= skip + take;
                    played += take;
                }
                Period::Last { take } => {
                    buffered -= take;
                    played += take;
                }
            }
        }
        assert_eq!(played, whole);
    }
}
