//! H.264 over RTP (RFC 6184) back into whole pictures: single NAL units,
//! STAP-A aggregates and FU-A fragments become one Annex-B access unit per
//! RTP timestamp. A lost packet spoils its picture, and every later one
//! until the next keyframe, so a decoder never sees a damaged frame.

/// One picture, as Annex-B (start code before every NAL unit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUnit {
    pub data: Vec<u8>,
    /// Starts a new sequence: holds an IDR slice (or SPS), safe to decode from.
    pub key: bool,
    /// The RTP timestamp (90 kHz).
    pub timestamp: u32,
}

const START: [u8; 4] = [0, 0, 0, 1];
const MAX_UNIT: usize = 4 * 1024 * 1024;

/// What the depacketizer has seen, for diagnosing a call without a picture.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub packets: u64,
    pub units: u64,
    pub keyframes: u64,
    /// Pictures dropped because a packet was lost.
    pub damaged: u64,
    /// Pictures dropped while waiting for a keyframe.
    pub waiting: u64,
    /// Count of each NAL unit type (0..32) seen, including fragment starts.
    pub nal_types: [u64; 32],
}

#[derive(Default)]
pub struct Depacketizer {
    pub stats: Stats,
    unit: Vec<u8>,
    key: bool,
    timestamp: Option<u32>,
    next_seq: Option<u16>,
    /// Inside an FU-A fragment run.
    fragment: bool,
    /// The current picture lost a packet.
    damaged: bool,
    /// Waiting for a keyframe after a loss (or at the start).
    need_key: bool,
    /// SPS/PPS seen out of band (SDP) or in band, put before each keyframe
    /// that lacks them.
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl Depacketizer {
    pub fn new() -> Self {
        Depacketizer {
            need_key: true,
            ..Default::default()
        }
    }

    /// Parameter sets from SDP `sprop-parameter-sets` (base64, comma separated).
    pub fn set_parameter_sets(&mut self, sprop: &str) {
        use base64::Engine as _;
        for part in sprop.split(',') {
            let Ok(nal) = base64::engine::general_purpose::STANDARD.decode(part.trim()) else {
                continue;
            };
            match nal.first().map(|b| b & 0x1F) {
                Some(7) => self.sps = Some(nal),
                Some(8) => self.pps = Some(nal),
                _ => {}
            }
        }
    }

    /// Feed one RTP packet (header included). Returns a finished picture
    /// when this packet completes one.
    pub fn push(&mut self, packet: &[u8]) -> Option<AccessUnit> {
        let (seq, timestamp, marker, payload) = parse_rtp(packet)?;
        self.stats.packets += 1;
        if let Some(&h) = payload.first() {
            let t = (h & 0x1F) as usize;
            let t = if t == 28 && payload.len() > 1 && payload[1] & 0x80 != 0 {
                (payload[1] & 0x1F) as usize
            } else {
                t
            };
            if t != 28 {
                self.stats.nal_types[t] += 1;
            }
        }
        let mut done = None;
        if self.timestamp.is_some_and(|t| t != timestamp) {
            // A new picture began without the last one's marker bit.
            done = self.finish();
        }
        if let Some(expected) = self.next_seq {
            if seq != expected {
                self.damaged = true;
                self.fragment = false;
            }
        }
        self.next_seq = Some(seq.wrapping_add(1));
        self.timestamp = Some(timestamp);
        self.payload(payload);
        if marker {
            done = self.finish().or(done);
        }
        done
    }

    fn nal(&mut self, nal: &[u8]) {
        let Some(&first) = nal.first() else {
            return;
        };
        match first & 0x1F {
            5 => self.key = true,
            7 => {
                self.sps = Some(nal.to_vec());
                self.key = true;
            }
            8 => self.pps = Some(nal.to_vec()),
            _ => {}
        }
        if self.unit.len() + nal.len() + 4 <= MAX_UNIT {
            self.unit.extend_from_slice(&START);
            self.unit.extend_from_slice(nal);
        } else {
            self.damaged = true;
        }
    }

    fn payload(&mut self, p: &[u8]) {
        let Some(&head) = p.first() else {
            return;
        };
        match head & 0x1F {
            1..=23 => self.nal(p),
            24 => {
                // STAP-A: 16-bit size, then that many bytes, repeated.
                let mut rest = &p[1..];
                while rest.len() >= 2 {
                    let n = u16::from_be_bytes([rest[0], rest[1]]) as usize;
                    if rest.len() < 2 + n {
                        self.damaged = true;
                        break;
                    }
                    let nal = rest[2..2 + n].to_vec();
                    self.nal(&nal);
                    rest = &rest[2 + n..];
                }
            }
            28 => {
                // FU-A: indicator, header (S/E bits + type), fragment.
                if p.len() < 2 {
                    return;
                }
                let fu = p[1];
                let start = fu & 0x80 != 0;
                let end = fu & 0x40 != 0;
                let kind = fu & 0x1F;
                if start {
                    if kind == 5 {
                        self.key = true;
                    }
                    if self.unit.len() + p.len() + 4 > MAX_UNIT {
                        self.damaged = true;
                        return;
                    }
                    self.unit.extend_from_slice(&START);
                    self.unit.push((head & 0xE0) | kind);
                    self.unit.extend_from_slice(&p[2..]);
                    self.fragment = !end;
                } else if self.fragment {
                    if self.unit.len() + p.len() > MAX_UNIT {
                        self.damaged = true;
                        return;
                    }
                    self.unit.extend_from_slice(&p[2..]);
                    if end {
                        self.fragment = false;
                    }
                } else {
                    // A middle or end fragment whose start was lost.
                    self.damaged = true;
                }
            }
            _ => {}
        }
    }

    fn finish(&mut self) -> Option<AccessUnit> {
        let unit = std::mem::take(&mut self.unit);
        let key = std::mem::take(&mut self.key);
        let damaged = std::mem::take(&mut self.damaged) || self.fragment;
        self.fragment = false;
        let timestamp = self.timestamp.take()?;
        if unit.is_empty() {
            return None;
        }
        if damaged {
            self.need_key = true;
            self.stats.damaged += 1;
            return None;
        }
        if self.need_key {
            if !key {
                self.stats.waiting += 1;
                return None;
            }
            self.need_key = false;
        }
        self.stats.units += 1;
        if key {
            self.stats.keyframes += 1;
        }
        let mut data = unit;
        if key && !has_nal(&data, 7) {
            // Put the parameter sets in front so the decoder can start here.
            let mut front = Vec::new();
            for ps in [&self.sps, &self.pps].into_iter().flatten() {
                front.extend_from_slice(&START);
                front.extend_from_slice(ps);
            }
            front.extend_from_slice(&data);
            data = front;
        }
        Some(AccessUnit {
            data,
            key,
            timestamp,
        })
    }
}

/// Whether an Annex-B unit holds a NAL unit of this type.
fn has_nal(annexb: &[u8], kind: u8) -> bool {
    annexb
        .windows(5)
        .any(|w| w[..4] == START && w[4] & 0x1F == kind)
}

/// (sequence, timestamp, marker, payload) of an RTP packet.
pub fn parse_rtp(p: &[u8]) -> Option<(u16, u32, bool, &[u8])> {
    if p.len() < 12 || p[0] >> 6 != 2 {
        return None;
    }
    let marker = p[1] & 0x80 != 0;
    let seq = u16::from_be_bytes([p[2], p[3]]);
    let ts = u32::from_be_bytes([p[4], p[5], p[6], p[7]]);
    let mut start = 12 + 4 * (p[0] & 0x0F) as usize;
    if p[0] & 0x10 != 0 {
        let ext = p.get(start..start + 4)?;
        start += 4 + 4 * u16::from_be_bytes([ext[2], ext[3]]) as usize;
    }
    let mut end = p.len();
    if p[0] & 0x20 != 0 {
        end = end.checked_sub(*p.last()? as usize)?;
    }
    (start <= end).then(|| (seq, ts, marker, &p[start..end]))
}

/// The WebCodecs codec string (`avc1.PPCCLL`) for an SPS NAL unit.
pub fn codec_string(sps: &[u8]) -> Option<String> {
    (sps.len() >= 4).then(|| format!("avc1.{:02X}{:02X}{:02X}", sps[1], sps[2], sps[3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtp(seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, if marker { 0xE8 } else { 0x68 }];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&ts.to_be_bytes());
        p.extend_from_slice(&[0, 0, 0, 1]);
        p.extend_from_slice(payload);
        p
    }

    const SPS: &[u8] = &[0x67, 0x42, 0xE0, 0x1F, 0xAA];
    const PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];

    #[test]
    fn rebuilds_a_keyframe_from_stap_a_and_fu_a() {
        let mut d = Depacketizer::new();
        let mut stap = vec![0x18];
        for nal in [SPS, PPS] {
            stap.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            stap.extend_from_slice(nal);
        }
        assert!(d.push(&rtp(1, 900, false, &stap)).is_none());
        // IDR (type 5, NRI 3) split in two fragments.
        assert!(d.push(&rtp(2, 900, false, &[0x7C, 0x85, 1, 2])).is_none());
        let unit = d.push(&rtp(3, 900, true, &[0x7C, 0x45, 3, 4])).unwrap();
        assert!(unit.key);
        assert_eq!(unit.timestamp, 900);
        let mut want = Vec::new();
        for nal in [SPS, PPS, &[0x65, 1, 2, 3, 4][..]] {
            want.extend_from_slice(&START);
            want.extend_from_slice(nal);
        }
        assert_eq!(unit.data, want);
        assert_eq!(codec_string(SPS).unwrap(), "avc1.42E01F");
    }

    #[test]
    fn waits_for_a_keyframe_and_drops_damaged_pictures() {
        let mut d = Depacketizer::new();
        // A delta frame before any keyframe is not delivered.
        assert!(d.push(&rtp(1, 100, true, &[0x41, 9])).is_none());
        // A keyframe whose middle fragment is lost: dropped.
        d.set_parameter_sets("Z0LgH6o=,aM48gA==");
        assert!(d.push(&rtp(2, 200, false, &[0x7C, 0x85, 1])).is_none());
        assert!(d.push(&rtp(4, 200, true, &[0x7C, 0x45, 3])).is_none());
        // The following delta frame is held back too.
        assert!(d.push(&rtp(5, 300, true, &[0x41, 9])).is_none());
        // A whole keyframe gets the out-of-band parameter sets in front.
        let unit = d.push(&rtp(6, 400, true, &[0x65, 7])).unwrap();
        assert!(unit.key);
        assert!(unit.data.starts_with(&[0, 0, 0, 1, 0x67]));
        // And a delta frame after it flows.
        let delta = d.push(&rtp(7, 500, true, &[0x41, 9])).unwrap();
        assert!(!delta.key);
        assert_eq!(delta.data, vec![0, 0, 0, 1, 0x41, 9]);
    }

    #[test]
    fn a_new_timestamp_closes_a_picture_without_a_marker() {
        let mut d = Depacketizer::new();
        assert!(d.push(&rtp(1, 100, false, &[0x65, 1])).is_none());
        let unit = d.push(&rtp(2, 200, false, &[0x41, 2])).unwrap();
        assert_eq!(unit.timestamp, 100);
        assert!(unit.key);
    }
}
