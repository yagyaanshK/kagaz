//! Pictures and sound to the window's own player. Whatever the source (a
//! camera's relay stream, a recording, the doorbell's relay), it reaches
//! the page over a channel as small binary packets, and the page decodes
//! them itself; no helper program on any platform.
//!
//! A packet: one byte of kind (1 sound, 2 keyframe, 3 picture, 4 the end),
//! four bytes of timestamp (90 kHz, big-endian), then the payload: 8 kHz
//! 16-bit little-endian samples, Annex-B video, or a UTF-8 reason.

use kagaz_core::media::Media;
use kagaz_core::mp4::Recorder;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager};

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
    fn has(&self, id: u64) -> bool {
        self.list.lock().unwrap().contains_key(&id)
    }
}

// ---- recording what a watch shows, as an MP4 file ----

/// Recordings under way, by the watch they record.
static RECORDINGS: LazyLock<Mutex<HashMap<u64, Recorder>>> = LazyLock::new(Default::default);

#[derive(Debug, Clone, Serialize)]
pub struct Recorded {
    watch: u64,
    /// The file on a computer; on a phone, where it was copied to.
    path: String,
    location: String,
    seconds: f64,
    bytes: u64,
}

/// A piece of a watched stream: into its recording, if it has one. A
/// recording that cannot be written (a full disk) ends there.
fn record(app: &AppHandle, id: u64, m: &Media) {
    let failed = {
        let mut all = RECORDINGS.lock().unwrap();
        match all.get_mut(&id) {
            Some(r) => r.push(m).is_err(),
            None => return,
        }
    };
    if failed {
        finish_recording(app, id);
    }
}

/// The watch is over (or could not be written): keep what was recorded
/// and tell the page.
fn finish_recording(app: &AppHandle, id: u64) {
    let Some(rec) = RECORDINGS.lock().unwrap().remove(&id) else {
        return;
    };
    match keep(app, id, rec) {
        Ok(saved) => {
            let _ = app.emit("recording-saved", &saved);
        }
        Err(e) => {
            let _ = app.emit("recording-failed", (id, e));
        }
    }
}

fn keep(app: &AppHandle, watch: u64, rec: Recorder) -> Result<Recorded, String> {
    let saved = rec.finish().map_err(|e| e.to_string())?;
    let location = super::saving::keep(app, &saved.path)?;
    Ok(Recorded {
        watch,
        path: if cfg!(mobile) { location.clone() } else { saved.path.display().to_string() },
        location,
        seconds: saved.seconds,
        bytes: saved.bytes,
    })
}

/// Start recording what watch `watch` shows into `<name>.mp4`, from its
/// next full picture.
#[tauri::command]
pub async fn camera_record_start(app: AppHandle, watch: u64, name: String) -> Result<(), String> {
    if !app.state::<Watches>().has(watch) {
        return Err("this picture is not playing any more".into());
    }
    let rec = super::blocking(move || {
        let path = super::saving::new_file(&format!("{name}.mp4"))?;
        Recorder::create(&path).map_err(|e| format!("could not make {}: {e}", path.display()))
    })
    .await?;
    RECORDINGS.lock().unwrap().insert(watch, rec);
    Ok(())
}

/// Stop recording and keep the file.
#[tauri::command]
pub async fn camera_record_stop(app: AppHandle, watch: u64) -> Result<Recorded, String> {
    let rec = RECORDINGS
        .lock()
        .unwrap()
        .remove(&watch)
        .ok_or("this picture is not being recorded")?;
    super::blocking(move || keep(&app, watch, rec)).await
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
                    record(&app, id, &m);
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
        finish_recording(&app, id);
    });
    id
}

/// Watch a camera live (HD or VGA). Returns the watch id.
#[tauri::command]
pub async fn camera_watch(
    app: AppHandle,
    device_id: String,
    hd: bool,
    // "auto", "high" or "low"; cameras that cannot choose for themselves
    // go by `hd`.
    quality: Option<String>,
    channel: Channel<InvokeResponseBody>,
) -> Result<u64, String> {
    let e = super::running(&app.state::<super::CameraState>())?;
    // A Xiaomi camera: Kagaz speaks to it itself, on this network.
    if let Some(x) = e.xiaomi.iter().find(|x| x.id() == device_id).cloned() {
        use kagaz_core::cameras::xiaomi::live::Quality;
        let quality = match quality.as_deref() {
            Some("auto") => Quality::Auto,
            Some("low") => Quality::Low,
            Some("high") => Quality::High,
            _ if hd => Quality::High,
            _ => Quality::Low,
        };
        let key = format!("{device_id}:{quality:?}");
        let watcher = super::blocking(move || {
            e._server.watch_with(&key, move |hand| super::xiaomi_live(&x.cam, quality, hand))
        })
        .await?;
        return Ok(forward(app, watcher.rx, channel));
    }
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
    let speed = speed.unwrap_or(1.0).clamp(0.25, 8.0);
    // A Xiaomi camera's card, played by the camera itself.
    if let Some(x) = super::xiaomi_of(&e, &device_id) {
        let (token, watcher) = super::blocking(move || {
            e._server.watch_playback_with(speed, move |hand| super::xiaomi_playback(&x.cam, from, to, hand))
        })
        .await?;
        return Ok(PlaybackWatch {
            id: forward(app, watcher.rx, channel),
            token,
            cached: false,
        });
    }
    let (_, cam) = super::camera_of(&e, &device_id)?;
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
