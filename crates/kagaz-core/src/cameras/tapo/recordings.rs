//! Recordings on a Tapo camera's SD card, reached through the cloud
//! passthrough the way the app does: which days have footage, the clips of
//! one day, and the camera's clock. Method names and parameter shapes
//! follow the app (as documented by the ontapo library, MIT).
//!
//! Days are the *camera's* calendar days: the card indexes footage by the
//! camera's own clock, so a request for "today" must use its timezone.

use super::cloud::{Camera, CloudError, Session};
use super::relay::{
    request_relay_for, stream_download, stream_playback, RelayError, STREAM_PLAYBACK,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The SD card as the camera reports it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdCard {
    /// "normal", "offline", "unformatted", ... as the camera words it.
    pub state: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub loop_recording: bool,
    /// Unix time of the oldest footage kept, 0 when unknown.
    pub recording_since: i64,
}

impl SdCard {
    /// True when the card is present and in use.
    pub fn usable(&self) -> bool {
        self.state == "normal"
    }
}

/// One recorded span on the card, unix seconds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Clip {
    pub start: i64,
    pub end: i64,
    /// 1 continuous, 2 motion/event, as the camera labels it.
    pub video_type: u32,
}

impl Clip {
    pub fn seconds(&self) -> i64 {
        self.end - self.start
    }
}

/// A camera's recordings reached through one session.
pub struct Recordings<'a> {
    pub session: &'a Session,
    pub camera: &'a Camera,
}

impl<'a> Recordings<'a> {
    pub fn new(session: &'a Session, camera: &'a Camera) -> Self {
        Self { session, camera }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, CloudError> {
        self.session.device_request(
            &self.camera.device_id,
            &self.camera.app_server,
            method,
            params,
        )
    }

    /// The card's state and space.
    pub fn sd_card(&self) -> Result<SdCard, CloudError> {
        let result = self.call(
            "getSdCardStatus",
            json!({ "harddisk_manage": { "table": ["hd_info"] } }),
        )?;
        Ok(parse_sd_card(&result))
    }

    /// The device-side user id that `searchVideoOfDay` and playback need.
    /// It is stable, so it is asked for once per camera and remembered;
    /// the camera sometimes refuses the question (-71101) when asked often,
    /// so a refusal is retried a few times.
    pub fn user_id(&self) -> Result<u64, CloudError> {
        if let Some(id) = user_ids()
            .lock()
            .ok()
            .and_then(|m| m.get(&self.camera.device_id).copied())
        {
            return Ok(id);
        }
        let mut last = None;
        for attempt in 0..4u64 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(700 * attempt));
            }
            match self.call("getUserID", json!({ "system": { "get_user_id": "null" } })) {
                Ok(result) => {
                    let id = parse_user_id(&result).ok_or_else(|| CloudError::Rejected {
                        status: 200,
                        message: "getUserID returned no user_id".into(),
                    })?;
                    if let Ok(mut m) = user_ids().lock() {
                        m.insert(self.camera.device_id.clone(), id);
                    }
                    return Ok(id);
                }
                Err(e @ CloudError::Rejected { .. }) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.expect("at least one attempt"))
    }

    /// Forget the remembered user id (the camera said it is no longer valid).
    pub fn forget_user_id(&self) {
        if let Ok(mut m) = user_ids().lock() {
            m.remove(&self.camera.device_id);
        }
    }

    /// The camera's UTC offset in minutes (its clock decides the recording days).
    pub fn utc_offset_minutes(&self) -> Result<i32, CloudError> {
        let result = self.call("getTimezone", json!({ "system": { "name": "basic" } }))?;
        let raw = result
            .pointer("/system/basic/timezone")
            .and_then(Value::as_str)
            .unwrap_or("UTC+00:00");
        Ok(parse_utc_offset(raw).unwrap_or(0))
    }

    /// Camera-local days (`YYYYMMDD`) between two days, inclusive, that have footage.
    pub fn dates(&self, start_date: &str, end_date: &str) -> Result<Vec<String>, CloudError> {
        let result = self.call(
            "searchDateWithVideo",
            json!({ "playback": { "search_year_utility": {
                "channel": [0], "start_date": start_date, "end_date": end_date } } }),
        )?;
        Ok(parse_dates(&result))
    }

    /// The clips of one camera-local day (`YYYYMMDD`), oldest first.
    pub fn clips(&self, date: &str) -> Result<Vec<Clip>, CloudError> {
        let ask = |uid: u64| {
            self.call(
                "searchVideoOfDay",
                json!({ "playback": { "search_video_utility": {
                    "channel": 0, "date": date, "end_index": 999_999_999, "id": uid, "start_index": 0 } } }),
            )
        };
        let result = match ask(self.user_id()?) {
            Ok(r) => r,
            // A stale user id: ask for a fresh one and try once more.
            Err(CloudError::Rejected { message, .. }) if message.contains("-7110") => {
                self.forget_user_id();
                ask(self.user_id()?)?
            }
            Err(e) => return Err(e),
        };
        Ok(parse_clips(&result))
    }

    /// Pull the footage between two unix times through the relay as fast as
    /// the camera sends it (the app's download), handing every MPEG-TS part
    /// to `sink`. One recording session per camera at a time; a second one
    /// is refused (code -52405). Returns the bytes delivered.
    pub fn pull(
        &self,
        start: i64,
        end: i64,
        sink: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<u64, PullError> {
        self.session_with(start, |relay, track, uid| {
            stream_download(
                relay,
                &self.session.terminal_uuid,
                track,
                uid,
                start,
                end,
                sink,
            )
        })
    }

    /// The same footage paced at real time, for watching as it arrives.
    pub fn play(
        &self,
        start: i64,
        end: i64,
        sink: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<u64, PullError> {
        self.session_with(start, |relay, track, uid| {
            stream_playback(
                relay,
                &self.session.terminal_uuid,
                track,
                uid,
                start,
                end,
                sink,
            )
        })
    }

    fn session_with(
        &self,
        start: i64,
        run: impl FnOnce(&super::relay::RelayParams, &str, u64) -> Result<u64, RelayError>,
    ) -> Result<u64, PullError> {
        let uid = self.user_id()?;
        let track_id = playback_track_id(&self.camera.device_id, start);
        let relay = request_relay_for(
            self.session,
            &self.camera.device_id,
            &self.camera.app_server,
            &track_id,
            "HD",
            STREAM_PLAYBACK,
        )?;
        Ok(run(&relay, &track_id, uid)?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PullError {
    #[error(transparent)]
    Cloud(#[from] CloudError),
    #[error(transparent)]
    Relay(#[from] RelayError),
}

impl PullError {
    /// The camera is already sending a recording to someone; try again later.
    pub fn is_busy(&self) -> bool {
        matches!(self, PullError::Relay(RelayError::Refused(-52405)))
    }
}

fn user_ids() -> &'static std::sync::Mutex<std::collections::HashMap<String, u64>> {
    static IDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    IDS.get_or_init(Default::default)
}

/// The track id the app uses for a recording download.
pub fn playback_track_id(device_id: &str, start: i64) -> String {
    format!("backup-{device_id}-{start}")
}

/// The camera-local day (`YYYYMMDD`) that a unix time falls on.
pub fn day_of(unix: i64, utc_offset_minutes: i32) -> String {
    let (y, m, d) = civil_from_days((unix + i64::from(utc_offset_minutes) * 60).div_euclid(86_400));
    format!("{y:04}{m:02}{d:02}")
}

/// The unix time at which a camera-local day (`YYYYMMDD`) begins.
pub fn day_start(date: &str, utc_offset_minutes: i32) -> Option<i64> {
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let y: i64 = date[0..4].parse().ok()?;
    let m: u32 = date[4..6].parse().ok()?;
    let d: u32 = date[6..8].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400 - i64::from(utc_offset_minutes) * 60)
}

/// "YYYY-MM-DD HH:MM[:SS]" (or with a `T`) in a clock with this UTC offset → unix time.
pub fn parse_time(text: &str, utc_offset_minutes: i32) -> Option<i64> {
    let text = text.trim();
    let (date, time) = text
        .split_once(' ')
        .or_else(|| text.split_once('T'))
        .unwrap_or((text, "00:00"));
    let date: String = date.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut parts = time.split(':').map(|p| p.parse::<i64>().ok());
    let h = parts.next().flatten()?;
    let m = parts.next().flatten().unwrap_or(0);
    let s = parts.next().flatten().unwrap_or(0);
    if !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..60).contains(&s) {
        return None;
    }
    Some(day_start(&date, utc_offset_minutes)? + h * 3600 + m * 60 + s)
}

/// A unix time as "YYYY-MM-DD HH:MM:SS" in a clock with this UTC offset.
pub fn format_time(unix: i64, utc_offset_minutes: i32) -> String {
    let local = unix + i64::from(utc_offset_minutes) * 60;
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let secs = local.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// "YYYYMMDD" → "YYYY-MM-DD".
pub fn dash_date(date: &str) -> String {
    if date.len() == 8 {
        format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
    } else {
        date.to_string()
    }
}

fn parse_sd_card(result: &Value) -> SdCard {
    let slots = result
        .pointer("/harddisk_manage/hd_info")
        .and_then(Value::as_array);
    let Some(hd) = slots
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|slot| slot.values())
        .find(|v| v.is_object())
    else {
        return SdCard {
            state: "offline".into(),
            ..SdCard::default()
        };
    };
    let text = |k: &str| {
        hd.get(k)
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default()
    };
    let state = {
        let s = text("detect_status");
        if s.is_empty() {
            text("status")
        } else {
            s
        }
    };
    SdCard {
        state: if state.is_empty() {
            "offline".into()
        } else {
            state
        },
        total_bytes: text("total_space_accurate")
            .parse()
            .unwrap_or_else(|_| bytes_from_text(&text("total_space"))),
        free_bytes: text("free_space_accurate")
            .parse()
            .unwrap_or_else(|_| bytes_from_text(&text("free_space"))),
        loop_recording: text("loop_record_status") == "1",
        recording_since: text("record_start_time").parse().unwrap_or(0),
    }
}

/// "29.7GB", "512MB", "0B" → bytes (decimal units, as the camera rounds them).
fn bytes_from_text(s: &str) -> u64 {
    let s = s.trim();
    let digits_end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let number: f64 = s[..digits_end].parse().unwrap_or(0.0);
    let unit = s[digits_end..].trim().to_ascii_uppercase();
    let factor = match unit.chars().next() {
        Some('K') => 1e3,
        Some('M') => 1e6,
        Some('G') => 1e9,
        Some('T') => 1e12,
        _ => 1.0,
    };
    (number * factor) as u64
}

fn parse_user_id(result: &Value) -> Option<u64> {
    let v = result
        .get("user_id")
        .or_else(|| result.pointer("/system/get_user_id/user_id"))?;
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
    .filter(|&n| n != 0)
}

fn parse_dates(result: &Value) -> Vec<String> {
    let mut dates: Vec<String> = result
        .pointer("/playback/search_results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|m| m.values())
        .filter_map(|v| v.get("date").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    dates.sort();
    dates.dedup();
    dates
}

fn parse_clips(result: &Value) -> Vec<Clip> {
    let mut clips: Vec<Clip> = result
        .pointer("/playback/search_video_results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|m| m.values())
        .filter_map(|v| {
            let start = v.get("startTime").and_then(Value::as_i64)?;
            let end = v.get("endTime").and_then(Value::as_i64)?;
            (end > start).then_some(Clip {
                start,
                end,
                video_type: v
                    .get("vedio_type")
                    .or_else(|| v.get("video_type"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
            })
        })
        .collect();
    clips.sort_by_key(|c| c.start);
    clips
}

/// "UTC+05:30", "+0530", "-05:00" → minutes east of UTC.
pub fn parse_utc_offset(raw: &str) -> Option<i32> {
    let s = raw
        .trim()
        .trim_start_matches("UTC")
        .trim_start_matches("GMT");
    if s.is_empty() {
        return Some(0);
    }
    let (sign, rest) = match s.as_bytes()[0] {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => (1, s),
    };
    let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
    let (h, m) = match digits.len() {
        1 | 2 => (digits.parse::<i32>().ok()?, 0),
        3 => (digits[..1].parse().ok()?, digits[1..].parse().ok()?),
        4 => (digits[..2].parse().ok()?, digits[2..].parse().ok()?),
        _ => return None,
    };
    Some(sign * (h * 60 + m))
}

// Howard Hinnant's civil date algorithms (public domain).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sd_card_comes_from_the_first_slot() {
        let v = json!({ "harddisk_manage": { "hd_info": [ { "hd_info_1": {
            "detect_status": "normal", "total_space_accurate": "31914983424",
            "free_space_accurate": "1234", "loop_record_status": "1",
            "record_start_time": "1750000000", "rw_attr": "rw" } } ] } });
        let card = parse_sd_card(&v);
        assert!(card.usable());
        assert_eq!(card.total_bytes, 31_914_983_424);
        assert_eq!(card.free_bytes, 1234);
        assert!(card.loop_recording);
        assert_eq!(card.recording_since, 1_750_000_000);
        assert_eq!(parse_sd_card(&json!({})).state, "offline");
        let v = json!({ "harddisk_manage": { "hd_info": [ { "hd_info_1": {
            "status": "normal", "total_space": "29.7GB", "free_space": "0B" } } ] } });
        let card = parse_sd_card(&v);
        assert_eq!(card.total_bytes, 29_700_000_000);
        assert_eq!(card.free_bytes, 0);
    }

    #[test]
    fn user_id_dates_and_clips_parse_like_the_app() {
        assert_eq!(parse_user_id(&json!({ "user_id": 1234 })), Some(1234));
        assert_eq!(parse_user_id(&json!({ "user_id": 0 })), None);
        let dates = parse_dates(&json!({ "playback": { "search_results": [
            { "search_result_1": { "date": "20260702" } },
            { "search_result_2": { "date": "20260701" } } ] } }));
        assert_eq!(dates, vec!["20260701", "20260702"]);
        let clips = parse_clips(&json!({ "playback": { "search_video_results": [
            { "search_video_results_1": { "startTime": 200, "endTime": 260, "vedio_type": 2 } },
            { "search_video_results_2": { "startTime": 100, "endTime": 100, "vedio_type": 1 } },
            { "search_video_results_3": { "startTime": 10, "endTime": 70, "vedio_type": 1 } } ] } }));
        assert_eq!(
            clips,
            vec![
                Clip {
                    start: 10,
                    end: 70,
                    video_type: 1
                },
                Clip {
                    start: 200,
                    end: 260,
                    video_type: 2
                }
            ]
        );
    }

    #[test]
    fn camera_days_follow_the_camera_clock() {
        assert_eq!(parse_utc_offset("UTC+05:30"), Some(330));
        assert_eq!(parse_utc_offset("-0500"), Some(-300));
        assert_eq!(parse_utc_offset("UTC"), Some(0));
        // 2026-07-01 00:00 IST = 2026-06-30 18:30 UTC
        let ist_midnight = day_start("20260701", 330).unwrap();
        assert_eq!(
            ist_midnight,
            days_from_civil(2026, 7, 1) * 86_400 - 330 * 60
        );
        assert_eq!(day_of(ist_midnight, 330), "20260701");
        assert_eq!(day_of(ist_midnight - 1, 330), "20260630");
        assert_eq!(day_of(ist_midnight, 0), "20260630");
        assert_eq!(day_start("2026070", 0), None);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(playback_track_id("ABC", 5), "backup-ABC-5");
        let t = parse_time("2026-07-01 14:05:09", 330).unwrap();
        assert_eq!(t, ist_midnight + 14 * 3600 + 5 * 60 + 9);
        assert_eq!(format_time(t, 330), "2026-07-01 14:05:09");
        assert_eq!(
            parse_time("2026-07-01T14:05", 0),
            parse_time("20260701 14:05", 0)
        );
        assert_eq!(parse_time("2026-07-01 24:00", 0), None);
        assert_eq!(dash_date("20260701"), "2026-07-01");
    }
}
