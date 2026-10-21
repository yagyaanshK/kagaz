//! What a Xiaomi camera's SD card holds. The camera keeps one file a
//! minute and, asked to play a moment, answers only when it has that
//! minute; it offers no list. So Kagaz asks at intervals over one
//! connection (most answers in under a second) and keeps what it found in
//! `<cache>/xiaomi-footage/<did>.json`: a finished day is never asked
//! again, and today is asked only from where the last look stopped.

use super::api::Camera;
use super::live::{self, Quality};
use super::Session;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// How far apart the moments asked about are, in seconds.
pub const STEP: i64 = 600;

/// One open connection for asking about the card.
pub struct Card {
    view: live::Live,
}

impl Card {
    pub fn open(session: &mut Session, camera: &Camera) -> Result<Card, String> {
        let mut view = live::open(session, camera, Quality::Low, false)?;
        view.stop()?;
        Ok(Card { view })
    }

    /// The file holding moment `at`, as (start, length), if the card has one.
    /// The camera sometimes takes a few seconds to find a file, so a
    /// moment with no quick answer is asked about again, patiently, before
    /// it counts as empty (measured: every such second ask found one).
    pub fn at(&mut self, at: i64) -> Result<Option<(i64, i64)>, String> {
        match self.view.footage_at(at, Duration::from_millis(1500))? {
            Some(file) => Ok(Some(file)),
            None => self.view.footage_at(at, Duration::from_secs(4)),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Day {
    /// Asked up to here (unix seconds).
    asked_until: i64,
    /// Files found, as (start, length).
    files: Vec<(i64, i64)>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    /// By the day's start (unix seconds, local midnight).
    days: BTreeMap<i64, Day>,
}

fn store_path(did: &str) -> PathBuf {
    let safe: String = did.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    crate::paths::cache_dir().join("xiaomi-footage").join(format!("{safe}.json"))
}

fn load(did: &str) -> Store {
    std::fs::read(store_path(did))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(did: &str, store: &Store) {
    let path = store_path(did);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serde_json::to_vec(store).unwrap_or_default());
}

/// The files of the day starting at `day_start`, asking the camera about
/// whatever part of the day up to `now` was not asked before.
pub fn day(
    session: &mut Session,
    camera: &Camera,
    day_start: i64,
    now: i64,
    on_progress: &mut dyn FnMut(i64),
) -> Result<Vec<(i64, i64)>, String> {
    let mut store = load(&camera.did);
    let mut entry = store.days.get(&day_start).cloned().unwrap_or_default();
    let day_end = day_start + 86_400;
    // The minute being written now is not on the card yet.
    let until = day_end.min(now - 90);
    let from = entry.asked_until.max(day_start);
    if from + STEP / 2 < until {
        let mut card = Card::open(session, camera)?;
        // Ask in the middle of each stretch, half a minute past (files start at :30).
        let mut t = from - from.rem_euclid(STEP) + STEP / 2;
        while t < until {
            if t >= from {
                if let Some(file) = card.at(t)? {
                    if !entry.files.contains(&file) {
                        entry.files.push(file);
                    }
                }
                on_progress(t);
            }
            t += STEP;
        }
        entry.files.sort();
        entry.asked_until = until;
        store.days.insert(day_start, entry.clone());
        save(&camera.did, &store);
    }
    Ok(entry.files)
}

/// Which of the last `days` days (local midnights) have footage, newest
/// first: each day not known yet is asked about at a few moments, going
/// back until two days in a row have nothing (the card's oldest).
pub fn days(
    session: &mut Session,
    camera: &Camera,
    today_start: i64,
    days: u32,
) -> Result<Vec<i64>, String> {
    let mut store = load(&camera.did);
    let mut out = Vec::new();
    let mut card: Option<Card> = None;
    let mut empty_in_a_row = 0;
    for back in 0..i64::from(days) {
        let start = today_start - back * 86_400;
        let known = store.days.get(&start).filter(|d| !d.files.is_empty()).is_some();
        let found = if known {
            true
        } else {
            if card.is_none() {
                card = Some(Card::open(session, camera)?);
            }
            let card = card.as_mut().expect("opened");
            let mut any = false;
            for hour in [12, 18, 6, 21] {
                let t = start + hour * 3600 + 30;
                if t > crate::cameras::xiaomi::card::now() - 90 {
                    continue;
                }
                if let Some(file) = card.at(t)? {
                    let day = store.days.entry(start).or_default();
                    if !day.files.contains(&file) {
                        day.files.push(file);
                        day.files.sort();
                    }
                    any = true;
                    break;
                }
            }
            any
        };
        if found {
            out.push(start);
            empty_in_a_row = 0;
        } else {
            empty_in_a_row += 1;
            if empty_in_a_row >= 2 && back > 0 {
                break;
            }
        }
    }
    save(&camera.did, &store);
    Ok(out)
}

/// Files joined into stretches of footage: files a step apart or closer
/// are taken as one recording (the minutes between were not asked about).
pub fn stretches(files: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for &(start, length) in files {
        let end = start + length.max(1);
        match out.last_mut() {
            Some(last) if start <= last.1 + STEP + 60 => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_a_step_apart_join_into_one_stretch() {
        let files = [(1000, 59), (1600, 59), (2200, 59), (9000, 59)];
        assert_eq!(stretches(&files), vec![(1000, 2259), (9000, 9059)]);
        assert!(stretches(&[]).is_empty());
    }
}
