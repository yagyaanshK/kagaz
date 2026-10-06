//! Saving a span of recorded footage as an MP4 file, through the video
//! engine: the stream server pulls the footage from the camera over the
//! relay as MPEG-TS, the engine remuxes it, and the fragmented MP4 it
//! produces is written to disk. One download per camera at a time.

use super::cloud::Camera;
use super::serve::TsServer;
use crate::extras::go2rtc::{Engine, ExtraError};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum Progress {
    /// Bytes written so far.
    Bytes(u64),
}

/// Pull footage between two unix times into `path` as MP4. `on_progress`
/// returns `false` to cancel. Returns the bytes written.
pub fn save_span(
    server: &TsServer,
    engine: &Engine,
    camera: &Camera,
    from: i64,
    to: i64,
    path: &Path,
    on_progress: &mut dyn FnMut(Progress) -> bool,
) -> Result<u64, ExtraError> {
    let (token, url) = server.playback_url(&camera.device_id, from, to, true);
    let name = format!("playback-{token}");
    engine.add_stream(&name, &url)?;
    let mut file = std::fs::File::create(path).map_err(|source| ExtraError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut write_error = None;
    let mut written = 0u64;
    let mut last_report = std::time::Instant::now();
    let result = engine.save_mp4(
        &name,
        &mut || server.playback_state(&token).is_some_and(|s| s.finished),
        &mut |chunk| {
            if let Err(e) = std::io::Write::write_all(&mut file, chunk) {
                write_error = Some(e);
                return false;
            }
            written += chunk.len() as u64;
            if last_report.elapsed() > Duration::from_millis(500) {
                last_report = std::time::Instant::now();
                return on_progress(Progress::Bytes(written));
            }
            true
        },
    );
    engine.remove_stream(&name);
    let state = server.playback_state(&token).unwrap_or_default();
    server.forget_playback(&token);
    if let Some(e) = write_error {
        return Err(ExtraError::Io {
            path: path.to_path_buf(),
            source: e,
        });
    }
    let total = result?;
    if total == 0 {
        let why = if state.error.is_empty() {
            "the camera sent nothing for that time".to_string()
        } else {
            state.error
        };
        let _ = std::fs::remove_file(path);
        return Err(ExtraError::Start(why));
    }
    Ok(total)
}
