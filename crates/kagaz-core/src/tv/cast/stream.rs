//! One Cast Streaming stream, the sender's side: each encoded picture is
//! encrypted (AES-128 in counter mode, a nonce from the picture's number),
//! cut into packets with Cast's header, and kept until the TV says it has
//! it. The TV's reports say which pictures arrived whole and which packets
//! are missing; those go again. A report from here every half second
//! tells the TV which moment each picture belongs to.
//!
//! No sockets here: the session hands packets in and out.

use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The most a packet may be: an Ethernet frame less the IP and UDP headers.
pub const PACKET: usize = 1500 - 20 - 8;
/// The RTP header and Cast's own.
const HEADER: usize = 19;
/// The picture clock: 90,000 ticks a second.
pub const CLOCK: u32 = 90_000;
/// In a missing-packet note: none of the picture's packets came.
const ALL_LOST: u16 = 0xffff;
/// Picture numbers travel as 8 bits, so no more than this many may wait
/// for the TV's word at once.
const MOST_UNANSWERED: i64 = 120;
/// How often a report goes to the TV.
const REPORT_EVERY: Duration = Duration::from_millis(500);

/// What both sides agreed for the stream.
#[derive(Debug, Clone)]
pub struct Agreed {
    /// This sender's number, and the TV's for its reports.
    pub ssrc: u32,
    pub tv_ssrc: u32,
    pub key: [u8; 16],
    pub iv_mask: [u8; 16],
    /// Cast's number for the codec (101 for H.264, 127 for Opus).
    pub payload_type: u8,
    /// The stream's clock, ticks a second: 90,000 for pictures, the
    /// sample rate for sound.
    pub clock: u32,
    /// How long after its moment the TV shows a picture.
    pub delay: Duration,
}

/// Encrypts (or decrypts: the same) one picture's bytes in place.
pub fn crypt(key: &[u8; 16], iv_mask: &[u8; 16], picture: i64, data: &mut [u8]) {
    let aes = aes::Aes128::new(GenericArray::from_slice(key));
    let mut counter = [0u8; 16];
    counter[8..12].copy_from_slice(&(picture as u32).to_be_bytes());
    for (c, m) in counter.iter_mut().zip(iv_mask) {
        *c ^= m;
    }
    for chunk in data.chunks_mut(16) {
        let mut block = GenericArray::clone_from_slice(&counter);
        aes.encrypt_block(&mut block);
        for (d, k) in chunk.iter_mut().zip(block.iter()) {
            *d ^= k;
        }
        // The counter is one 128-bit number, most significant byte first.
        for b in counter.iter_mut().rev() {
            *b = b.wrapping_add(1);
            if *b != 0 {
                break;
            }
        }
    }
}

/// A picture the TV has not yet said it has.
struct Waiting {
    number: i64,
    rtp: u32,
    key: bool,
    /// Encrypted.
    data: Vec<u8>,
    /// Per packet: must it go (again), and when it last went.
    go: Vec<bool>,
    went: Vec<Option<Instant>>,
}

impl Waiting {
    fn packets(&self) -> usize {
        self.go.len()
    }
}

/// How the stream is doing, for the owner's numbers.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tally {
    pub packets: u64,
    pub bytes: u64,
    /// Packets sent again because the TV missed them.
    pub again: u64,
    /// The newest picture's last packet sent once more, the TV not having
    /// said it has it yet (no sign of trouble: the TV's word was not due).
    pub nudges: u64,
    /// Times the TV asked for a whole picture.
    pub lost_pictures: u64,
}

pub struct Sender {
    agreed: Agreed,
    sequence: u16,
    /// The next picture's number; the first is 0.
    next: i64,
    /// Every picture up to this one has arrived whole (-1: none yet).
    checkpoint: i64,
    /// The newest picture the TV has shown it knows of.
    known: i64,
    waiting: VecDeque<Waiting>,
    /// The round trip to the TV and back; zero until measured.
    round_trip: Duration,
    start: Instant,
    /// Seconds since 1900 at `start`, for the reports' clock.
    ntp_start: u64,
    /// The newest picture's clock and moment, for the reports.
    newest: Option<(u32, Instant)>,
    last_report: Option<Instant>,
    packets_sent: u32,
    octets_sent: u32,
    last_key: i64,
    picture_lost_at: i64,
    key_asked: Option<Instant>,
    key_wanted: bool,
    pub tally: Tally,
}

impl Sender {
    pub fn new(agreed: Agreed) -> Sender {
        let since_1970 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        // Any start does for the sequence; other senders start at random.
        let sequence = (since_1970 as u16) ^ (agreed.ssrc as u16);
        Sender {
            agreed,
            sequence,
            next: 0,
            checkpoint: -1,
            known: -1,
            waiting: VecDeque::new(),
            round_trip: Duration::ZERO,
            start: Instant::now(),
            ntp_start: since_1970 + 2_208_988_800,
            newest: None,
            last_report: None,
            packets_sent: 0,
            octets_sent: 0,
            last_key: -1,
            picture_lost_at: -1,
            key_asked: None,
            key_wanted: false,
            tally: Tally::default(),
        }
    }

    /// The same clock as another stream's, so the TV can keep sound and
    /// pictures in step: its reports tell each stream's moments by it.
    pub fn same_clock_as(mut self, other: &Sender) -> Sender {
        self.start = other.start;
        self.ntp_start = other.ntp_start;
        self
    }

    /// How long after its moment the TV plays what this stream carries.
    pub fn delay(&self) -> Duration {
        self.agreed.delay
    }

    pub fn round_trip(&self) -> Duration {
        self.round_trip
    }

    /// Pictures the TV has not answered for yet.
    pub fn unanswered(&self) -> usize {
        self.waiting.len()
    }

    /// Whether a picture stamped `rtp` may be queued now. When the TV falls
    /// behind (a weak link), pictures are skipped before they are made, so
    /// every one made is sent.
    pub fn room(&self, rtp: u32) -> bool {
        self.room_within(rtp, None)
    }

    /// The same, with the most time the waiting pictures may cover given
    /// (sound waits up to its whole delay: a skipped packet is a gap that
    /// is heard, and the TV itself drops what comes too late).
    pub fn room_within(&self, rtp: u32, most: Option<Duration>) -> bool {
        if self.next - self.checkpoint > MOST_UNANSWERED {
            return false;
        }
        // The time the waiting pictures cover, a pause counted as 50 ms at
        // most (a still screen sends few pictures, far apart).
        let clock = self.agreed.clock as u64;
        let ticks = |d: Duration| (d.as_micros() as u64 * clock / 1_000_000) as u32;
        let most_step = ticks(Duration::from_millis(50));
        let mut covered = 0u32;
        let mut it = self.waiting.iter().peekable();
        while let Some(w) = it.next() {
            let until = it.peek().map(|n| n.rtp).unwrap_or(rtp);
            covered += until.wrapping_sub(w.rtp).min(most_step);
        }
        // Two round trips (one for a missed packet's note and resend), at
        // least two pictures at 30 a second and a little, at most two
        // thirds of the delay: the oldest then still has time to arrive.
        let floor = Duration::from_micros(81_667);
        let most = most.unwrap_or_else(|| (self.round_trip * 2).clamp(floor, floor.max(self.agreed.delay * 2 / 3)));
        covered <= ticks(most)
    }

    /// Queues one encoded picture (Annex B), stamped `rtp` on the picture
    /// clock, whose moment was `when`.
    pub fn queue(&mut self, picture: &[u8], key: bool, rtp: u32, when: Instant) {
        let number = self.next;
        self.next += 1;
        let mut data = picture.to_vec();
        crypt(&self.agreed.key, &self.agreed.iv_mask, number, &mut data);
        let packets = data.len().div_ceil(PACKET - HEADER).max(1);
        self.waiting.push_back(Waiting { number, rtp, key, data, go: vec![true; packets], went: vec![None; packets] });
        if key {
            self.last_key = number;
        }
        let when = match self.newest {
            Some((_, last)) => when.max(last),
            None => when,
        };
        self.newest = Some((rtp, when));
    }

    /// Whether the TV lost the picture and needs a whole one; true once
    /// per asking, and at most twice a second.
    pub fn take_key_request(&mut self, now: Instant) -> bool {
        if !std::mem::take(&mut self.key_wanted) {
            return false;
        }
        if self.key_asked.is_some_and(|t| now.duration_since(t) < Duration::from_millis(500)) {
            return false;
        }
        self.key_asked = Some(now);
        true
    }

    /// The report due now, if one is: before the first picture's packets,
    /// then every half second.
    pub fn report(&mut self, now: Instant) -> Option<Vec<u8>> {
        let (rtp, when) = self.newest?;
        if self.last_report.is_some_and(|t| now.duration_since(t) < REPORT_EVERY) {
            return None;
        }
        self.last_report = Some(now);
        // The picture clock at this very moment.
        let since = now.saturating_duration_since(when);
        let rtp_now = rtp.wrapping_add((since.as_micros() as u64 * self.agreed.clock as u64 / 1_000_000) as u32);
        let mut p = Vec::with_capacity(28);
        p.extend([0x80, 200, 0, 6]);
        p.extend(self.agreed.ssrc.to_be_bytes());
        p.extend(self.ntp(now).to_be_bytes());
        p.extend(rtp_now.to_be_bytes());
        p.extend(self.packets_sent.to_be_bytes());
        p.extend(self.octets_sent.to_be_bytes());
        Some(p)
    }

    /// The reports' clock: seconds since 1900 and a 32-bit fraction.
    fn ntp(&self, at: Instant) -> u64 {
        let since = at.saturating_duration_since(self.start);
        let fraction = (since.subsec_nanos() as u64) << 32;
        ((self.ntp_start + since.as_secs()) << 32) | (fraction / 1_000_000_000)
    }

    /// The next packet to send now: the oldest that has not gone or must
    /// go again; else, when the TV may not know of the newest picture, its
    /// last packet once more.
    pub fn packet(&mut self, now: Instant) -> Option<Vec<u8>> {
        let mut chosen = None;
        let mut nudge = false;
        for (i, w) in self.waiting.iter().enumerate() {
            if let Some(p) = w.go.iter().position(|g| *g) {
                chosen = Some((i, p));
                break;
            }
        }
        if chosen.is_none() {
            let w = self.waiting.back()?;
            if self.known >= w.number {
                return None;
            }
            let p = w.packets() - 1;
            let every = (self.agreed.delay / 10).max(self.round_trip * 2);
            if w.went[p].is_some_and(|t| now.duration_since(t) < every) {
                return None;
            }
            chosen = Some((self.waiting.len() - 1, p));
            nudge = true;
        }
        let (i, p) = chosen?;
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        let w = &mut self.waiting[i];
        let most = PACKET - HEADER;
        let from = p * most;
        let chunk = &w.data[from.min(w.data.len())..(from + most).min(w.data.len())];
        let last = p == w.packets() - 1;
        let mut out = Vec::with_capacity(HEADER + chunk.len());
        let marker = if last { 0x80 } else { 0 };
        let whole = if w.key { 0x80 } else { 0 };
        let builds_on = if w.key { w.number } else { w.number - 1 };
        out.push(0x80);
        out.push(marker | self.agreed.payload_type);
        out.extend(sequence.to_be_bytes());
        out.extend(w.rtp.to_be_bytes());
        out.extend(self.agreed.ssrc.to_be_bytes());
        // Cast's part: whole picture or not, "the picture it builds on
        // follows", the picture's number, this packet's and the last's,
        // and the picture it builds on (itself for a whole one).
        out.push(whole | 0x40);
        out.push(w.number as u8);
        out.extend((p as u16).to_be_bytes());
        out.extend(((w.packets() - 1) as u16).to_be_bytes());
        out.push(builds_on as u8);
        out.extend(chunk);
        if nudge {
            self.tally.nudges += 1;
        } else if w.went[p].is_some() {
            self.tally.again += 1;
        }
        w.go[p] = false;
        w.went[p] = Some(now);
        self.packets_sent = self.packets_sent.wrapping_add(1);
        self.octets_sent = self.octets_sent.wrapping_add(chunk.len() as u32);
        self.tally.packets += 1;
        self.tally.bytes += out.len() as u64;
        Some(out)
    }

    fn drop_waiting(&mut self, number: i64) {
        if let Some(i) = self.waiting.iter().position(|w| w.number == number) {
            self.waiting.remove(i);
        }
    }

    /// A report from the TV (one packet holding several notes).
    pub fn heard(&mut self, mut packet: &[u8], now: Instant) {
        let u32_at = |b: &[u8], i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        while packet.len() >= 4 {
            if packet[0] >> 5 != 0b100 {
                return;
            }
            let (subtype, kind) = (packet[0] & 0x1f, packet[1]);
            let size = u16::from_be_bytes([packet[2], packet[3]]) as usize * 4;
            if packet.len() < 4 + size {
                return;
            }
            let body = &packet[4..4 + size];
            packet = &packet[4 + size..];
            match (kind, subtype) {
                // The TV's reception report: how long the round trip is.
                (201, blocks) if body.len() >= 4 && u32_at(body, 0) == self.agreed.tv_ssrc => {
                    for block in body[4..].chunks_exact(24).take(blocks as usize) {
                        if u32_at(block, 0) != self.agreed.ssrc {
                            continue;
                        }
                        let (last_report, held) = (u32_at(block, 16), u32_at(block, 20));
                        if last_report == 0 || self.last_report.is_none() {
                            continue;
                        }
                        // In 1/65536 s: now, less when our report left,
                        // less how long the TV held it.
                        let middle = (self.ntp(now) >> 16) as u32;
                        let ticks = middle.wrapping_sub(last_report).wrapping_sub(held) as i32;
                        let measured = if ticks <= 0 {
                            Duration::from_micros(75)
                        } else {
                            Duration::from_micros(ticks as u64 * 1_000_000 / 65_536)
                        }
                        .min(self.agreed.delay);
                        self.round_trip = if self.round_trip.is_zero() {
                            measured
                        } else if measured > self.round_trip {
                            (self.round_trip + measured) / 2
                        } else {
                            (self.round_trip * 7 + measured) / 8
                        };
                    }
                }
                // The TV lost the picture: it needs a whole one, unless
                // one is already on its way.
                (206, 1) if body.len() >= 8 && u32_at(body, 0) == self.agreed.tv_ssrc && u32_at(body, 4) == self.agreed.ssrc => {
                    if self.checkpoint >= self.last_key {
                        self.picture_lost_at = self.checkpoint.max(self.last_key);
                        self.key_wanted = true;
                        self.tally.lost_pictures += 1;
                    }
                }
                (206, 15) => self.feedback(body, now),
                _ => {}
            }
        }
    }

    /// Cast's own note: every picture up to one has arrived; these packets
    /// are missing; these later pictures arrived whole.
    fn feedback(&mut self, b: &[u8], now: Instant) {
        let u32_at = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        if b.len() < 16 || u32_at(0) != self.agreed.tv_ssrc || u32_at(4) != self.agreed.ssrc || &b[8..12] != b"CAST" {
            return;
        }
        // The picture numbers come as their low 8 bits. The checkpoint is
        // the newest such number not after the newest picture sent.
        let newest = self.next - 1;
        let mut checkpoint = (newest & !0xff) | b[12] as i64;
        if checkpoint > newest {
            checkpoint -= 256;
        }
        if checkpoint < self.checkpoint {
            return;
        }
        while self.waiting.front().is_some_and(|w| w.number <= checkpoint) {
            self.waiting.pop_front();
        }
        self.checkpoint = checkpoint;
        self.known = self.known.max(checkpoint);
        let after = |low: u8| {
            let mut n = (checkpoint & !0xff) | low as i64;
            if n <= checkpoint {
                n += 256;
            }
            n
        };
        // A packet that left less than a round trip ago may be the one the
        // note is about, still on its way: it does not go again. On a fast
        // link the round trip is next to nothing, so at least 10 ms.
        let round_trip = self.round_trip.max(Duration::from_millis(10));
        let old_enough = |went: Option<Instant>| match went {
            Some(t) => now.duration_since(t) >= round_trip,
            None => false,
        };
        let notes = b[13] as usize;
        let mut rest = &b[16..];
        for _ in 0..notes {
            if rest.len() < 4 {
                return;
            }
            let (number, first, more) = (after(rest[0]), u16::from_be_bytes([rest[1], rest[2]]), rest[3]);
            rest = &rest[4..];
            let Some(w) = self.waiting.iter_mut().find(|w| w.number == number) else { continue };
            self.known = self.known.max(number);
            if first == ALL_LOST {
                for p in 0..w.packets() {
                    if old_enough(w.went[p]) {
                        w.go[p] = true;
                    }
                }
                continue;
            }
            // The packet named, and a bit for each of the next eight.
            let missing = std::iter::once(first as usize).chain((0..8).filter(|i| more >> i & 1 == 1).map(|i| first as usize + 1 + i));
            for p in missing {
                if p < w.packets() && old_enough(w.went[p]) {
                    w.go[p] = true;
                }
            }
        }
        // Then, perhaps, the pictures after the checkpoint that arrived
        // whole: a bit each, the first standing for the checkpoint plus two.
        if rest.len() >= 6 && &rest[..4] == b"CST2" {
            let octets = rest[5] as usize;
            for (i, byte) in rest[6..].iter().take(octets).enumerate() {
                for bit in 0..8 {
                    if byte >> bit & 1 == 1 {
                        let number = checkpoint + 2 + (i * 8 + bit) as i64;
                        if number < self.next {
                            self.drop_waiting(number);
                            self.known = self.known.max(number);
                        }
                    }
                }
            }
        }
    }
}

/// A stand-in for the TV's side of one stream, for tests and for checking
/// a sender: it puts pictures together from packets, answers as a Cast
/// receiver does, and hands out each whole picture in order.
pub struct Receiver {
    agreed: Agreed,
    /// Packets of pictures not yet whole: number -> (packets, how many are in).
    parts: std::collections::BTreeMap<i64, Vec<Option<Vec<u8>>>>,
    /// The newest picture handed out.
    checkpoint: i64,
    /// From the sender's last report: its clock's middle bits, and when it came.
    last_report: Option<(u32, Instant)>,
    whole: VecDeque<Vec<u8>>,
    /// Packets that came for pictures already handed out, or twice.
    pub extra: u64,
}

impl Receiver {
    pub fn new(agreed: Agreed) -> Receiver {
        Receiver { agreed, parts: Default::default(), checkpoint: -1, last_report: None, whole: VecDeque::new(), extra: 0 }
    }

    /// A packet from the sender, a picture's part or a report.
    pub fn take(&mut self, p: &[u8], now: Instant) {
        let its = |at: usize| p.len() >= at + 4 && p[at..at + 4] == self.agreed.ssrc.to_be_bytes();
        if p.len() >= 28 && p[1] == 200 {
            if !its(4) {
                return;
            }
            let ntp = u64::from_be_bytes(p[8..16].try_into().unwrap());
            self.last_report = Some(((ntp >> 16) as u32, now));
            return;
        }
        if p.len() < HEADER || p[0] != 0x80 || p[1] & 0x7f != self.agreed.payload_type || !its(8) {
            return;
        }
        let low = p[13];
        // The picture's full number: the one nearest after the checkpoint.
        let mut number = (self.checkpoint & !0xff) | low as i64;
        if number <= self.checkpoint {
            number += 256;
        }
        if number - self.checkpoint > 200 {
            self.extra += 1;
            return;
        }
        let (id, last) = (u16::from_be_bytes([p[14], p[15]]) as usize, u16::from_be_bytes([p[16], p[17]]) as usize);
        let parts = self.parts.entry(number).or_insert_with(|| vec![None; last + 1]);
        if id >= parts.len() || parts[id].is_some() {
            self.extra += 1;
            return;
        }
        parts[id] = Some(p[HEADER..].to_vec());
        // Hand out every picture that is whole and next in line.
        while let Some(parts) = self.parts.get(&(self.checkpoint + 1)) {
            if parts.iter().any(Option::is_none) {
                break;
            }
            let number = self.checkpoint + 1;
            let mut data: Vec<u8> = self.parts.remove(&number).unwrap().into_iter().flatten().flatten().collect();
            crypt(&self.agreed.key, &self.agreed.iv_mask, number, &mut data);
            self.whole.push_back(data);
            self.checkpoint = number;
        }
    }

    /// The receiver's own number, for its answer to the offer.
    pub fn tv_ssrc(&self) -> u32 {
        self.agreed.tv_ssrc
    }

    /// The next whole picture (Annex B), in order.
    pub fn picture(&mut self) -> Option<Vec<u8>> {
        self.whole.pop_front()
    }

    /// The report a receiver sends back: reception, then Cast's note with
    /// the checkpoint and the packets still missing.
    pub fn report(&self, now: Instant) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend([0x81, 201, 0, 7]);
        p.extend(self.agreed.tv_ssrc.to_be_bytes());
        p.extend(self.agreed.ssrc.to_be_bytes());
        p.extend([0u8; 12]);
        let (last, held) = match self.last_report {
            Some((middle, at)) => (middle, (now.duration_since(at).as_micros() as u64 * 65_536 / 1_000_000) as u32),
            None => (0, 0),
        };
        p.extend(last.to_be_bytes());
        p.extend(held.to_be_bytes());
        let mut notes = Vec::new();
        // A picture none of whose packets came, before one that is known.
        let newest = self.parts.keys().next_back().copied().unwrap_or(self.checkpoint);
        for number in self.checkpoint + 1..newest {
            if !self.parts.contains_key(&number) && notes.len() < 255 * 4 {
                notes.push(number as u8);
                notes.extend(ALL_LOST.to_be_bytes());
                notes.push(0);
            }
        }
        for (number, parts) in &self.parts {
            // Only packets that are behind: before the last one come of
            // this picture, or all of it once a later picture has begun.
            let later = *number < newest;
            let upto = if later { parts.len() } else { parts.iter().rposition(Option::is_some).unwrap_or(0) };
            let mut i = 0;
            while i < upto && notes.len() < 255 * 4 {
                if parts[i].is_some() {
                    i += 1;
                    continue;
                }
                let mut more = 0u8;
                for bit in 0..8 {
                    if i + 1 + bit < upto && parts.get(i + 1 + bit).is_some_and(Option::is_none) {
                        more |= 1 << bit;
                    }
                }
                notes.push(*number as u8);
                notes.extend((i as u16).to_be_bytes());
                notes.push(more);
                i += 9;
            }
        }
        let words = (12 + 4 + notes.len()) / 4;
        p.extend([0x80 | 15, 206]);
        p.extend((words as u16).to_be_bytes());
        p.extend(self.agreed.tv_ssrc.to_be_bytes());
        p.extend(self.agreed.ssrc.to_be_bytes());
        p.extend(b"CAST");
        p.push(self.checkpoint as u8);
        p.push((notes.len() / 4) as u8);
        p.extend((self.agreed.delay.as_millis() as u16).to_be_bytes());
        p.extend(notes);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agreed() -> Agreed {
        Agreed { ssrc: 60_002, tv_ssrc: 60_003, key: *b"0123456789abcdef", iv_mask: *b"fedcba9876543210", payload_type: 101, clock: CLOCK, delay: Duration::from_millis(200) }
    }

    #[test]
    fn the_cipher_is_aes_128_counter_mode_with_the_picture_number_in_the_nonce() {
        // AES-128-CTR, NIST SP 800-38A F.5.1's key and first block, reached
        // by choosing the mask so the nonce for picture 0 is the vector's.
        let key = [0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
        let nonce = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe, 0xff];
        let mut data = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a, 0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03,
            0xac, 0x9c, 0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51,
        ];
        crypt(&key, &nonce, 0, &mut data);
        assert_eq!(
            data,
            [
                0x87, 0x4d, 0x61, 0x91, 0xb6, 0x20, 0xe3, 0x26, 0x1b, 0xef, 0x68, 0x64, 0x99, 0x0d, 0xb6, 0xce, 0x98, 0x06, 0xf6, 0x6b, 0x79, 0x70,
                0xfd, 0xff, 0x86, 0x17, 0x18, 0x7b, 0xb9, 0xff, 0xfd, 0xff
            ]
        );
        // Another picture: another nonce (bytes 8 to 11 carry its number).
        let mut a = *b"the same sixteen";
        let mut b = a;
        crypt(&key, &nonce, 1, &mut a);
        crypt(&key, &nonce, 2, &mut b);
        assert_ne!(a, b);
    }

    #[test]
    fn a_packet_carries_casts_header() {
        let mut s = Sender::new(agreed());
        let now = Instant::now();
        s.queue(&vec![7u8; 3000], true, 9000, now);
        let report = s.report(now).unwrap();
        assert_eq!((report[1], report.len()), (200, 28));
        let first = s.packet(now).unwrap();
        assert_eq!(first.len(), PACKET);
        assert_eq!((first[0], first[1]), (0x80, 101));
        assert_eq!(u32::from_be_bytes(first[4..8].try_into().unwrap()), 9000);
        assert_eq!(u32::from_be_bytes(first[8..12].try_into().unwrap()), 60_002);
        // A whole picture, number 0, packet 0 of 0..=2, building on itself.
        assert_eq!(first[12..19], [0xc0, 0, 0, 0, 0, 2, 0]);
        s.packet(now).unwrap();
        let last = s.packet(now).unwrap();
        assert_eq!(last[1], 0x80 | 101);
        assert_eq!(last.len(), HEADER + 3000 - 2 * (PACKET - HEADER));
        // The next picture builds on the one before.
        s.queue(&[1, 2, 3], false, 12_000, now);
        let p = s.packet(now).unwrap();
        assert_eq!(p[12..19], [0x40, 1, 0, 0, 0, 0, 0]);
    }

    /// Pictures of every size through a link that loses packets: each one
    /// arrives whole, in order, and the same.
    #[test]
    fn every_picture_arrives_through_a_link_that_loses_packets() {
        let mut s = Sender::new(agreed());
        let mut r = Receiver::new(agreed());
        let start = Instant::now();
        let mut now = start;
        let mut sent = Vec::new();
        let mut got = Vec::new();
        let mut on_the_wire = 0u64;
        let mut skipped = 0;
        // 400 pictures, more than the 8-bit numbers hold, 60 a second.
        for n in 0..400u32 {
            let rtp = n * 1500;
            let picture: Vec<u8> = (0..(n as usize * 97) % 9000 + 1).map(|i| (i as u32 * 31 + n) as u8).collect();
            if s.room(rtp) {
                s.queue(&picture, n % 100 == 0, rtp, now);
                sent.push(picture);
            } else {
                skipped += 1;
            }
            // A picture's time in millisecond steps: packets out (every
            // 13th lost), the receiver's report back every 5 ms.
            for ms in 0..17 {
                now += Duration::from_millis(1);
                if let Some(p) = s.report(now) {
                    r.take(&p, now);
                }
                for _ in 0..8 {
                    let Some(p) = s.packet(now) else { break };
                    on_the_wire += 1;
                    if on_the_wire % 13 != 0 {
                        r.take(&p, now);
                    }
                }
                if ms % 5 == 0 {
                    s.heard(&r.report(now), now);
                }
                while let Some(p) = r.picture() {
                    got.push(p);
                }
            }
        }
        // The rest drains.
        for _ in 0..500 {
            now += Duration::from_millis(1);
            while let Some(p) = s.packet(now) {
                r.take(&p, now);
            }
            s.heard(&r.report(now), now);
            while let Some(p) = r.picture() {
                got.push(p);
            }
        }
        assert_eq!(got.len(), sent.len());
        assert!(got == sent, "a picture arrived changed");
        assert_eq!(s.unanswered(), 0);
        assert!(s.tally.again > 0, "lost packets were sent again");
        assert!(skipped < 40, "{skipped} pictures skipped on a link losing 1 in 13");
        assert!(!s.round_trip().is_zero());
    }

    #[test]
    fn a_lost_picture_asks_for_a_whole_one_once() {
        let mut s = Sender::new(agreed());
        let now = Instant::now();
        s.queue(&[1; 10], true, 0, now);
        s.report(now);
        while s.packet(now).is_some() {}
        let mut tv = Receiver::new(agreed());
        tv.checkpoint = 0;
        s.heard(&tv.report(now), now);
        assert_eq!(s.unanswered(), 0);
        let mut lost = vec![0x80 | 1, 206, 0, 2];
        lost.extend(60_003u32.to_be_bytes());
        lost.extend(60_002u32.to_be_bytes());
        s.heard(&lost, now);
        s.heard(&lost, now);
        assert!(s.take_key_request(now));
        assert!(!s.take_key_request(now));
        // Asked again at once: not yet.
        s.heard(&lost, now + Duration::from_millis(100));
        assert!(!s.take_key_request(now + Duration::from_millis(100)));
        // A whole picture on its way: no more asking.
        s.queue(&[1; 10], true, 3000, now);
        s.heard(&lost, now + Duration::from_secs(1));
        assert!(!s.take_key_request(now + Duration::from_secs(1)));
    }
}
