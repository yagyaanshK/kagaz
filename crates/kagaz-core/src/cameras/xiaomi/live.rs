//! A Xiaomi camera's live view, spoken by Kagaz itself: keys from the
//! cloud for this connection, then CS2 and miss straight to the camera on
//! this network, turned into the pictures and sound Kagaz's own player
//! takes ([`crate::media::Media`]), on every platform alike.

use super::api::Camera;
use super::{cs2, miss, Session};
use crate::intercom::g711::Law;
use crate::media::{nal_units, Media, START};
use std::time::Duration;

/// Picture sizes the camera can be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// The camera picks as the network allows.
    Auto,
    Low,
    High,
}

impl Quality {
    fn code(self) -> u8 {
        match self {
            Quality::Auto => 0,
            Quality::Low => 1,
            Quality::High => 2,
        }
    }
}

pub struct Live {
    client: miss::Client,
    /// The camera sends H.265, which the player cannot show; said once.
    told_h265: bool,
    /// Pictures before the first keyframe cannot be shown; they are held back.
    seen_key: bool,
}

/// Open the camera's live view. `session` may have its token renewed on
/// the way; the caller saves it.
pub fn open(session: &mut Session, camera: &Camera, quality: Quality, audio: bool) -> Result<Live, String> {
    let cloud = session.connect().map_err(|e| e.to_string())?;
    let keys = miss::Keys::new();
    let k = cloud
        .miss_keys(&camera.region, &camera.did, &keys.public_hex())
        .map_err(|e| e.to_string())?;
    if k.vendor != 4 {
        let name = match k.vendor {
            1 => "TUTK",
            3 => "Agora",
            6 => "MTP",
            _ => "an unknown service",
        };
        return Err(format!("this camera connects through {name}, which Kagaz does not speak yet (only CS2)"));
    }
    let ip = camera
        .ip
        .parse()
        .map_err(|_| "the camera's address on this network is not known".to_string())?;
    let conn = cs2::Conn::dial(ip)?;
    let mut client = miss::Client::login(conn, &keys, &k.device_public, &k.sign, &camera.model)?;
    client.start(quality.code(), audio)?;
    Ok(Live {
        client,
        told_h265: false,
        seen_key: false,
    })
}

/// Open the camera and play its SD card from `start` to `end` (unix
/// seconds) instead of the live view.
pub fn open_playback(session: &mut Session, camera: &Camera, start: i64, end: i64) -> Result<Live, String> {
    let mut view = open(session, camera, Quality::High, true)?;
    view.client.stop()?;
    view.client.playback(start, end, 1)?;
    view.seen_key = false;
    Ok(view)
}

impl Live {
    pub fn protocol(&self) -> &'static str {
        self.client.protocol()
    }

    /// Stop the pictures (live or played back).
    pub fn stop(&mut self) -> Result<(), String> {
        self.client.stop()
    }

    /// The card's file holding moment `at`, if it has one (see [`super::card`]).
    pub fn footage_at(&mut self, at: i64, wait: Duration) -> Result<Option<(i64, i64)>, String> {
        self.client.footage_at(at, wait)
    }

    /// What the camera answered to a request, if it has (within `wait`).
    pub fn answer(&self, wait: Duration) -> Option<(u32, String)> {
        self.client
            .read_answer(wait)
            .ok()
            .map(|(cmd, data)| (cmd, String::from_utf8_lossy(&data).into_owned()))
    }

    /// The next pieces for the player (one camera packet's worth).
    pub fn next(&mut self) -> Result<Vec<Media>, String> {
        let p = self.client.read(Duration::from_secs(10))?;
        let timestamp = (p.timestamp.wrapping_mul(90)) as u32;
        Ok(match p.codec {
            miss::CODEC_H264 => {
                let units = nal_units(&p.payload);
                let key = units.iter().any(|u| u.first().map(|b| b & 0x1f) == Some(5));
                self.seen_key |= key;
                if !self.seen_key {
                    return Ok(Vec::new());
                }
                let mut data = Vec::with_capacity(p.payload.len() + 8);
                for u in units {
                    data.extend_from_slice(&START);
                    data.extend_from_slice(u);
                }
                vec![Media::Video { data, key, timestamp }]
            }
            miss::CODEC_PCMA => vec![Media::Audio {
                pcm: Law::A.decode(&p.payload),
                timestamp,
            }],
            miss::CODEC_PCMU => vec![Media::Audio {
                pcm: Law::Mu.decode(&p.payload),
                timestamp,
            }],
            miss::CODEC_H265 if !self.told_h265 => {
                self.told_h265 = true;
                return Err("this camera sends H.265 pictures, which Kagaz's player cannot show yet".into());
            }
            _ => Vec::new(),
        })
    }

    /// Talk through the camera's speaker: 8 kHz sound, sent as A-law in
    /// 40 ms pieces (call [`Live::start_talking`] first).
    pub fn talk(&mut self, pcm: &[i16]) -> Result<(), String> {
        for piece in pcm.chunks(320) {
            let bytes = Law::A.encode(piece);
            self.client.write_audio(miss::CODEC_PCMA, &bytes)?;
        }
        Ok(())
    }

    pub fn start_talking(&mut self) -> Result<(), String> {
        if self.client.speaker_codec() != miss::CODEC_PCMA {
            return Err("this camera's speaker takes a kind of sound Kagaz does not send yet".into());
        }
        self.client.start_speaker()
    }
}
