//! Pictures and sound to the window's own player. Whatever the source (a
//! camera's relay stream, a recording, the doorbell's relay), it reaches
//! the page over a channel as small binary packets, and the page decodes
//! them itself; no helper program on any platform.
//!
//! A packet: one byte of kind (1 sound, 2 keyframe, 3 picture, 4 the end),
//! four bytes of timestamp (90 kHz, big-endian), then the payload: 8 kHz
//! 16-bit little-endian samples, Annex-B video, or a UTF-8 reason.

use kagaz_core::media::Media;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager};

/// One stream being watched: a flag that ends a forwarding thread, or the
/// doorbell's live session, which ends when dropped.
pub enum Watch {
    Thread(Arc<AtomicBool>),
    /// Held only so that dropping it ends the session.
    Live(#[allow(dead_code)] kagaz_core::intercom::onetouch::live::Live),
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Watch::Thread(stop) = self {
            stop.store(true, Ordering::Release);
        }
    }
}

#[derive(Default)]
pub struct Watches {
    list: Mutex<HashMap<u64, Watch>>,
    next: AtomicU64,
}

impl Watches {
    pub fn add(&self, w: Watch) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        self.list.lock().unwrap().insert(id, w);
        id
    }
    pub fn remove(&self, id: u64) {
        self.list.lock().unwrap().remove(&id);
    }
}

pub fn packet(m: &Media) -> Vec<u8> {
    match m {
        Media::Audio { pcm, timestamp } => {
            let mut p = Vec::with_capacity(5 + pcm.len() * 2);
            p.push(1);
            p.extend_from_slice(&timestamp.to_be_bytes());
            for s in pcm {
                p.extend_from_slice(&s.to_le_bytes());
            }
            p
        }
        Media::Video {
            data,
            key,
            timestamp,
        } => {
            let mut p = Vec::with_capacity(5 + data.len());
            p.push(if *key { 2 } else { 3 });
            p.extend_from_slice(&timestamp.to_be_bytes());
            p.extend_from_slice(data);
            p
        }
        Media::Ended(why) => {
            let mut p = vec![4, 0, 0, 0, 0];
            p.extend_from_slice(why.as_bytes());
            p
        }
    }
}

/// Send to the page until the stream ends or the watch is removed.
pub fn send(channel: &Channel<InvokeResponseBody>, m: &Media) -> bool {
    channel.send(InvokeResponseBody::Raw(packet(m))).is_ok()
}

/// Forward a watcher's media to the page in the background; the returned
/// id ends it (`media_unwatch`). Dropping the receiver ends the tap.
pub fn forward(app: AppHandle, rx: mpsc::Receiver<Media>, channel: Channel<InvokeResponseBody>) -> u64 {
    let stop = Arc::new(AtomicBool::new(false));
    let id = app.state::<Watches>().add(Watch::Thread(stop.clone()));
    std::thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(m) => {
                    let end = matches!(m, Media::Ended(_));
                    if !send(&channel, &m) || end {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    send(&channel, &Media::Ended("the stream ended".into()));
                    break;
                }
            }
        }
        app.state::<Watches>().remove(id);
    });
    id
}

/// Watch a camera live (HD or VGA). Returns the watch id.
#[tauri::command]
pub async fn camera_watch(
    app: AppHandle,
    device_id: String,
    hd: bool,
    channel: Channel<InvokeResponseBody>,
) -> Result<u64, String> {
    let e = super::running(&app.state::<super::CameraState>())?;
    let watcher = super::blocking(move || e._server.watch_live(&device_id, hd)).await?;
    Ok(forward(app, watcher.rx, channel))
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackWatch {
    id: u64,
    token: String,
    cached: bool,
}

/// Watch recorded footage between two unix times (window time); the
/// token answers `playback_state` for where it stands.
#[tauri::command]
pub async fn playback_watch(
    app: AppHandle,
    device_id: String,
    from: i64,
    to: i64,
    speed: Option<f64>,
    channel: Channel<InvokeResponseBody>,
) -> Result<PlaybackWatch, String> {
    let e = super::running(&app.state::<super::CameraState>())?;
    let (_, cam) = super::camera_of(&e, &device_id)?;
    let speed = speed.unwrap_or(1.0).clamp(0.25, 8.0);
    let (from, to) = super::camera_span(&device_id, from, to);
    let (token, watcher, cached) = super::blocking(move || {
        let (token, w) = e._server.watch_playback(&cam.device_id, from, to, speed)?;
        let cached = kagaz_core::cameras::tapo::cache::find(
            &kagaz_core::paths::recordings_dir(),
            &cam.device_id,
            from,
        )
        .is_some();
        Ok((token, w, cached))
    })
    .await?;
    Ok(PlaybackWatch {
        id: forward(app, watcher.rx, channel),
        token,
        cached,
    })
}

/// Stop watching: the stream's source ends when nobody watches it.
#[tauri::command]
pub fn media_unwatch(app: AppHandle, id: u64) {
    app.state::<Watches>().remove(id);
}

/// A line from the page's player into the stream log, next to Rust's own,
/// so a blank picture can be diagnosed from one file.
#[tauri::command]
pub fn page_log(line: String) {
    let line: String = line.chars().take(300).filter(|c| !c.is_control()).collect();
    kagaz_core::cameras::tapo::serve::log(&format!("page: {line}"));
}
