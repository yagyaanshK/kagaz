//! The computer's sound over Remote Desktop: it arrives in blocks on a
//! channel of its own and is played here as it comes, by Kagaz's own
//! player (play.rs), which also decodes it where the computer packs its
//! sound (Opus, ADPCM).
//!
//! In the tests nothing is played, on anyone's speakers: the blocks are
//! only counted.

use super::super::session::Numbers;
use super::play::Player;
use ironrdp_rdpsnd::client::RdpsndClientHandler;
use ironrdp_rdpsnd::pdu::{AudioFormat, AudioFormatFlags, PitchPdu, VolumePdu, WaveFormat};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Sound {
    /// Which of Remote Desktop's sound channels this one serves, for the
    /// log: a computer uses one or the other.
    channel: &'static str,
    /// What the two agreed on, for the log.
    agreed: String,
    blocks: u64,
    /// The link is fast: only sound as it is (plain, stereo) is taken,
    /// where the computer has it so. Left the choice, Windows goes by a
    /// guess at the link and often sends its thinnest (one channel,
    /// packed small) on a home network that carries the full sound with
    /// ease.
    fast: bool,
    /// Since the log was last told: the longest the sound was ahead of the
    /// speakers (how late it plays) and the longest block, in milliseconds.
    ahead: u64,
    system: u64,
    longest: u64,
    told: Option<Instant>,
    /// What plays it; none where sound is only counted.
    player: Option<Player>,
    /// What is said to be playable where there is no player to ask.
    plain: Vec<AudioFormat>,
    /// The person has turned the sound off here for now: blocks still
    /// come (the computer goes on sending) and are let go.
    quiet: Arc<AtomicBool>,
    numbers: Arc<Numbers>,
}

impl Sound {
    /// `quiet` is shared with the session, which sets it when the person
    /// turns the sound off or on again.
    pub fn new(channel: &'static str, fast: bool, quiet: Arc<AtomicBool>, numbers: Arc<Numbers>) -> Sound {
        let pcm = |channels: u16, rate: u32| AudioFormat {
            format: WaveFormat::PCM,
            n_channels: channels,
            n_samples_per_sec: rate,
            n_avg_bytes_per_sec: rate * u32::from(channels) * 2,
            n_block_align: channels * 2,
            bits_per_sample: 16,
            data: None,
        };
        Sound {
            channel,
            agreed: String::new(),
            blocks: 0,
            fast,
            ahead: 0,
            system: 0,
            longest: 0,
            told: None,
            player: (!cfg!(test)).then(Player::new),
            plain: vec![pcm(2, 48000), pcm(2, 44100)],
            quiet,
            numbers,
        }
    }
}

impl RdpsndClientHandler for Sound {
    fn get_flags(&self) -> AudioFormatFlags {
        // How loud the computer says its sound is, is kept to here.
        if self.player.is_some() {
            AudioFormatFlags::VOLUME
        } else {
            AudioFormatFlags::empty()
        }
    }

    fn get_formats(&self) -> &[AudioFormat] {
        &self.plain
    }

    fn choose_formats(&self, server_formats: &[AudioFormat]) -> Vec<AudioFormat> {
        // Windows lists its ways from the one it likes best and picks
        // among those sent back, so they go back in its order.
        let mut chosen = match &self.player {
            Some(_) => Player::playable(server_formats),
            None => self.plain.clone(),
        };
        if self.fast {
            let full = |f: &AudioFormat| {
                f.format == WaveFormat::PCM && f.n_channels >= 2 && f.n_samples_per_sec >= 44100 && f.bits_per_sample >= 16
            };
            if chosen.iter().any(|f| full(f) && server_formats.contains(f)) {
                chosen.retain(full);
            }
        }
        // For the log: how far the two got in agreeing.
        self.numbers.set_sound_via(format!(
            "{}, {} ways offered, {} of them playable here",
            self.channel,
            server_formats.len(),
            chosen.iter().filter(|c| server_formats.contains(c)).count()
        ));
        chosen
    }

    fn wave(&mut self, format: &AudioFormat, _ts: u32, data: Cow<'_, [u8]>) {
        self.numbers.add_sound(data.len());
        self.blocks += 1;
        {
            if self.blocks == 1 {
                let form = match format.format {
                    WaveFormat::PCM => "PCM".to_string(),
                    WaveFormat::ADPCM => "ADPCM".to_string(),
                    WaveFormat::DVI_ADPCM => "IMA ADPCM".to_string(),
                    WaveFormat::ALAW => "A-law".to_string(),
                    WaveFormat::MULAW => "mu-law".to_string(),
                    WaveFormat::OPUS => "Opus".to_string(),
                    other => format!("{other:?}"),
                };
                self.agreed = format!("{form} at {} a second, {} channels", format.n_samples_per_sec, format.n_channels);
            }
        }
        let long = match &mut self.player {
            Some(player) if self.quiet.load(Ordering::Acquire) => {
                // Nothing held back to play when the sound is turned on again.
                player.close();
                0
            }
            Some(player) => player.wave(format, &data),
            None => 0,
        };
        self.longest = self.longest.max(long);
        let doing = self.player.as_ref().map(Player::doing).unwrap_or_default();
        self.ahead = self.ahead.max(doing.ahead);
        self.system = self.system.max(doing.system);
        // For the log, every couple of seconds: how the sound comes and
        // how well it plays.
        if self.told.is_none_or(|at| at.elapsed() >= Duration::from_secs(2)) {
            self.told = Some(Instant::now());
            let mut said = format!("{}, {}, {} blocks", self.channel, self.agreed, self.blocks);
            if self.player.is_some() {
                said += &format!(
                    " of up to {} ms, {} gaps, {} ms kept ready, up to {} ms ahead of the speakers ({} of them in the system's sound)",
                    self.longest, doing.gaps, doing.reserve, self.ahead, self.system
                );
                if doing.thrown > 0 {
                    said += &format!(", {} ms let go after stalls", doing.thrown);
                }
                if doing.silent {
                    said += ", no way to the speakers found";
                }
            }
            self.numbers.set_sound_via(said);
            self.ahead = 0;
            self.system = 0;
            self.longest = 0;
        }
    }

    fn playback_latency(&self) -> Duration {
        self.player.as_ref().map_or(Duration::ZERO, Player::latency)
    }

    fn set_volume(&mut self, volume: VolumePdu) {
        if let Some(player) = &mut self.player {
            player.set_volume(volume.volume_left, volume.volume_right);
        }
    }

    fn set_pitch(&mut self, _pitch: PitchPdu) {}

    fn close(&mut self) {
        if let Some(player) = &mut self.player {
            player.close();
        }
    }
}

/// The sound's older channel for a session whose sound stays on the
/// computer: there only because Windows offers the drives' channel
/// beside it, it takes no form of sound, so none can be sent.
#[derive(Debug)]
pub struct Silent;

impl RdpsndClientHandler for Silent {
    fn get_formats(&self) -> &[AudioFormat] {
        &[]
    }

    fn choose_formats(&self, _server_formats: &[AudioFormat]) -> Vec<AudioFormat> {
        Vec::new()
    }

    fn wave(&mut self, _format: &AudioFormat, _ts: u32, _data: Cow<'_, [u8]>) {}

    fn set_volume(&mut self, _volume: VolumePdu) {}

    fn set_pitch(&mut self, _pitch: PitchPdu) {}

    fn close(&mut self) {}
}
