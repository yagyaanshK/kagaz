//! Playback speed by rewriting time: the player in the window takes a
//! streamed MPEG-TS at the pace its timestamps say, and ignores a playback
//! rate on it (measured). So for 2x Kagaz halves the distance between
//! timestamps (PTS, DTS and PCR) as the stream goes by, and the camera's
//! fast delivery keeps the player fed; for 0.5x it doubles them.

const PTS_MOD: u64 = 1 << 33;

/// Rescales the timestamps in an MPEG-TS stream by a factor.
#[derive(Debug)]
pub struct Retimer {
    speed: f64,
    first_pts: Option<u64>,
    first_pcr: Option<u64>,
    carry: Vec<u8>,
}

impl Retimer {
    pub fn new(speed: f64) -> Retimer {
        Retimer {
            speed: if speed.is_finite() && speed > 0.0 {
                speed
            } else {
                1.0
            },
            first_pts: None,
            first_pcr: None,
            carry: Vec::new(),
        }
    }

    /// Feed any slice of the stream; returns whole packets, retimed.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        if (self.speed - 1.0).abs() < 1e-9 {
            return bytes.to_vec();
        }
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(bytes);
        let mut i = 0;
        while i + 188 <= data.len() {
            if data[i] != 0x47 {
                i += 1;
                continue;
            }
            self.packet(&mut data[i..i + 188]);
            i += 188;
        }
        self.carry = data[i..].to_vec();
        data.truncate(i);
        data
    }

    fn scale(&self, first: u64, t: u64) -> u64 {
        // Distance from the first stamp, modulo the 33-bit wrap, divided by the speed.
        let delta = (t + PTS_MOD - first) % PTS_MOD;
        (first + (delta as f64 / self.speed) as u64) % PTS_MOD
    }

    fn packet(&mut self, p: &mut [u8]) {
        let pusi = p[1] & 0x40 != 0;
        let afc = (p[3] >> 4) & 0x3;
        let mut off = 4;
        if afc >= 2 {
            let len = usize::from(p[4]);
            // PCR: 6 bytes after the flags, when the flag says so.
            if len >= 7 && p[5] & 0x10 != 0 {
                let base = (u64::from(p[6]) << 25)
                    | (u64::from(p[7]) << 17)
                    | (u64::from(p[8]) << 9)
                    | (u64::from(p[9]) << 1)
                    | (u64::from(p[10]) >> 7);
                let first = *self.first_pcr.get_or_insert(base);
                let scaled = self.scale(first, base);
                p[6] = (scaled >> 25) as u8;
                p[7] = (scaled >> 17) as u8;
                p[8] = (scaled >> 9) as u8;
                p[9] = (scaled >> 1) as u8;
                p[10] = (p[10] & 0x7F) | (((scaled & 1) as u8) << 7);
            }
            off += 1 + len;
        }
        if afc == 2 || !pusi || off + 14 > 188 {
            return;
        }
        let h = &mut p[off..];
        if h[..3] != [0, 0, 1] {
            return;
        }
        let flags = h[7];
        if flags & 0x80 != 0 {
            let pts = read_stamp(&h[9..14]);
            let first = *self.first_pts.get_or_insert(pts);
            let scaled = self.scale(first, pts);
            write_stamp(&mut h[9..14], scaled);
            if flags & 0x40 != 0 && h.len() >= 19 {
                let dts = read_stamp(&h[14..19]);
                let scaled = self.scale(first, dts);
                write_stamp(&mut h[14..19], scaled);
            }
        }
    }
}

fn read_stamp(q: &[u8]) -> u64 {
    ((u64::from(q[0]) & 0x0E) << 29)
        | (u64::from(q[1]) << 22)
        | ((u64::from(q[2]) & 0xFE) << 14)
        | (u64::from(q[3]) << 7)
        | (u64::from(q[4]) >> 1)
}

fn write_stamp(q: &mut [u8], t: u64) {
    q[0] = (q[0] & 0xF0) | (((t >> 30) & 0x7) as u8) << 1 | 1;
    q[1] = (t >> 22) as u8;
    q[2] = (((t >> 15) & 0x7F) as u8) << 1 | 1;
    q[3] = (t >> 7) as u8;
    q[4] = ((t & 0x7F) as u8) << 1 | 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pes_packet(pid: u16, pts: u64) -> Vec<u8> {
        let mut p = vec![
            0x47,
            0x40 | (pid >> 8) as u8,
            pid as u8,
            0x10,
            0,
            0,
            1,
            0xE0,
            0,
            0,
            0x80,
            0x80,
            5,
        ];
        let mut stamp = [0x21u8; 5];
        write_stamp(&mut stamp, pts);
        p.extend_from_slice(&stamp);
        p.resize(188, 0);
        p
    }

    fn pcr_packet(pid: u16, pcr_base: u64) -> Vec<u8> {
        let mut p = vec![0x47, (pid >> 8) as u8, pid as u8, 0x20, 183, 0x10];
        p.extend_from_slice(&[
            (pcr_base >> 25) as u8,
            (pcr_base >> 17) as u8,
            (pcr_base >> 9) as u8,
            (pcr_base >> 1) as u8,
            (((pcr_base & 1) as u8) << 7) | 0x7E,
            0,
        ]);
        p.resize(188, 0xFF);
        p
    }

    #[test]
    fn stamps_round_trip() {
        for t in [0u64, 1, 90_000, 8_589_934_591] {
            let mut q = [0u8; 5];
            write_stamp(&mut q, t);
            assert_eq!(read_stamp(&q), t);
        }
    }

    #[test]
    fn double_speed_halves_the_distance_between_stamps() {
        let mut r = Retimer::new(2.0);
        let mut stream = pes_packet(0x44, 900_000);
        stream.extend(pes_packet(0x44, 900_000 + 180_000));
        stream.extend(pcr_packet(0x44, 900_000));
        stream.extend(pcr_packet(0x44, 900_000 + 180_000));
        let out = r.push(&stream);
        assert_eq!(out.len(), 188 * 4);
        assert_eq!(read_stamp(&out[13..18]), 900_000);
        assert_eq!(read_stamp(&out[188 + 13..188 + 18]), 900_000 + 90_000);
        let pcr = |p: &[u8]| {
            (u64::from(p[6]) << 25)
                | (u64::from(p[7]) << 17)
                | (u64::from(p[8]) << 9)
                | (u64::from(p[9]) << 1)
                | (u64::from(p[10]) >> 7)
        };
        assert_eq!(pcr(&out[2 * 188..]), 900_000);
        assert_eq!(pcr(&out[3 * 188..]), 900_000 + 90_000);
        // Half speed doubles it; 1x leaves bytes untouched.
        let mut slow = Retimer::new(0.5);
        let out = slow.push(&stream);
        assert_eq!(read_stamp(&out[188 + 13..188 + 18]), 900_000 + 360_000);
        assert_eq!(Retimer::new(1.0).push(&stream), stream);
        // Odd-sized pieces are carried over.
        let mut r = Retimer::new(2.0);
        let mut out = r.push(&stream[..200]);
        out.extend(r.push(&stream[200..]));
        assert_eq!(out.len(), 188 * 4);
    }
}
