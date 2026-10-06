//! Sound from a Tapo stream: the camera sends G.711 (A-law on the models
//! seen, stream type 0x90; 0x91 is taken as mu-law) inside its MPEG-TS.
//! Nothing downstream plays that, so Kagaz pulls the audio packets out of
//! the stream it is already relaying, decodes them to 16-bit PCM and serves
//! a plain streamed WAV, which the window's audio element plays.

/// Which G.711 companding the camera uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Law {
    ALaw,
    MuLaw,
}

/// Pulls the G.711 payload out of MPEG-TS as it streams by.
#[derive(Debug, Default)]
pub struct AudioDemux {
    pmt_pid: Option<u16>,
    audio_pid: Option<u16>,
    law: Option<Law>,
    carry: Vec<u8>,
}

impl AudioDemux {
    pub fn law(&self) -> Option<Law> {
        self.law
    }

    /// Feed any slice of the stream; returns the raw G.711 bytes found in it.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
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

    fn packet(&mut self, p: &[u8], out: &mut Vec<u8>) {
        let pid = (u16::from(p[1] & 0x1F) << 8) | u16::from(p[2]);
        let pusi = p[1] & 0x40 != 0;
        let has_payload = p[3] & 0x10 != 0;
        if !has_payload {
            return;
        }
        let mut off = 4;
        if (p[3] >> 4) & 0x3 >= 2 {
            off += 1 + usize::from(p[4]);
        }
        if off >= 188 {
            return;
        }
        if pid == 0 || Some(pid) == self.pmt_pid {
            // Tables: skip the pointer field, then read the section.
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
            if pid == 0 && sec[0] == 0 {
                let mut j = 8;
                while j + 4 <= end {
                    let program = (u16::from(sec[j]) << 8) | u16::from(sec[j + 1]);
                    let ppid = (u16::from(sec[j + 2] & 0x1F) << 8) | u16::from(sec[j + 3]);
                    if program != 0 {
                        self.pmt_pid = Some(ppid);
                    }
                    j += 4;
                }
            } else if sec[0] == 2 && self.audio_pid.is_none() {
                let info = (usize::from(sec[10] & 0x0F) << 8) | usize::from(sec[11]);
                let mut j = 12 + info;
                while j + 5 <= end {
                    let stream_type = sec[j];
                    let epid = (u16::from(sec[j + 1] & 0x1F) << 8) | u16::from(sec[j + 2]);
                    let es_len = (usize::from(sec[j + 3] & 0x0F) << 8) | usize::from(sec[j + 4]);
                    match stream_type {
                        0x90 => {
                            self.audio_pid = Some(epid);
                            self.law = Some(Law::ALaw);
                        }
                        0x91 => {
                            self.audio_pid = Some(epid);
                            self.law = Some(Law::MuLaw);
                        }
                        _ => {}
                    }
                    j += 5 + es_len;
                }
            }
            return;
        }
        if Some(pid) != self.audio_pid {
            return;
        }
        let mut payload = &p[off..];
        if pusi && payload.len() >= 9 && payload[..3] == [0, 0, 1] {
            let header = 9 + usize::from(payload[8]);
            payload = payload.get(header..).unwrap_or(&[]);
        }
        out.extend_from_slice(payload);
    }
}

/// One G.711 byte to a 16-bit sample.
pub fn decode_sample(law: Law, byte: u8) -> i16 {
    match law {
        Law::ALaw => {
            let b = byte ^ 0x55;
            let sign = b & 0x80;
            let exp = (b >> 4) & 7;
            let mant = i32::from(b & 0x0F);
            let mut v = (mant << 4) + 8;
            if exp > 0 {
                v = (v + 0x100) << (exp - 1);
            }
            // In A-law a set sign bit means positive.
            (if sign != 0 { v } else { -v }) as i16
        }
        Law::MuLaw => {
            let b = !byte;
            let sign = b & 0x80;
            let exp = (b >> 4) & 7;
            let mant = i32::from(b & 0x0F);
            let v = (((mant << 3) + 0x84) << exp) - 0x84;
            (if sign != 0 { -v } else { v }) as i16
        }
    }
}

/// G.711 bytes to little-endian 16-bit PCM.
pub fn decode(law: Law, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.extend_from_slice(&decode_sample(law, b).to_le_bytes());
    }
    out
}

/// A WAV head for an endless 8 kHz mono 16-bit stream (sizes set to the
/// maximum, as streaming players expect).
pub fn wav_header() -> [u8; 44] {
    let mut h = [0u8; 44];
    h[..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&0x7FFF_FFF6u32.to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&1u16.to_le_bytes()); // mono
    h[24..28].copy_from_slice(&8000u32.to_le_bytes());
    h[28..32].copy_from_slice(&16000u32.to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&0x7FFF_FFD2u32.to_le_bytes());
    h
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

    #[test]
    fn finds_the_audio_pid_from_the_tables_and_strips_pes_heads() {
        // PAT: program 1 on PID 0x100.
        let mut pat = vec![
            0u8, 0, 0xB0, 13, 0, 1, 0xC1, 0, 0, 0, 1, 0xE1, 0x00, 0, 0, 0, 0,
        ];
        pat[0] = 0; // pointer
        let pat_packet = packet(0, true, &pat);
        // PMT: video 0x1b on 0x44, audio 0x90 on 0x45.
        let pmt = vec![
            0u8, 2, 0xB0, 23, 0, 1, 0xC1, 0, 0, 0xE0, 0x44, 0xF0, 0, 0x1B, 0xE0, 0x44, 0xF0, 0,
            0x90, 0xE0, 0x45, 0xF0, 0, 0, 0, 0, 0,
        ];
        let pmt_packet = packet(0x100, true, &pmt);
        let mut pes = vec![0u8, 0, 1, 0xBD, 0, 0, 0x80, 0x80, 5, 0x21, 0, 1, 0, 1];
        pes.extend_from_slice(&[0xD5; 20]);
        let audio1 = packet(0x45, true, &pes);
        let audio2 = packet(0x45, false, &[0x55; 10]);
        let video = packet(0x44, true, &[1, 2, 3]);
        let mut all = Vec::new();
        for p in [&pat_packet, &pmt_packet, &video, &audio1, &audio2] {
            all.extend_from_slice(p);
        }
        let mut d = AudioDemux::default();
        // Feed in odd-sized pieces to exercise the carry.
        let mut out = d.push(&all[..200]);
        out.extend(d.push(&all[200..]));
        assert_eq!(d.law(), Some(Law::ALaw));
        assert_eq!(out.len(), 188 - 4 - 14 + 188 - 4);
        assert!(out[..20].iter().all(|&b| b == 0xD5));
    }

    #[test]
    fn g711_decodes_like_the_reference_tables() {
        assert_eq!(decode_sample(Law::ALaw, 0xD5), 8);
        assert_eq!(decode_sample(Law::ALaw, 0x55), -8);
        assert_eq!(decode_sample(Law::ALaw, 0x80), 5504);
        assert_eq!(decode_sample(Law::ALaw, 0x2A), -32256);
        assert_eq!(decode_sample(Law::MuLaw, 0xFF), 0);
        assert_eq!(decode_sample(Law::MuLaw, 0x7F), 0);
        assert_eq!(decode_sample(Law::MuLaw, 0x00), -32124);
        assert_eq!(decode(Law::ALaw, &[0xD5]), vec![8, 0]);
        let h = wav_header();
        assert_eq!(&h[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 8000);
    }
}
