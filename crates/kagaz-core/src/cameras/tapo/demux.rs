//! A camera's MPEG-TS taken apart as it streams by: the H.264 pictures as
//! whole access units and the G.711 sound as 16-bit samples, for the
//! window's own player. (`audio` keeps the WAV route the helper's modes use.)

use crate::intercom::g711::Law;
use crate::media::{nal_type, nal_units, Media, START};

/// A picture larger than this is not a picture.
const MAX_UNIT: usize = 4 * 1024 * 1024;

/// What has been seen, for diagnosing a tile without a picture.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub packets: u64,
    pub units: u64,
    pub keyframes: u64,
    /// Pictures dropped while waiting for the first keyframe.
    pub waiting: u64,
}

#[derive(Default)]
pub struct Demux {
    pub stats: Stats,
    pmt_pid: Option<u16>,
    video_pid: Option<u16>,
    audio_pid: Option<u16>,
    law: Option<Law>,
    carry: Vec<u8>,
    /// The picture being put together (a PES payload, already Annex-B).
    unit: Vec<u8>,
    unit_pts: u32,
    audio_pts: u32,
    /// Bytes still to come of the current PES packet, when it said its
    /// length (video usually does not); anything past that is stuffing.
    video_left: usize,
    audio_left: usize,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    /// Nothing until a keyframe: a decoder cannot start on a delta picture.
    need_key: bool,
}

impl Demux {
    pub fn new() -> Self {
        Demux {
            need_key: true,
            ..Default::default()
        }
    }

    /// Feed any slice of the stream; returns the pictures and sound it completed.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Media> {
        let mut out = Vec::new();
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(bytes);
        let mut i = 0;
        while i + 188 <= data.len() {
            if data[i] != 0x47 {
                i += 1;
                continue;
            }
            self.packet(&data[i..i + 188], &mut out);
            i += 188;
        }
        self.carry = data[i..].to_vec();
        out
    }

    fn packet(&mut self, p: &[u8], out: &mut Vec<Media>) {
        let pid = (u16::from(p[1] & 0x1F) << 8) | u16::from(p[2]);
        let pusi = p[1] & 0x40 != 0;
        if p[3] & 0x10 == 0 {
            return; // no payload
        }
        let mut off = 4;
        if (p[3] >> 4) & 0x3 >= 2 {
            off += 1 + usize::from(p[4]);
        }
        if off >= 188 {
            return;
        }
        if pid == 0 || Some(pid) == self.pmt_pid {
            self.table(p, off, pusi, pid == 0);
            return;
        }
        let is_video = Some(pid) == self.video_pid;
        if !is_video && Some(pid) != self.audio_pid {
            return;
        }
        self.stats.packets += 1;
        let mut payload = &p[off..];
        let mut pts = None;
        if pusi && payload.len() >= 9 && payload[..3] == [0, 0, 1] {
            let header = 9 + usize::from(payload[8]);
            let length = (usize::from(payload[4]) << 8) | usize::from(payload[5]);
            let left = if length == 0 {
                usize::MAX
            } else {
                (length + 6).saturating_sub(header)
            };
            if is_video {
                self.video_left = left;
            } else {
                self.audio_left = left;
            }
            if payload[7] & 0x80 != 0 && payload.len() >= 14 {
                let q = &payload[9..14];
                let t = ((u64::from(q[0]) & 0x0E) << 29)
                    | (u64::from(q[1]) << 22)
                    | ((u64::from(q[2]) & 0xFE) << 14)
                    | (u64::from(q[3]) << 7)
                    | (u64::from(q[4]) >> 1);
                pts = Some(t as u32);
            }
            payload = payload.get(header..).unwrap_or(&[]);
        }
        let left = if is_video {
            &mut self.video_left
        } else {
            &mut self.audio_left
        };
        if payload.len() > *left {
            payload = &payload[..*left];
        }
        *left = left.saturating_sub(payload.len());
        if is_video {
            if pusi {
                self.finish(out);
                if let Some(t) = pts {
                    self.unit_pts = t;
                }
            }
            if self.unit.len() + payload.len() <= MAX_UNIT {
                self.unit.extend_from_slice(payload);
            } else {
                self.unit.clear();
                self.need_key = true;
            }
        } else if !payload.is_empty() {
            if let Some(t) = pts {
                self.audio_pts = t;
            }
            let law = self.law.unwrap_or(Law::A);
            out.push(Media::Audio {
                pcm: law.decode(payload),
                timestamp: self.audio_pts,
            });
            // The next chunk of the same frame follows this one in time.
            self.audio_pts = self.audio_pts.wrapping_add(payload.len() as u32);
        }
    }

    /// The program tables: which packets carry the picture and the sound.
    fn table(&mut self, p: &[u8], off: usize, pusi: bool, pat: bool) {
        let mut s = off;
        if pusi {
            s += 1 + usize::from(p[off]);
        }
        if s + 12 > 188 {
            return;
        }
        let sec = &p[s..];
        let len = (usize::from(sec[1] & 0x0F) << 8) | usize::from(sec[2]);
        let end = (3 + len).saturating_sub(4).min(sec.len());
        if pat && sec[0] == 0 {
            let mut j = 8;
            while j + 4 <= end {
                let program = (u16::from(sec[j]) << 8) | u16::from(sec[j + 1]);
                let ppid = (u16::from(sec[j + 2] & 0x1F) << 8) | u16::from(sec[j + 3]);
                if program != 0 {
                    self.pmt_pid = Some(ppid);
                }
                j += 4;
            }
        } else if !pat && sec[0] == 2 && self.video_pid.is_none() && self.audio_pid.is_none() {
            let info = (usize::from(sec[10] & 0x0F) << 8) | usize::from(sec[11]);
            let mut j = 12 + info;
            while j + 5 <= end {
                let stream_type = sec[j];
                let epid = (u16::from(sec[j + 1] & 0x1F) << 8) | u16::from(sec[j + 2]);
                let es_len = (usize::from(sec[j + 3] & 0x0F) << 8) | usize::from(sec[j + 4]);
                match stream_type {
                    0x1B if self.video_pid.is_none() => self.video_pid = Some(epid),
                    0x90 => {
                        self.audio_pid = Some(epid);
                        self.law = Some(Law::A);
                    }
                    0x91 => {
                        self.audio_pid = Some(epid);
                        self.law = Some(Law::Mu);
                    }
                    _ => {}
                }
                j += 5 + es_len;
            }
        }
    }

    /// The picture put together so far is complete: hand it out as one
    /// Annex-B unit with 4-byte start codes, the parameter sets in front of
    /// a keyframe that lacks them.
    fn finish(&mut self, out: &mut Vec<Media>) {
        let unit = std::mem::take(&mut self.unit);
        if unit.is_empty() {
            return;
        }
        let mut key = false;
        let mut has_sps = false;
        let mut data = Vec::with_capacity(unit.len() + 64);
        for nal in nal_units(&unit) {
            match nal_type(nal) {
                5 => key = true,
                7 => {
                    self.sps = Some(nal.to_vec());
                    has_sps = true;
                    key = true;
                }
                8 => self.pps = Some(nal.to_vec()),
                _ => {}
            }
            data.extend_from_slice(&START);
            data.extend_from_slice(nal);
        }
        if data.is_empty() {
            return;
        }
        if self.need_key {
            if !key {
                self.stats.waiting += 1;
                return;
            }
            self.need_key = false;
        }
        if key && !has_sps {
            let mut front = Vec::new();
            for ps in [&self.sps, &self.pps].into_iter().flatten() {
                front.extend_from_slice(&START);
                front.extend_from_slice(ps);
            }
            front.extend_from_slice(&data);
            data = front;
        }
        self.stats.units += 1;
        if key {
            self.stats.keyframes += 1;
        }
        out.push(Media::Video {
            data,
            key,
            timestamp: self.unit_pts,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![
            0x47,
            ((pid >> 8) as u8) | if pusi { 0x40 } else { 0 },
            pid as u8,
            0x10,
        ];
        p.extend_from_slice(payload);
        p.resize(188, 0xFF);
        p
    }

    // A PES packet that says its length, as the camera's sound does; the
    // rest of the TS packet is stuffing.
    fn pes(pts: u64, body: &[u8]) -> Vec<u8> {
        let length = 3 + 5 + body.len();
        let mut p = vec![0, 0, 1, 0xE0, (length >> 8) as u8, length as u8, 0x80, 0x80, 5];
        p.push(0x21 | (((pts >> 30) & 7) as u8) << 1);
        p.push((pts >> 22) as u8);
        p.push((((pts >> 15) & 0x7F) as u8) << 1 | 1);
        p.push((pts >> 7) as u8);
        p.push(((pts & 0x7F) as u8) << 1 | 1);
        p.extend_from_slice(body);
        p
    }

    fn tables() -> Vec<u8> {
        let mut out = packet(0, true, &[0, 0, 0xB0, 13, 0, 1, 0xC1, 0, 0, 0, 1, 0xE0, 0x42, 0, 0, 0, 0]);
        // PMT: video 0x1b on 0x44, audio 0x90 on 0x45.
        out.extend(packet(
            0x42,
            true,
            &[
                0, 2, 0xB0, 23, 0, 1, 0xC1, 0, 0, 0xE0, 0x44, 0xF0, 0, 0x1B, 0xE0, 0x44, 0xF0, 0, 0x90,
                0xE0, 0x45, 0xF0, 0, 0, 0, 0, 0,
            ],
        ));
        out
    }

    #[test]
    fn delivers_whole_pictures_from_the_first_keyframe_and_sound_as_samples() {
        let mut d = Demux::new();
        let mut ts = tables();
        // A delta picture first: held back.
        ts.extend(packet(0x44, true, &pes(900, &[0, 0, 1, 0x41, 9])));
        // A keyframe with its parameter sets, split over two packets; a
        // picture's PES gives no length, so the packets are filled exactly.
        let mut idr = pes(1800, &[0, 0, 0, 1, 0x67, 0x42, 0xE0, 0x1F, 0, 0, 0, 1, 0x68, 0xCE]);
        idr[4] = 0;
        idr[5] = 0;
        let mut first = packet(0x44, true, &idr);
        first.truncate(4 + idr.len());
        first.resize(188, 0); // zero stuffing reads as part of the next start code
        ts.extend(first);
        let mut second = packet(0x44, false, &[0, 0, 1, 0x65, 1, 2, 3]);
        second.truncate(4 + 7);
        second.resize(188, 0);
        ts.extend(second);
        // Sound: two A-law bytes at PTS 1000.
        ts.extend(packet(0x45, true, &pes(1000, &[0xD5, 0x55])));
        // The next picture closes the keyframe.
        ts.extend(packet(0x44, true, &pes(2700, &[0, 0, 1, 0x41, 7])));
        let media = d.push(&ts);
        assert_eq!(media.len(), 2);
        match &media[0] {
            Media::Audio { pcm, timestamp } => {
                assert_eq!(*timestamp, 1000);
                assert_eq!(pcm.len(), 2);
            }
            other => panic!("expected sound, got {other:?}"),
        }
        match &media[1] {
            Media::Video { data, key, timestamp } => {
                assert!(key);
                assert_eq!(*timestamp, 1800);
                let types: Vec<u8> = nal_units(data).iter().map(|u| nal_type(u)).collect();
                assert_eq!(types, vec![7, 8, 5]);
                assert!(data.starts_with(&[0, 0, 0, 1, 0x67]));
            }
            other => panic!("expected a picture, got {other:?}"),
        }
        assert_eq!(d.stats.waiting, 1);
        assert_eq!(d.stats.keyframes, 1);
        // The next picture closes the delta one at 2700; a later keyframe
        // without its parameter sets gets them put in front.
        let mut more = packet(0x44, true, &pes(3600, &[0, 0, 1, 0x65, 8]));
        more.extend(packet(0x44, true, &pes(4500, &[0, 0, 1, 0x41, 1])));
        let media = d.push(&more);
        assert_eq!(media.len(), 2);
        assert!(matches!(&media[0], Media::Video { key: false, timestamp: 2700, .. }));
        match &media[1] {
            Media::Video { data, key, timestamp } => {
                assert!(key);
                assert_eq!(*timestamp, 3600);
                let types: Vec<u8> = nal_units(data).iter().map(|u| nal_type(u)).collect();
                assert_eq!(types, vec![7, 8, 5]);
            }
            other => panic!("expected a picture, got {other:?}"),
        }
    }
}
