//! The stream's rate fitted to the link, from what the TV reports: once a
//! second, trouble on the link (packets the TV had to ask for again, more
//! than 2 in 100; a whole picture asked for; pictures held back because
//! the TV was behind) takes the rate down by 30 %; three clean seconds
//! in a row take it up by 15 %, never above the rate the session started
//! with nor below an eighth of it. A computer too slow to make every
//! picture is not the link's fault and does not count.

use std::time::{Duration, Instant};

/// The counts so far in the session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub packets: u64,
    pub again: u64,
    pub whole_asked: u64,
    pub held_back: u64,
}

pub struct Fit {
    pub rate: u32,
    top: u32,
    floor: u32,
    last: Counts,
    checked: Instant,
    clean: u32,
}

impl Fit {
    pub fn new(top: u32, now: Instant) -> Fit {
        Fit { rate: top, top, floor: top / 8, last: Counts::default(), checked: now, clean: 0 }
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
        };
        self.last = c;
        let trouble = d.held_back > 0 || d.whole_asked > 0 || d.again * 50 > d.packets.max(1);
        let before = self.rate;
        if trouble {
            self.clean = 0;
            self.rate = (self.rate as u64 * 7 / 10).max(self.floor as u64) as u32;
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
        let mut at = |s: u64| t0 + Duration::from_secs(s);
        // A clean link at the top: nothing changes.
        for s in 1..=5 {
            c.packets += 1000;
            assert_eq!(f.check(at(s), c), None);
        }
        // Five packets in a hundred asked for again: down 30 %.
        c.packets += 1000;
        c.again += 50;
        assert_eq!(f.check(at(6), c), Some(11_200_000));
        // Within the second: nothing.
        assert_eq!(f.check(at(6) + Duration::from_millis(500), c), None);
        // Held back pictures: down again; then down to the floor at most.
        for s in 7..=20 {
            c.packets += 1000;
            c.held_back += 1;
            f.check(at(s), c);
        }
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
