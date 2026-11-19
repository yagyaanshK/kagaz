//! The stream's rate fitted to the link, from what the TV reports: once a
//! second, a full link takes the rate down by 30 %. A link is full when
//! its round trip grows 40 ms past the session's best (its queues
//! filling), or when packets are lost (more than 2 in 100 asked for
//! again, or a whole picture asked for) or pictures held back while the
//! stream uses at least half the rate. Losses while the stream uses far
//! less, its round trip steady, are the air's, not the rate's: lowering
//! a ceiling the stream is not near would only cost a whole picture. Three clean seconds in a row take
//! the rate up by 15 %; never above the rate the session started with
//! nor below an eighth of it. A computer too slow to make every picture
//! is not the link's fault and does not count.

use std::time::{Duration, Instant};

/// The counts so far in the session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub packets: u64,
    pub again: u64,
    pub whole_asked: u64,
    pub held_back: u64,
    /// Bytes sent, pictures and sound.
    pub bytes: u64,
    /// The round trip as it stands (zero until the TV's first report).
    pub round_trip: Duration,
}

pub struct Fit {
    pub rate: u32,
    top: u32,
    floor: u32,
    last: Counts,
    checked: Instant,
    clean: u32,
    best_round_trip: Option<Duration>,
}

impl Fit {
    pub fn new(top: u32, now: Instant) -> Fit {
        Fit { rate: top, top, floor: top / 8, last: Counts::default(), checked: now, clean: 0, best_round_trip: None }
    }

    /// Called often; once a second it looks at what changed and gives a
    /// new rate when the rate should change.
    pub fn check(&mut self, now: Instant, c: Counts) -> Option<u32> {
        if now.duration_since(self.checked) < Duration::from_secs(1) {
            return None;
        }
        self.checked = now;
        let d = Counts {
            packets: c.packets - self.last.packets,
            again: c.again - self.last.again,
            whole_asked: c.whole_asked - self.last.whole_asked,
            held_back: c.held_back - self.last.held_back,
            bytes: c.bytes - self.last.bytes,
            round_trip: c.round_trip,
        };
        self.last = c;
        let losses = d.whole_asked > 0 || d.again * 50 > d.packets.max(1);
        let near_the_rate = d.bytes * 8 * 2 >= self.rate as u64;
        let mut queues_filling = false;
        if !c.round_trip.is_zero() {
            let best = self.best_round_trip.get_or_insert(c.round_trip);
            *best = (*best).min(c.round_trip);
            queues_filling = c.round_trip > *best + Duration::from_millis(40);
        }
        let before = self.rate;
        if queues_filling || ((losses || d.held_back > 0) && near_the_rate) {
            self.clean = 0;
            self.rate = (self.rate as u64 * 7 / 10).max(self.floor as u64) as u32;
        } else if losses || d.held_back > 0 {
            // Not the rate's doing: held where it is.
            self.clean = 0;
        } else {
            self.clean += 1;
            if self.clean >= 3 {
                self.clean = 0;
                self.rate = (self.rate as u64 * 115 / 100).min(self.top as u64) as u32;
            }
        }
        (self.rate != before).then_some(self.rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn down_on_trouble_up_when_clean() {
        let t0 = Instant::now();
        let mut f = Fit::new(16_000_000, t0);
        let mut c = Counts::default();
        let at = |s: u64| t0 + Duration::from_secs(s);
        // A clean link at the top: nothing changes.
        for s in 1..=5 {
            c.packets += 1000;
            assert_eq!(f.check(at(s), c), None);
        }
        // Five packets in a hundred asked for again while the stream uses
        // little: the air's doing, the rate stays.
        c.packets += 1000;
        c.again += 50;
        c.bytes += 100_000;
        assert_eq!(f.check(at(6), c), None);
        // The same while the stream uses most of the rate: down 30 %.
        c.packets += 1000;
        c.again += 50;
        c.bytes += 1_800_000;
        assert_eq!(f.check(at(7), c), Some(11_200_000));
        // Within the second: nothing.
        assert_eq!(f.check(at(7) + Duration::from_millis(500), c), None);
        // The round trip growing past its best: queues filling, down; then
        // down to the floor at most.
        c.round_trip = Duration::from_millis(5);
        for s in 8..=20 {
            c.packets += 1000;
            c.round_trip = Duration::from_millis(if s == 8 { 5 } else { 80 });
            f.check(at(s), c);
        }
        c.round_trip = Duration::from_millis(5);
        assert_eq!(f.rate, 2_000_000);
        // One packet in a hundred asked again is no trouble; three clean
        // seconds take it up by 15 %.
        for s in 21..=23 {
            c.packets += 1000;
            c.again += 10;
            let r = f.check(at(s), c);
            assert_eq!(r, (s == 23).then_some(2_300_000));
        }
        // Clean for long: back to the top, never over.
        for s in 24..=100 {
            c.packets += 1000;
            f.check(at(s), c);
        }
        assert_eq!(f.rate, 16_000_000);
    }
}
