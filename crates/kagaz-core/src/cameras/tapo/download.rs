//! Saving a span of recorded footage as an MP4 file, through the video
//! engine: the stream server pulls the footage from the camera over the
//! relay as MPEG-TS, the engine remuxes it, and the fragmented MP4 it
//! produces is written to disk. One download per camera at a time.

use super::audio::AudioDemux;
use super::cloud::{Camera, Session};
use super::recordings::Recordings;
use super::serve::TsServer;
use crate::extras::ffmpeg;
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

/// Pull footage between two unix times straight from the camera (as fast
/// as it sends it), keep the picture as it is and put the sound in as AAC,
/// through ffmpeg. Returns the bytes of the finished file.
pub fn save_span_with_sound(
    session: &Session,
    camera: &Camera,
    from: i64,
    to: i64,
    path: &Path,
    ffmpeg_binary: &Path,
    on_progress: &mut dyn FnMut(Progress) -> bool,
) -> Result<u64, ExtraError> {
    let io = |p: &Path, source| ExtraError::Io {
        path: p.to_path_buf(),
        source,
    };
    let work = path.with_extension(format!("kagaz-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&work).map_err(|e| io(&work, e))?;
    let ts_path = work.join("in.ts");
    let alaw_path = work.join("sound.alaw");
    let result = (|| {
        let mut ts = std::fs::File::create(&ts_path).map_err(|e| io(&ts_path, e))?;
        let mut alaw = std::fs::File::create(&alaw_path).map_err(|e| io(&alaw_path, e))?;
        let mut demux = AudioDemux::default();
        let mut total = 0u64;
        let mut sound = 0u64;
        let mut last_report = std::time::Instant::now();
        let mut write_error: Option<std::io::Error> = None;
        let pulled = Recordings::new(session, camera).pull(from, to, &mut |chunk| {
            use std::io::Write;
            if let Err(e) = ts.write_all(chunk) {
                write_error = Some(e);
                return false;
            }
            let g711 = demux.push(chunk);
            if !g711.is_empty() {
                sound += g711.len() as u64;
                if let Err(e) = alaw.write_all(&g711) {
                    write_error = Some(e);
                    return false;
                }
            }
            total += chunk.len() as u64;
            if last_report.elapsed() > Duration::from_millis(500) {
                last_report = std::time::Instant::now();
                return on_progress(Progress::Bytes(total));
            }
            true
        });
        if let Some(e) = write_error {
            return Err(io(&ts_path, e));
        }
        let pulled = pulled.map_err(|e| ExtraError::Start(e.to_string()))?;
        if pulled == 0 {
            return Err(ExtraError::Start(
                "the camera sent nothing for that time".into(),
            ));
        }
        drop(ts);
        drop(alaw);
        let audio = (sound > 0).then(|| {
            (
                alaw_path.as_path(),
                demux.audio_offset_seconds().unwrap_or(0.0),
            )
        });
        ffmpeg::mux_mp4(ffmpeg_binary, &ts_path, audio, path)?;
        std::fs::metadata(path)
            .map(|m| m.len())
            .map_err(|e| io(path, e))
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}
