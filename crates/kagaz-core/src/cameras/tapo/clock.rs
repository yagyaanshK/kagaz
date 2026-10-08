//! When a camera's own clock was wrong, and by how much.
//!
//! After a power cut a Tapo camera stamps its first frames with a date from
//! its firmware, then restores the last time it remembers plus a minute, so
//! its recording list shows a pause of exactly 60 s. Normally it gets the
//! real time from the internet within moments and the list shows a second
//! jump forward, as long as the real outage. Without internet it keeps the
//! stale time for the rest of the day: everything after the restart is
//! labelled too early, and the burned-in clock agrees with the list, so
//! nothing on the camera shows it.
//!
//! A correction says "between these two camera times, add this many
//! seconds". Corrections come from the user ("the camera showed X when the
//! real time was Y") or from the clock watcher, and are kept beside the
//! day's footage in `clock.json`, so they travel with the recordings.
//! Times called *camera* are as the camera labels them; *real* ones are
//! corrected.

use super::cache;
use super::recordings::Clip;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The pause a restart leaves in the recording list, in seconds.
const RESTART_GAP: std::ops::RangeInclusive<i64> = 55..=65;
/// A forward jump within this long after a restart is the clock being set.
const CORRECTION_WITHIN: i64 = 600;
/// Gaps shorter than this are the camera's own seams between clips.
const SEAM: i64 = 20;
/// A camera clock this far off counts as wrong.
pub const WRONG_AFTER: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restart {
    /// Camera time the recording stopped (the clock was still right here).
    pub stopped: i64,
    /// Camera time it resumed, on the restored (possibly stale) clock.
    pub resumed: i64,
    /// Camera time the clock was set from the internet, and the jump it made.
    pub corrected_at: Option<i64>,
    pub jump: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correction {
    /// Camera times the correction covers: [from, until).
    pub from: i64,
    pub until: i64,
    /// Seconds to add to camera time to get the real time.
    pub offset: i64,
    /// "manual" or "watcher".
    pub source: String,
    /// What it was made from, for the user ("showed 12:42:00 at 13:57:00").
    #[serde(default)]
    pub note: String,
    /// The watcher still sees the clock wrong: `until` grows with each reading.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
}

/// One look at a camera's clock by the watcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    /// This computer's time (unix) and the camera's at that moment.
    pub at: i64,
    pub camera: i64,
}

/// What `clock.json` keeps for one camera day.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayClock {
    #[serde(default)]
    pub corrections: Vec<Correction>,
    #[serde(default)]
    pub readings: Vec<Reading>,
}

/// The restarts in one day's clip list (clips in any order).
pub fn restarts(clips: &[Clip]) -> Vec<Restart> {
    let mut sorted: Vec<&Clip> = clips.iter().collect();
    sorted.sort_by_key(|c| c.start);
    let gaps: Vec<(i64, i64)> = sorted
        .windows(2)
        .map(|w| (w[0].end, w[1].start))
        .filter(|(a, b)| b - a > SEAM)
        .collect();
    let mut out = Vec::new();
    for (i, &(stopped, resumed)) in gaps.iter().enumerate() {
        if !RESTART_GAP.contains(&(resumed - stopped)) {
            continue;
        }
        let fix = gaps
            .get(i + 1)
            .filter(|(a, _)| a - resumed < CORRECTION_WITHIN);
        out.push(Restart {
            stopped,
            resumed,
            corrected_at: fix.map(|&(_, b)| b),
            jump: fix.map(|&(a, b)| b - a),
        });
    }
    out
}

/// The correction for "the camera showed `shown` when the real time was
/// `real`": from the restart before that moment (or the day's first clip)
/// to the next restart or the end of the day's clips.
pub fn correction_for(clips: &[Clip], shown: i64, real: i64, note: String) -> Correction {
    let found = restarts(clips);
    let from = found
        .iter()
        .filter(|r| r.resumed <= shown)
        .map(|r| r.resumed)
        .max()
        .or_else(|| clips.iter().map(|c| c.start).min())
        .unwrap_or(shown)
        .min(shown);
    let until = found
        .iter()
        .filter(|r| r.stopped > shown)
        .map(|r| r.stopped)
        .min()
        .or_else(|| clips.iter().map(|c| c.end).max())
        .unwrap_or(shown)
        .max(shown + 1);
    Correction {
        from,
        until,
        offset: real - shown,
        source: "manual".into(),
        note,
        open: false,
    }
}

/// Take one watcher reading into a camera day's record. `clips` is that
/// day's recording list (camera times). While the camera's clock is wrong
/// the watcher's correction runs from the restart before the reading to the
/// latest wrong reading; once the clock is right again it ends where the
/// clock jumped (the first gap in the recordings at least as long as the
/// error), or at the last recording.
pub fn record_reading(day: &mut DayClock, reading: Reading, clips: &[Clip]) {
    day.readings.push(reading.clone());
    let off = reading.at - reading.camera;
    let wrong = off.abs() > WRONG_AFTER;
    let mut sorted: Vec<&Clip> = clips.iter().collect();
    sorted.sort_by_key(|c| c.start);
    // Close what is open unless this reading continues it.
    for c in day.corrections.iter_mut().filter(|c| c.open) {
        if wrong && (c.offset - off).abs() <= WRONG_AFTER {
            c.until = c.until.max(reading.camera + 1);
            return;
        }
        c.open = false;
        if c.offset > 0 {
            let jump = sorted
                .windows(2)
                .map(|w| (w[0].end, w[1].start))
                .find(|&(a, b)| a + 1 >= c.until && b - a >= c.offset - WRONG_AFTER);
            c.until = match jump {
                Some((a, _)) => a,
                None => sorted
                    .iter()
                    .map(|k| k.end)
                    .filter(|&e| e <= reading.camera)
                    .max()
                    .unwrap_or(c.until)
                    .max(c.until),
            };
        }
    }
    if !wrong {
        return;
    }
    let from = restarts(clips)
        .iter()
        .filter(|r| r.resumed <= reading.camera)
        .map(|r| r.resumed)
        .max()
        .or_else(|| {
            sorted
                .first()
                .map(|c| c.start)
                .filter(|&s| s <= reading.camera)
        })
        .unwrap_or(reading.camera);
    day.corrections.push(Correction {
        from,
        until: reading.camera + 1,
        offset: off,
        source: "watcher".into(),
        note: format!("the camera's clock was {off} s off"),
        open: true,
    });
}

/// All corrections known for a camera, across its days.
#[derive(Debug, Clone, Default)]
pub struct Corrections(pub Vec<Correction>);

impl Corrections {
    pub fn load(root: &Path, device_id: &str) -> Corrections {
        let dir = cache::day_dir(root, device_id, "");
        let mut all = Vec::new();
        if let Ok(days) = std::fs::read_dir(&dir) {
            for day in days.flatten() {
                if let Some(c) = read(&day.path().join("clock.json")) {
                    all.extend(c.corrections);
                }
            }
        }
        all.sort_by_key(|c| c.from);
        Corrections(all)
    }

    fn covering(&self, camera: i64) -> Option<&Correction> {
        self.0.iter().find(|c| c.from <= camera && camera < c.until)
    }

    /// The offset in force at a camera time.
    pub fn offset_at(&self, camera: i64) -> i64 {
        self.covering(camera).map_or(0, |c| c.offset)
    }

    pub fn to_real(&self, camera: i64) -> i64 {
        camera + self.offset_at(camera)
    }

    /// The camera time showing a real moment: corrected stretches first,
    /// where they are moved to; elsewhere the camera's time is the real one.
    pub fn to_camera(&self, real: i64) -> i64 {
        self.0
            .iter()
            .find(|c| c.from + c.offset <= real && real < c.until + c.offset)
            .map_or(real, |c| real - c.offset)
    }

    /// Clips moved to real time.
    pub fn clips(&self, clips: &[Clip]) -> Vec<Clip> {
        clips
            .iter()
            .map(|c| {
                let off = self.offset_at(c.start);
                Clip {
                    start: c.start + off,
                    end: c.end + off,
                    video_type: c.video_type,
                }
            })
            .collect()
    }

    /// A camera span moved to real time.
    pub fn span(&self, start: i64, end: i64) -> (i64, i64) {
        let off = self.offset_at(start);
        (start + off, end + off)
    }
}

fn read(path: &Path) -> Option<DayClock> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// One camera day's clock record (empty when there is none).
pub fn load_day(root: &Path, device_id: &str, day: &str) -> DayClock {
    read(&cache::day_dir(root, device_id, day).join("clock.json")).unwrap_or_default()
}

pub fn save_day(root: &Path, device_id: &str, day: &str, clock: &DayClock) -> std::io::Result<()> {
    let dir = cache::day_dir(root, device_id, day);
    std::fs::create_dir_all(&dir)?;
    let text = serde_json::to_string_pretty(clock).map_err(std::io::Error::other)?;
    let tmp = dir.join("clock.json.part");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, dir.join("clock.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(start: i64, end: i64) -> Clip {
        Clip {
            start,
            end,
            video_type: 1,
        }
    }

    #[test]
    fn a_restart_is_a_one_minute_pause_and_a_later_jump_is_its_fix() {
        // 1000-2000, restart (60 s), 2060-2066, jump of 339 s, 2405-3000,
        // then a restart nobody corrected: 3000 -> 3060 to the end.
        let clips = [
            clip(1000, 2000),
            clip(2060, 2066),
            clip(2405, 3000),
            clip(3060, 5000),
        ];
        let r = restarts(&clips);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].stopped, r[0].resumed), (2000, 2060));
        assert_eq!((r[0].corrected_at, r[0].jump), (Some(2405), Some(339)));
        assert_eq!((r[1].stopped, r[1].resumed), (3000, 3060));
        assert_eq!(r[1].corrected_at, None);
        // Clip seams are not restarts.
        assert!(restarts(&[clip(0, 100), clip(101, 200)]).is_empty());
    }

    #[test]
    fn the_watcher_corrects_from_the_restart_to_where_the_clock_jumped() {
        // Right until 2000, restart, stale clock from 2060 (really 4500 s
        // later); the clock is set at camera 4000, jumping to 8500.
        let mut clips = vec![clip(1000, 2000), clip(2060, 3000)];
        let mut day = DayClock::default();
        record_reading(
            &mut day,
            Reading {
                at: 1500,
                camera: 1500,
            },
            &clips,
        );
        assert!(day.corrections.is_empty());
        record_reading(
            &mut day,
            Reading {
                at: 7400,
                camera: 2900,
            },
            &clips,
        );
        assert_eq!(day.corrections.len(), 1);
        let c = &day.corrections[0];
        assert_eq!(
            (c.from, c.until, c.offset, c.open),
            (2060, 2901, 4500, true)
        );
        clips.push(clip(3000, 4000));
        record_reading(
            &mut day,
            Reading {
                at: 8300,
                camera: 3800,
            },
            &clips,
        );
        assert_eq!(day.corrections[0].until, 3801);
        clips.push(clip(8500, 9000));
        record_reading(
            &mut day,
            Reading {
                at: 9100,
                camera: 9100,
            },
            &clips,
        );
        let c = &day.corrections[0];
        assert_eq!((c.from, c.until, c.open), (2060, 4000, false));
        assert_eq!(day.readings.len(), 4);
    }

    #[test]
    fn a_pair_of_times_corrects_from_the_restart_to_the_end_of_the_day() {
        let clips = [clip(1000, 2000), clip(2060, 5000)];
        let c = correction_for(&clips, 2700, 2700 + 4500, String::new());
        assert_eq!((c.from, c.until, c.offset), (2060, 5000, 4500));
        let all = Corrections(vec![c]);
        // Before the restart nothing moves; after it, everything moves.
        assert_eq!(all.to_real(1500), 1500);
        assert_eq!(all.to_real(2060), 6560);
        assert_eq!(all.to_camera(6560), 2060);
        assert_eq!(all.to_camera(1500), 1500);
        assert_eq!(all.clips(&clips), vec![clip(1000, 2000), clip(6560, 9500)]);
    }
}
