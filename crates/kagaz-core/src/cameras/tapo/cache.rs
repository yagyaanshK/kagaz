//! Recordings fetched from a camera, kept on disk so the same footage is
//! never pulled over the relay twice. One file per span as the camera sent
//! it (MPEG-TS), named by the span's unix times, beside a small JSON note:
//!
//!   <recordings>/<device-id>/<YYYY-MM-DD>/<start>-<end>.ts
//!   <recordings>/<device-id>/<YYYY-MM-DD>/<start>-<end>.json
//!
//! A span fetched from a clip's own start begins exactly there; a span
//! asked for mid-clip starts at the camera's previous full frame, a second
//! or two early, which `exact_start` records.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub device_id: String,
    /// The span asked for, unix seconds.
    pub start: i64,
    pub end: i64,
    /// True when `start` is the first frame (the span began at a clip start).
    pub exact_start: bool,
    /// Seconds of video actually in the file (from its timestamps).
    pub seconds: f64,
    pub bytes: u64,
    /// The whole span arrived (false: stopped early, `seconds` says how far).
    pub complete: bool,
    pub fetched_at: i64,
    /// The file, filled in when listed.
    #[serde(skip)]
    pub path: PathBuf,
}

impl Entry {
    /// The span the file really covers.
    pub fn covered_end(&self) -> i64 {
        if self.complete {
            self.end
        } else {
            self.start + self.seconds.floor() as i64
        }
    }

    pub fn covers(&self, t: i64) -> bool {
        self.start <= t && t < self.covered_end()
    }
}

/// The folder for one camera and day ("YYYY-MM-DD").
pub fn day_dir(root: &Path, device_id: &str, day: &str) -> PathBuf {
    root.join(safe(device_id)).join(day)
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Every cached span of a camera, oldest first.
pub fn list(root: &Path, device_id: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(days) = std::fs::read_dir(root.join(safe(device_id))) else {
        return out;
    };
    for day in days.flatten() {
        let Ok(files) = std::fs::read_dir(day.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_some_and(|e| e == "json") {
                let ts = path.with_extension("ts");
                if let Some(mut e) = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Entry>(&t).ok())
                {
                    if ts.is_file() {
                        e.path = ts;
                        out.push(e);
                    }
                }
            }
        }
    }
    out.sort_by_key(|e| e.start);
    out
}

/// The cached span that has footage at `t`, preferring an exact one.
pub fn find(root: &Path, device_id: &str, t: i64) -> Option<Entry> {
    let mut hits: Vec<Entry> = list(root, device_id)
        .into_iter()
        .filter(|e| e.covers(t))
        .collect();
    hits.sort_by_key(|e| (!e.exact_start, e.start));
    hits.into_iter().next()
}

/// Is this whole span already on disk?
pub fn has_span(root: &Path, device_id: &str, start: i64, end: i64) -> bool {
    list(root, device_id)
        .iter()
        .any(|e| e.start <= start && e.covered_end() >= end - 1)
}

/// Writes one span to disk as it arrives; `finish` makes it visible.
pub struct Writer {
    entry: Entry,
    file: std::fs::File,
    part: PathBuf,
    pts: super::relay::PtsTracker,
    finished: bool,
}

impl Drop for Writer {
    /// A writer dropped without `finish` (an error on the way) leaves nothing behind.
    fn drop(&mut self) {
        if !self.finished {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

impl Writer {
    pub fn create(
        root: &Path,
        device_id: &str,
        day: &str,
        start: i64,
        end: i64,
        exact_start: bool,
    ) -> std::io::Result<Writer> {
        let dir = day_dir(root, device_id, day);
        std::fs::create_dir_all(&dir)?;
        let base = dir.join(format!("{start}-{end}"));
        let part = base.with_extension("ts.part");
        let file = std::fs::File::create(&part)?;
        Ok(Writer {
            entry: Entry {
                device_id: device_id.to_string(),
                start,
                end,
                exact_start,
                seconds: 0.0,
                bytes: 0,
                complete: false,
                fetched_at: now(),
                path: base.with_extension("ts"),
            },
            file,
            part,
            pts: Default::default(),
            finished: false,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        self.file.write_all(chunk)?;
        self.entry.bytes += chunk.len() as u64;
        self.pts.update(chunk);
        Ok(())
    }

    pub fn seconds(&self) -> f64 {
        self.pts.seconds()
    }

    /// Keep what arrived (a stopped playback still counts for what it got).
    /// Fewer than two seconds of video is not worth keeping.
    pub fn finish(mut self, complete: bool) -> std::io::Result<Option<Entry>> {
        use std::io::Write;
        self.finished = true;
        self.file.flush()?;
        self.entry.seconds = self.pts.seconds();
        let span = (self.entry.end - self.entry.start) as f64;
        self.entry.complete = complete || self.entry.seconds >= span - 2.0;
        if self.entry.seconds < 2.0 {
            let _ = std::fs::remove_file(&self.part);
            return Ok(None);
        }
        std::fs::rename(&self.part, &self.entry.path)?;
        let note = serde_json::to_string_pretty(&self.entry).expect("json");
        std::fs::write(self.entry.path.with_extension("json"), note)?;
        Ok(Some(self.entry.clone()))
    }
}

const TS: usize = 188;
const PTS_WRAP: u64 = 1 << 33;

/// What one 188-byte packet says, as far as playing from a file needs.
struct Packet {
    pid: u16,
    pusi: bool,
    random_access: bool,
    pts: Option<u64>,
}

fn packet_info(p: &[u8]) -> Packet {
    let pid = (u16::from(p[1] & 0x1F) << 8) | u16::from(p[2]);
    let pusi = p[1] & 0x40 != 0;
    let afc = (p[3] >> 4) & 0x3;
    let mut off = 4;
    let mut random_access = false;
    if afc >= 2 {
        let len = usize::from(p[4]);
        if len > 0 {
            random_access = p[5] & 0x40 != 0;
        }
        off += 1 + len;
    }
    let mut pts = None;
    if afc != 2 && pusi && off + 14 <= TS && p[off..off + 3] == [0, 0, 1] && p[off + 7] & 0x80 != 0
    {
        let q = &p[off + 9..off + 14];
        pts = Some(
            ((u64::from(q[0]) & 0x0E) << 29)
                | (u64::from(q[1]) << 22)
                | ((u64::from(q[2]) & 0xFE) << 14)
                | (u64::from(q[3]) << 7)
                | (u64::from(q[4]) >> 1),
        );
        // The camera does not flag its full frames; find them in the H.264
        // data instead: a sequence header (SPS, 7) or an IDR slice (5).
        let body = off + 9 + usize::from(p[off + 8]);
        if body < TS {
            random_access |= p[body..]
                .windows(4)
                .any(|w| w[..3] == [0, 0, 1] && matches!(w[3] & 0x1F, 5 | 7));
        }
    }
    Packet {
        pid,
        pusi,
        random_access,
        pts,
    }
}

/// Where a table section starts in a packet: past any adaptation field and
/// the pointer field. The camera pads its tables with an adaptation field.
fn section_start(p: &[u8]) -> Option<usize> {
    let afc = (p[3] >> 4) & 0x3;
    let mut off = 4;
    if afc >= 2 {
        off += 1 + usize::from(p[4]);
    }
    if afc == 2 || off >= TS {
        return None;
    }
    let at = off + 1 + usize::from(p[off]);
    (at < TS).then_some(at)
}

/// The program tables (PAT, PMT) and the video PID, from the file's start.
fn tables(file: &mut std::fs::File) -> std::io::Result<(Vec<u8>, u16)> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut head = vec![0u8; TS * 4000];
    let n = file.read(&mut head)?;
    head.truncate(n - n % TS);
    let mut pmt_pid = None;
    let mut video = None;
    let mut out = Vec::new();
    for p in head.as_chunks::<TS>().0.iter() {
        if p[0] != 0x47 {
            continue;
        }
        let info = packet_info(p);
        let Some(sec_at) = section_start(p) else {
            continue;
        };
        if info.pid == 0 && info.pusi && out.is_empty() {
            out.extend_from_slice(p);
            let sec = &p[sec_at..];
            if sec.len() >= 12 {
                pmt_pid = Some((u16::from(sec[10] & 0x1F) << 8) | u16::from(sec[11]));
            }
        } else if Some(info.pid) == pmt_pid && info.pusi && out.len() == TS {
            out.extend_from_slice(p);
            let sec = &p[sec_at..];
            if sec.len() < 12 {
                continue;
            }
            let info_len = (usize::from(sec[10] & 0x0F) << 8) | usize::from(sec[11]);
            let mut j = 12 + info_len;
            let end = ((usize::from(sec[1] & 0x0F) << 8) | usize::from(sec[2])) + 3 - 4;
            while j + 5 <= end.min(sec.len()) {
                let kind = sec[j];
                let pid = (u16::from(sec[j + 1] & 0x1F) << 8) | u16::from(sec[j + 2]);
                if (kind == 0x1B || kind == 0x24) && video.is_none() {
                    video = Some(pid);
                }
                j += 5 + ((usize::from(sec[j + 3] & 0x0F) << 8) | usize::from(sec[j + 4]));
            }
            break;
        }
    }
    let video = video.ok_or_else(|| std::io::Error::other("no video track in the file"))?;
    Ok((out, video))
}

/// The first video timestamp at or after byte `offset`.
fn pts_at(
    file: &mut std::fs::File,
    offset: u64,
    video: u16,
) -> std::io::Result<Option<(u64, u64)>> {
    use std::io::{Read, Seek, SeekFrom};
    let start = offset - offset % TS as u64;
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; TS * 2000];
    let n = file.read(&mut buf)?;
    for (i, p) in buf[..n - n % TS].as_chunks::<TS>().0.iter().enumerate() {
        if p[0] != 0x47 {
            continue;
        }
        let info = packet_info(p);
        if info.pid == video {
            if let Some(pts) = info.pts {
                return Ok(Some((start + (i * TS) as u64, pts)));
            }
        }
    }
    Ok(None)
}

/// Play a cached file from `from` to `to` (unix seconds): the program
/// tables, then from the last full frame at or before `from`, paced at real
/// time times `pace` (None: as fast as the reader takes it). Returns the
/// bytes handed to `sink`.
pub fn play_file(
    entry: &Entry,
    from: i64,
    to: i64,
    pace: Option<f64>,
    sink: &mut dyn FnMut(&[u8]) -> bool,
) -> std::io::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(&entry.path)?;
    let len = file.metadata()?.len();
    let (head, video) = tables(&mut file)?;
    let (_, first) = pts_at(&mut file, 0, video)?
        .ok_or_else(|| std::io::Error::other("no video timestamps in the file"))?;
    let rel = |pts: u64| (pts + PTS_WRAP - first) % PTS_WRAP;
    let target = ((from - entry.start).max(0) as u64) * 90_000;
    // Binary search for a spot a few seconds before the target...
    let (mut lo, mut hi) = (0u64, len);
    while hi - lo > (TS * 2000) as u64 {
        let mid = (lo + hi) / 2;
        match pts_at(&mut file, mid, video)? {
            Some((_, pts)) if rel(pts) + 3 * 90_000 < target => lo = mid,
            _ => hi = mid,
        }
    }
    // ...then forward to the last full frame at or before it.
    let mut pos = lo - lo % TS as u64;
    let mut start_at = pos;
    let mut start_pts = None;
    file.seek(SeekFrom::Start(pos))?;
    let mut buf = vec![0u8; TS * 512];
    'find: loop {
        let n = file.read(&mut buf)?;
        if n < TS {
            break;
        }
        for (i, p) in buf[..n - n % TS].as_chunks::<TS>().0.iter().enumerate() {
            if p[0] != 0x47 {
                continue;
            }
            let info = packet_info(p);
            if info.pid == video && info.random_access {
                if let Some(pts) = info.pts {
                    if rel(pts) > target && start_pts.is_some() {
                        break 'find;
                    }
                    start_at = pos + (i * TS) as u64;
                    start_pts = Some(rel(pts));
                    if rel(pts) > target {
                        break 'find;
                    }
                }
            }
        }
        pos += (n - n % TS) as u64;
        file.seek(SeekFrom::Start(pos))?;
    }
    let start_pts = start_pts.unwrap_or(0);
    // The span to send, measured from that frame.
    let span = ((to - from).max(0) as u64) * 90_000 + target.saturating_sub(start_pts);
    let mut sent = 0u64;
    if !sink(&head) {
        return Ok(0);
    }
    sent += head.len() as u64;
    file.seek(SeekFrom::Start(start_at))?;
    let began = std::time::Instant::now();
    loop {
        let n = file.read(&mut buf)?;
        if n < TS {
            return Ok(sent);
        }
        let n = n - n % TS;
        // Hold each piece until its time has come (half a second ahead).
        let mut last_pts = None;
        for p in buf[..n].as_chunks::<TS>().0.iter() {
            if p[0] == 0x47 {
                let info = packet_info(p);
                if info.pid == video {
                    if let Some(pts) = info.pts {
                        last_pts = Some(rel(pts).saturating_sub(start_pts));
                    }
                }
            }
        }
        if let Some(at) = last_pts {
            if at >= span {
                // Send up to the end of the span, then stop.
                let mut cut = 0;
                for (i, p) in buf[..n].as_chunks::<TS>().0.iter().enumerate() {
                    let info = packet_info(p);
                    if info.pid == video {
                        if let Some(pts) = info.pts {
                            if rel(pts).saturating_sub(start_pts) >= span {
                                cut = i * TS;
                                break;
                            }
                        }
                    }
                }
                if cut > 0 {
                    sink(&buf[..cut]);
                    sent += cut as u64;
                }
                return Ok(sent);
            }
            if let Some(speed) = pace {
                let due =
                    std::time::Duration::from_secs_f64(at as f64 / 90_000.0 / speed.max(0.05));
                let ahead = std::time::Duration::from_millis(500);
                let elapsed = began.elapsed();
                if due > elapsed + ahead {
                    std::thread::sleep(due - elapsed - ahead);
                }
            }
        }
        if !sink(&buf[..n]) {
            return Ok(sent);
        }
        sent += n as u64;
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_are_written_listed_and_found() {
        let root = std::env::temp_dir().join(format!("kagaz-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut w = Writer::create(&root, "DEV1", "2026-10-01", 1000, 1060, true).unwrap();
        // two PES packets 60 s apart (90 kHz)
        for pts in [0u64, 60 * 90_000] {
            let mut p = vec![0x47, 0x40, 0x44, 0x10, 0, 0, 1, 0xE0, 0, 0, 0x80, 0x80, 5];
            let q = [
                0x21 | (((pts >> 30) & 7) as u8) << 1,
                (pts >> 22) as u8,
                (((pts >> 15) & 0x7F) as u8) << 1 | 1,
                (pts >> 7) as u8,
                ((pts & 0x7F) as u8) << 1 | 1,
            ];
            p.extend_from_slice(&q);
            p.resize(188, 0);
            w.write(&p).unwrap();
        }
        let e = w.finish(false).unwrap().unwrap();
        assert!(e.complete);
        assert_eq!(e.seconds, 60.0);
        assert!(has_span(&root, "DEV1", 1000, 1060));
        assert!(!has_span(&root, "DEV1", 1000, 1200));
        assert_eq!(find(&root, "DEV1", 1030).unwrap().start, 1000);
        assert!(find(&root, "DEV1", 1060).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
