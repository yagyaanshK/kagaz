//! The stream Wi-Fi Display carries: H.264 in an MPEG transport stream,
//! seven 188-byte packets at most to an RTP packet (payload type 33), over
//! UDP to the port the TV named.

/// The program map's packet id, and the video's (the values the Wi-Fi
/// Display specification gives for its one program).
const PMT_PID: u16 = 0x0100;
const VIDEO_PID: u16 = 0x1011;
const TS: usize = 188;

/// How far ahead of the clock reference each picture is stamped, in 90 kHz
/// ticks: the TV's buffer. 100 ms.
pub const PTS_AHEAD: u64 = 9_000;

/// MPEG's CRC-32 (polynomial 0x04C11DB7, no reflection, no final xor).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

/// Packs H.264 access units into transport stream packets.
pub struct Muxer {
    cc_pat: u8,
    cc_pmt: u8,
    cc_video: u8,
    frames: u64,
}

impl Default for Muxer {
    fn default() -> Self {
        Self::new()
    }
}

impl Muxer {
    pub fn new() -> Muxer {
        Muxer { cc_pat: 0, cc_pmt: 0, cc_video: 0, frames: 0 }
    }

    fn header(pid: u16, start: bool, adaptation: bool, cc: u8) -> [u8; 4] {
        [
            0x47,
            (if start { 0x40 } else { 0 }) | ((pid >> 8) as u8 & 0x1f),
            pid as u8,
            (if adaptation { 0x30 } else { 0x10 }) | (cc & 0x0f),
        ]
    }

    fn section_packet(pid: u16, cc: u8, section: &[u8]) -> [u8; TS] {
        let mut p = [0xFFu8; TS];
        p[..4].copy_from_slice(&Self::header(pid, true, false, cc));
        p[4] = 0; // pointer field
        p[5..5 + section.len()].copy_from_slice(section);
        p
    }

    fn pat(&mut self) -> [u8; TS] {
        let mut s = vec![
            0x00, // table id: PAT
            0xB0, 0x0D, // section length 13
            0x00, 0x01, // transport stream id
            0xC1, 0x00, 0x00, // version 0, current; section 0 of 0
            0x00, 0x01, // program 1
            0xE0 | (PMT_PID >> 8) as u8,
            PMT_PID as u8,
        ];
        let crc = crc32(&s);
        s.extend_from_slice(&crc.to_be_bytes());
        let p = Self::section_packet(0, self.cc_pat, &s);
        self.cc_pat = self.cc_pat.wrapping_add(1);
        p
    }

    fn pmt(&mut self) -> [u8; TS] {
        let mut s = vec![
            0x02, // table id: PMT
            0xB0, 0x12, // section length 18
            0x00, 0x01, // program 1
            0xC1, 0x00, 0x00,
            0xE0 | (VIDEO_PID >> 8) as u8, VIDEO_PID as u8, // clock reference on the video
            0xF0, 0x00, // no program descriptors
            0x1B, // H.264
            0xE0 | (VIDEO_PID >> 8) as u8, VIDEO_PID as u8,
            0xF0, 0x00,
        ];
        let crc = crc32(&s);
        s.extend_from_slice(&crc.to_be_bytes());
        let p = Self::section_packet(PMT_PID, self.cc_pmt, &s);
        self.cc_pmt = self.cc_pmt.wrapping_add(1);
        p
    }

    /// One access unit (Annex B, as the encoder gives it) as transport
    /// stream packets. `pcr` is in 27 MHz ticks, `pts` in 90 kHz ticks.
    /// The tables go first on every key frame and every 15th picture.
    pub fn frame(&mut self, au: &[u8], key: bool, pcr: u64, pts: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(au.len() + au.len() / 180 * 10 + 3 * TS);
        if key || self.frames % 15 == 0 {
            out.extend_from_slice(&self.pat());
            out.extend_from_slice(&self.pmt());
        }
        self.frames += 1;

        // PES: an access unit delimiter first, then the picture.
        let mut pes = Vec::with_capacity(au.len() + 20);
        pes.extend_from_slice(&[0, 0, 1, 0xE0, 0, 0]); // video stream, unbounded length
        pes.extend_from_slice(&[0x80, 0x80, 5]); // PTS only
        let pts = pts & 0x1_FFFF_FFFF;
        pes.push(0x21 | ((pts >> 29) as u8 & 0x0E));
        pes.push((pts >> 22) as u8);
        pes.push(0x01 | ((pts >> 14) as u8 & 0xFE));
        pes.push((pts >> 7) as u8);
        pes.push(0x01 | ((pts << 1) as u8 & 0xFE));
        pes.extend_from_slice(&[0, 0, 0, 1, 0x09, 0xF0]);
        pes.extend_from_slice(au);

        let mut rest = &pes[..];
        let mut first = true;
        while !rest.is_empty() {
            let mut p = [0xFFu8; TS];
            // The first packet carries the clock reference (and marks a key
            // frame); the last is padded through its adaptation field.
            let mut af: Vec<u8> = Vec::new();
            if first {
                let base = (pcr / 300) & 0x1_FFFF_FFFF;
                let ext = (pcr % 300) as u16;
                af.push(0x10 | if key { 0x40 } else { 0 });
                af.push((base >> 25) as u8);
                af.push((base >> 17) as u8);
                af.push((base >> 9) as u8);
                af.push((base >> 1) as u8);
                af.push(((base as u8 & 1) << 7) | 0x7E | ((ext >> 8) as u8 & 1));
                af.push(ext as u8);
            }
            let room = |af_len: usize| -> usize {
                if af_len == 0 && !first { TS - 4 } else { TS - 4 - 1 - af_len }
            };
            let mut has_af = first;
            if rest.len() < room(af.len()) {
                // Pad: grow the adaptation field to fill the packet.
                let need = TS - 4 - rest.len();
                has_af = true;
                if need == 1 {
                    af.clear();
                } else {
                    if af.is_empty() {
                        af.push(0x00);
                    }
                    while 1 + af.len() < need {
                        af.push(0xFF);
                    }
                }
            }
            p[..4].copy_from_slice(&Self::header(VIDEO_PID, first, has_af, self.cc_video));
            self.cc_video = self.cc_video.wrapping_add(1);
            let mut at = 4;
            if has_af {
                p[at] = af.len() as u8;
                p[at + 1..at + 1 + af.len()].copy_from_slice(&af);
                at += 1 + af.len();
            }
            let take = (TS - at).min(rest.len());
            p[at..at + take].copy_from_slice(&rest[..take]);
            rest = &rest[take..];
            first = false;
            out.extend_from_slice(&p);
        }
        out
    }
}

/// Wraps transport stream packets in RTP, seven to a packet.
pub struct Rtp {
    seq: u16,
    ssrc: u32,
}

impl Rtp {
    pub fn new(ssrc: u32) -> Rtp {
        Rtp { seq: 0, ssrc }
    }

    /// The RTP packets for `ts` (a whole number of 188-byte packets),
    /// stamped with `pts` (90 kHz).
    pub fn packets(&mut self, ts: &[u8], pts: u64) -> Vec<Vec<u8>> {
        ts.chunks(7 * TS)
            .map(|chunk| {
                let mut p = Vec::with_capacity(12 + chunk.len());
                p.push(0x80);
                p.push(33);
                p.extend_from_slice(&self.seq.to_be_bytes());
                p.extend_from_slice(&(pts as u32).to_be_bytes());
                p.extend_from_slice(&self.ssrc.to_be_bytes());
                p.extend_from_slice(chunk);
                self.seq = self.seq.wrapping_add(1);
                p
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_a_known_pat() {
        // The PAT of a one-program stream with its map at 0x1000, as
        // common muxers write it.
        let s = [0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01, 0xF0, 0x00];
        assert_eq!(crc32(&s), 0x2AB1_04B2);
        let mut with = s.to_vec();
        with.extend_from_slice(&crc32(&s).to_be_bytes());
        assert_eq!(crc32(&with), 0);
    }

    #[test]
    fn frames_are_whole_packets_with_counters() {
        let mut m = Muxer::new();
        for len in [1usize, 160, 170, 171, 172, 182, 183, 184, 2000, 50_000] {
            let au = vec![0xAB; len];
            let out = m.frame(&au, len == 1, 27_000_000, 90_000 + PTS_AHEAD);
            assert_eq!(out.len() % TS, 0, "length {len}");
            // Every packet starts with the sync byte; the payload, read back,
            // is the PES with the whole access unit.
            let mut pes = Vec::new();
            for p in out.chunks(TS) {
                assert_eq!(p[0], 0x47);
                let pid = (((p[1] & 0x1f) as u16) << 8) | p[2] as u16;
                if pid != VIDEO_PID {
                    continue;
                }
                let mut at = 4;
                if p[3] & 0x20 != 0 {
                    at += 1 + p[4] as usize;
                }
                pes.extend_from_slice(&p[at..]);
            }
            assert_eq!(&pes[pes.len() - len..], &au[..], "length {len}");
            assert_eq!(&pes[..4], &[0, 0, 1, 0xE0]);
        }
    }

    #[test]
    fn rtp_seven_to_a_packet() {
        let mut r = Rtp::new(1);
        let ts = vec![0x47u8; TS * 10];
        let p = r.packets(&ts, 1234);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].len(), 12 + 7 * TS);
        assert_eq!(p[1].len(), 12 + 3 * TS);
        assert_eq!(&p[1][2..4], &[0, 1]);
        assert_eq!(p[0][1], 33);
    }
}
