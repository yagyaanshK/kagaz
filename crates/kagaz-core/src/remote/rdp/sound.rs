//! The computer's sound over Remote Desktop: it arrives in short blocks
//! on a channel of its own and is played here as it comes. The library's
//! player does the playing (through cpal, to this system's own sound) and
//! the decoding where the computer packs its sound (Opus, ADPCM).
//!
//! In the tests nothing is played, on anyone's speakers: the blocks are
//! only counted.

use super::super::session::Numbers;
use ironrdp_rdpsnd::client::RdpsndClientHandler;
use ironrdp_rdpsnd::pdu::{AudioFormat, AudioFormatFlags, PitchPdu, VolumePdu, WaveFormat};
use ironrdp_rdpsnd_native::cpal::RdpsndBackend;
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
pub struct Sound {
    /// What plays it; none where sound is only counted.
    player: Option<RdpsndBackend>,
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
    pub fn new(quiet: Arc<AtomicBool>, numbers: Arc<Numbers>) -> Sound {
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
            player: (!cfg!(test)).then(RdpsndBackend::new),
            plain: vec![pcm(2, 48000), pcm(2, 44100)],
            quiet,
            numbers,
        }
    }
}

impl RdpsndClientHandler for Sound {
    fn get_flags(&self) -> AudioFormatFlags {
        self.player.as_ref().map_or(AudioFormatFlags::empty(), RdpsndClientHandler::get_flags)
    }

    fn get_formats(&self) -> &[AudioFormat] {
        match &self.player {
            Some(player) => player.get_formats(),
            None => &self.plain,
        }
    }

    fn choose_formats(&self, server_formats: &[AudioFormat]) -> Vec<AudioFormat> {
        match &self.player {
            Some(player) => player.choose_formats(server_formats),
            None => self.plain.clone(),
        }
    }

    fn wave(&mut self, format: &AudioFormat, ts: u32, data: Cow<'_, [u8]>) {
        self.numbers.add_sound(data.len());
        let Some(player) = &mut self.player else { return };
        if self.quiet.load(Ordering::Acquire) {
            // Nothing held back to play when the sound is turned on again.
            player.close();
            return;
        }
        player.wave(format, ts, data);
    }

    fn playback_latency(&self) -> Duration {
        self.player.as_ref().map_or(Duration::ZERO, RdpsndClientHandler::playback_latency)
    }

    fn set_volume(&mut self, volume: VolumePdu) {
        if let Some(player) = &mut self.player {
            player.set_volume(volume);
        }
    }

    fn set_pitch(&mut self, pitch: PitchPdu) {
        if let Some(player) = &mut self.player {
            player.set_pitch(pitch);
        }
    }

    fn close(&mut self) {
        if let Some(player) = &mut self.player {
            player.close();
        }
    }
}
