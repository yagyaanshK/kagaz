//! Bounded H.264 RTP reordering and complete-frame delivery for UDP relays.
//! A lost fragment must not reach go2rtc's depayloader as a damaged picture.
use std::time::{Duration, Instant};

const REORDER_WAIT: Duration = Duration::from_millis(80);
const MAX_QUEUE: usize = 128;
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

struct Packet {
    bytes: Vec<u8>,
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
    payload: usize,
    end: usize,
    received: Instant,
}
impl Packet {
    fn parse(bytes: &[u8], received: Instant) -> Option<Self> {
        if bytes.len() < 12 || bytes[0] >> 6 != 2 {
            return None;
        }
        let mut payload = 12 + usize::from(bytes[0] & 15) * 4;
        if bytes[0] & 16 != 0 {
            let extension = bytes.get(payload..payload + 4)?;
            payload += 4 + usize::from(u16::from_be_bytes([extension[2], extension[3]])) * 4;
        }
        let padding = if bytes[0] & 32 != 0 {
            usize::from(*bytes.last()?)
        } else {
            0
        };
        if bytes[0] & 32 != 0 && padding == 0 {
            return None;
        }
        let end = bytes.len().checked_sub(padding)?;
        if payload >= end {
            return None;
        }
        Some(Self {
            bytes: bytes.to_vec(),
            sequence: u16::from_be_bytes([bytes[2], bytes[3]]),
            timestamp: u32::from_be_bytes(bytes[4..8].try_into().ok()?),
            ssrc: u32::from_be_bytes(bytes[8..12].try_into().ok()?),
            payload,
            end,
            received,
        })
    }
}

pub(super) struct H264Rtp {
    payload_type: u8,
    ssrc: Option<u32>,
    next: Option<u16>,
    started: bool,
    queue: Vec<Packet>,
    frame: Vec<Packet>,
    frame_bytes: usize,
    fragment: Option<u8>,
    keyframe: bool,
    need_keyframe: bool,
    sps: Vec<u8>,
    pps: Vec<u8>,
    output_sequence: u16,
}
impl H264Rtp {
    pub(super) fn new(payload_type: u8) -> Self {
        Self {
            payload_type,
            ssrc: None,
            next: None,
            started: false,
            queue: Vec::new(),
            frame: Vec::new(),
            frame_bytes: 0,
            fragment: None,
            keyframe: false,
            need_keyframe: true,
            sps: Vec::new(),
            pps: Vec::new(),
            output_sequence: 0,
        }
    }
    fn lose_frame(&mut self) {
        self.frame.clear();
        self.frame_bytes = 0;
        self.fragment = None;
        self.keyframe = false;
        self.need_keyframe = true;
    }
    pub(super) fn push(&mut self, bytes: &[u8], now: Instant) -> Vec<Vec<u8>> {
        let Some(packet) = Packet::parse(bytes, now) else {
            return Vec::new();
        };
        if bytes[1] & 127 != self.payload_type {
            return vec![bytes.to_vec()];
        }
        if self.ssrc != Some(packet.ssrc) {
            *self = Self::new(self.payload_type);
            self.ssrc = Some(packet.ssrc);
        }
        let next = *self.next.get_or_insert(packet.sequence);
        if (self.started && (packet.sequence.wrapping_sub(next) as i16) < 0)
            || self.queue.iter().any(|p| p.sequence == packet.sequence)
        {
            return self.poll(now);
        }
        self.queue.push(packet);
        self.poll(now)
    }
    pub(super) fn poll(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut output = Vec::new();
        while !self.queue.is_empty() {
            let expired = self
                .queue
                .iter()
                .any(|p| now.saturating_duration_since(p.received) >= REORDER_WAIT);
            if !self.started {
                if !expired && self.queue.len() < MAX_QUEUE {
                    break;
                }
                let base = self.next.unwrap();
                self.next = self
                    .queue
                    .iter()
                    .min_by_key(|p| p.sequence.wrapping_sub(base) as i16)
                    .map(|p| p.sequence);
                self.started = true;
            }
            let next = self.next.unwrap();
            let index = self.queue.iter().position(|p| p.sequence == next);
            let index = match index {
                Some(index) => index,
                None if expired || self.queue.len() >= MAX_QUEUE => {
                    self.lose_frame();
                    self.queue
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, p)| p.sequence.wrapping_sub(next))
                        .unwrap()
                        .0
                }
                None => break,
            };
            let packet = self.queue.swap_remove(index);
            self.next = Some(packet.sequence.wrapping_add(1));
            self.accept(packet, &mut output);
        }
        output
    }
    fn parameter(&mut self, nalu: &[u8]) {
        // Parameter sets are small. Do not cache arbitrary large payloads.
        if nalu.len() > 4096 {
            return;
        }
        match nalu[0] & 31 {
            7 => self.sps = nalu.to_vec(),
            8 => self.pps = nalu.to_vec(),
            _ => {}
        }
    }
    fn emit(&mut self, mut bytes: Vec<u8>, output: &mut Vec<Vec<u8>>) {
        bytes[2..4].copy_from_slice(&self.output_sequence.to_be_bytes());
        self.output_sequence = self.output_sequence.wrapping_add(1);
        output.push(bytes);
    }
    fn accept(&mut self, packet: Packet, output: &mut Vec<Vec<u8>>) {
        let payload = &packet.bytes[packet.payload..packet.end];
        let kind = payload[0] & 31;
        if kind == 7 || kind == 8 {
            self.parameter(payload);
            return;
        }
        if matches!(kind, 6 | 9) {
            return;
        }
        if self
            .frame
            .first()
            .is_some_and(|p| p.timestamp != packet.timestamp)
        {
            self.lose_frame();
        }
        let mut keyframe = false;
        match kind {
            1 | 5 if self.fragment.is_none() => {
                keyframe = kind == 5;
            }
            24 if self.fragment.is_none() => {
                let mut rest = &payload[1..];
                while !rest.is_empty() {
                    if rest.len() < 2 {
                        self.lose_frame();
                        return;
                    }
                    let size = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
                    if size == 0 || size + 2 > rest.len() {
                        self.lose_frame();
                        return;
                    }
                    let nalu = &rest[2..size + 2];
                    self.parameter(nalu);
                    keyframe |= nalu[0] & 31 == 5;
                    rest = &rest[size + 2..];
                }
            }
            28 if payload.len() >= 3 => {
                let nal = payload[1] & 31;
                let start = payload[1] & 128 != 0;
                let end = payload[1] & 64 != 0;
                if !matches!(nal, 1 | 5)
                    || (start && end)
                    || (start && self.fragment.is_some())
                    || (!start && self.fragment != Some(nal))
                {
                    self.lose_frame();
                    return;
                }
                keyframe = nal == 5;
                self.fragment = if end { None } else { Some(nal) };
            }
            _ => {
                self.lose_frame();
                return;
            }
        }
        self.keyframe |= keyframe;
        if self.need_keyframe && !self.keyframe {
            self.lose_frame();
            return;
        }
        self.frame_bytes += packet.bytes.len();
        if self.frame_bytes > MAX_FRAME_BYTES || self.frame.len() >= 4096 {
            self.lose_frame();
            return;
        }
        let marker = packet.bytes[1] & 128 != 0;
        self.frame.push(packet);
        if !marker {
            return;
        }
        if self.fragment.is_some() {
            self.lose_frame();
            return;
        }
        if self.keyframe {
            if self.sps.is_empty() || self.pps.is_empty() {
                self.lose_frame();
                return;
            }
            // Supply the actual in-band configuration before each complete IDR.
            // This also prevents the engine from inserting stale SDP parameters.
            let base = &self.frame[0].bytes;
            let mut header = base[..12].to_vec();
            header[0] = 128;
            header[1] &= 127;
            for parameter in [self.sps.clone(), self.pps.clone()] {
                let mut bytes = header.clone();
                bytes.extend(parameter);
                self.emit(bytes, output);
            }
        }
        let frame = std::mem::take(&mut self.frame);
        for packet in frame {
            self.emit(packet.bytes, output);
        }
        self.frame_bytes = 0;
        self.keyframe = false;
        self.need_keyframe = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut b = vec![128, 96 | if marker { 128 } else { 0 }];
        b.extend(seq.to_be_bytes());
        b.extend(ts.to_be_bytes());
        b.extend(1u32.to_be_bytes());
        b.extend(payload);
        b
    }
    fn configured() -> (H264Rtp, Instant) {
        let now = Instant::now();
        let mut r = H264Rtp::new(96);
        r.push(&packet(0, 0, false, &[0x67, 1]), now);
        r.push(&packet(1, 0, false, &[0x68, 2]), now);
        r.poll(now + REORDER_WAIT);
        (r, now + REORDER_WAIT)
    }
    #[test]
    fn reorders_fragments_and_drops_duplicates() {
        let (mut r, now) = configured();
        assert!(r.push(&packet(3, 10, false, &[28, 5, 2]), now).is_empty());
        assert!(r
            .push(&packet(2, 10, false, &[28, 128 | 5, 1]), now)
            .is_empty());
        assert!(r.push(&packet(3, 10, false, &[28, 5, 2]), now).is_empty());
        let out = r.push(&packet(4, 10, true, &[28, 64 | 5, 3]), now);
        assert_eq!(out.len(), 5); // SPS, PPS and the complete IDR
        assert_eq!(
            out.iter().skip(2).map(|p| p[14]).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        for (i, p) in out.iter().enumerate() {
            assert_eq!(u16::from_be_bytes([p[2], p[3]]), i as u16);
        }
    }
    #[test]
    fn loss_drops_broken_picture_and_dependents_until_next_idr() {
        let (mut r, now) = configured();
        assert_eq!(r.push(&packet(2, 10, true, &[0x65, 1]), now).len(), 3);
        r.push(&packet(3, 20, false, &[28, 128 | 1, 1]), now);
        r.push(&packet(5, 20, true, &[28, 64 | 1, 3]), now);
        assert!(r.poll(now + REORDER_WAIT).is_empty());
        assert!(r
            .push(&packet(6, 30, true, &[0x61, 1]), now + REORDER_WAIT)
            .is_empty());
        assert_eq!(
            r.push(&packet(7, 40, true, &[0x65, 1]), now + REORDER_WAIT)
                .len(),
            3
        );
    }
    #[test]
    fn startup_reordering_and_sequence_wrap_are_supported() {
        let now = Instant::now();
        let mut r = H264Rtp::new(96);
        r.push(&packet(0, 1, true, &[0x65, 1]), now);
        r.push(&packet(65534, 1, false, &[0x67, 1]), now);
        r.push(&packet(65535, 1, false, &[0x68, 1]), now);
        assert_eq!(r.poll(now + REORDER_WAIT).len(), 3);
        assert_eq!(
            r.push(&packet(1, 2, true, &[0x61, 1]), now + REORDER_WAIT)
                .len(),
            1
        );
    }
    #[test]
    fn malformed_packets_and_incomplete_frames_are_not_emitted() {
        let (mut r, now) = configured();
        assert!(r.push(&[128, 96], now).is_empty());
        assert!(r
            .push(&packet(2, 10, true, &[28, 128 | 5, 1]), now)
            .is_empty());
        assert!(r.push(&packet(3, 20, true, &[24, 0, 9, 1]), now).is_empty());
        assert_eq!(r.push(&packet(4, 30, true, &[0x65, 1]), now).len(), 3);
    }
}
