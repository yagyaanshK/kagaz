//! A small local HTTP server that exposes each Tapo camera's live view as
//! `http://127.0.0.1:<port>/tapo/<device-id>.ts`: a plain MPEG-TS stream
//! that go2rtc (or any player) can read. Every client gets its own relay
//! session; the relay fans the camera's live stream out, so several
//! viewers of one camera cost the camera nothing extra.

use crate::media::Media;
use super::cloud::{Camera, CloudError, Session};
use super::recordings::Recordings;
use super::relay::RelayError;
use super::relay::{request_relay, stream_preview};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Where one recording playback stands. A playback is one-shot: its token
/// serves once, so an engine that reconnects after the footage ends gets
/// "gone" instead of the same footage again.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlaybackState {
    pub started: bool,
    pub finished: bool,
    pub bytes: u64,
    /// Why it ended early, if it did.
    pub error: String,
    /// Playback speed, so its sound can be played at the same pace.
    pub speed: f64,
}

/// A listener for one session's raw G.711 bytes.
type AudioTap = std::sync::mpsc::SyncSender<Vec<u8>>;

/// A requested span of recorded footage.
#[derive(Debug, Clone)]
struct Span {
    from: i64,
    to: i64,
    /// One-shot token from `playback_url`.
    token: String,
    /// As fast as the camera sends it (saving) rather than paced (watching).
    fast: bool,
    /// Playback speed; timestamps are rescaled so the player shows it.
    speed: f64,
}

/// Append one line to `<cache>/cameras.log` with a timestamp, for diagnosis.
pub fn log(line: &str) {
    let t = crate::localtime::now();
    let text = format!("{:02}:{:02}:{:02} {line}\n", t.hour, t.minute, t.second);
    let path = crate::paths::cache_dir().join("cameras.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(text.as_bytes());
    }
}

pub struct TsServer {
    pub port: u16,
    /// Loopback addresses this server answers on (127.0.0.1 first). A
    /// webview allows only a few connections per host, so each camera tile
    /// can be given its own address.
    pub hosts: Vec<String>,
    session: Arc<Mutex<Session>>,
    cameras: Arc<Mutex<Vec<Camera>>>,
    /// The video engine's API port, once it runs, for the `/engine/` proxy.
    engine_port: Arc<Mutex<Option<u16>>>,
    /// Recording playbacks by token.
    playbacks: Arc<Mutex<HashMap<String, PlaybackState>>>,
    /// Listeners for the audio of a running session, by device id (live)
    /// or playback token; each gets the raw G.711 bytes as they arrive.
    taps: Arc<Mutex<HashMap<String, Vec<AudioTap>>>>,
    /// Cameras with a live relay session running right now.
    feeding: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Each camera's UTC offset in minutes, asked once (for the cache's day folders).
    offsets: Arc<Mutex<HashMap<String, i32>>>,
    /// Watchers of pictures and sound for the window's own player, by
    /// `<device id>:<HD|VGA>` (live) or playback token.
    media_taps: Arc<Mutex<HashMap<String, Vec<MediaTap>>>>,
    /// Live relay sessions open for watchers, by the same key.
    media_sessions: Arc<Mutex<std::collections::HashSet<String>>>,
}

/// One watcher of a stream's pictures and sound.
struct MediaTap {
    tx: std::sync::mpsc::SyncSender<Media>,
    /// Fell behind: no pictures until the next keyframe, so it never gets
    /// one whose reference it missed.
    skipping: bool,
}

/// Pictures and sound of one stream, for the window's own player.
pub struct Watcher {
    pub rx: std::sync::mpsc::Receiver<Media>,
}

/// How many extra loopback addresses to try (127.0.0.2 ...).
const ALIASES: u8 = 24;

impl TsServer {
    /// Bind on 127.0.0.1 (a free port when `port` is 0) and, where the
    /// system allows, on 127.0.0.2 onwards too; serve in the background.
    pub fn start(
        session: Session,
        cameras: Vec<Camera>,
        port: u16,
    ) -> std::io::Result<Arc<TsServer>> {
        let first = TcpListener::bind(("127.0.0.1", port))?;
        let port = first.local_addr()?.port();
        let mut listeners = vec![first];
        let mut hosts = vec!["127.0.0.1".to_string()];
        for n in 2..(2 + ALIASES) {
            let host = format!("127.0.0.{n}");
            if let Ok(l) = TcpListener::bind((host.as_str(), port)) {
                listeners.push(l);
                hosts.push(host);
            } else {
                break;
            }
        }
        let server = Arc::new(TsServer {
            port,
            hosts,
            session: Arc::new(Mutex::new(session)),
            cameras: Arc::new(Mutex::new(cameras)),
            engine_port: Arc::new(Mutex::new(None)),
            playbacks: Arc::new(Mutex::new(HashMap::new())),
            taps: Arc::new(Mutex::new(HashMap::new())),
            feeding: Arc::new(Mutex::new(std::collections::HashSet::new())),
            offsets: Arc::new(Mutex::new(HashMap::new())),
            media_taps: Arc::new(Mutex::new(HashMap::new())),
            media_sessions: Arc::new(Mutex::new(std::collections::HashSet::new())),
        });
        for listener in listeners {
            let s = server.clone();
            std::thread::Builder::new()
                .name("kagaz-ts-server".into())
                .spawn(move || {
                    for stream in listener.incoming().flatten() {
                        let s = s.clone();
                        std::thread::spawn(move || s.handle(stream));
                    }
                })?;
        }
        Ok(server)
    }

    /// Tell the server where the video engine listens, enabling `/engine/<stream>.ts`.
    pub fn set_engine_port(&self, port: u16) {
        if let Ok(mut p) = self.engine_port.lock() {
            *p = Some(port);
        }
    }

    /// The i-th camera's own host (wraps around when there are more cameras than addresses).
    pub fn host_for(&self, index: usize) -> &str {
        if self.hosts.len() > 1 {
            &self.hosts[1 + index % (self.hosts.len() - 1)]
        } else {
            &self.hosts[0]
        }
    }

    /// The engine's MPEG-TS output for `stream`, proxied on the i-th camera's own host.
    pub fn tile_url(&self, index: usize, stream: &str) -> String {
        format!(
            "http://{}:{}/engine/{stream}.ts",
            self.host_for(index),
            self.port
        )
    }

    /// The HD stream URL for a camera.
    pub fn url_for(&self, device_id: &str) -> String {
        format!("http://127.0.0.1:{}/tapo/{device_id}.ts", self.port)
    }

    /// The low-resolution (VGA) stream URL, for small tiles.
    pub fn vga_url_for(&self, device_id: &str) -> String {
        format!("http://127.0.0.1:{}/tapo/{device_id}.ts?res=vga", self.port)
    }

    /// Register a recording playback and return the one-shot URL that
    /// streams the footage between two unix times, paced at real time for
    /// watching, or as fast as the camera sends it (`fast`) for saving.
    pub fn playback_url(
        &self,
        device_id: &str,
        from: i64,
        to: i64,
        fast: bool,
    ) -> (String, String) {
        self.playback_url_at(device_id, from, to, fast, 1.0)
    }

    /// The same at a playback speed: above 1 the camera's fast delivery
    /// feeds the player and the timestamps are rescaled; at or below 1 the
    /// paced stream is enough.
    pub fn playback_url_at(
        &self,
        device_id: &str,
        from: i64,
        to: i64,
        fast: bool,
        speed: f64,
    ) -> (String, String) {
        let token = uuid::Uuid::new_v4().simple().to_string();
        if let Ok(mut p) = self.playbacks.lock() {
            p.insert(
                token.clone(),
                PlaybackState {
                    speed,
                    ..PlaybackState::default()
                },
            );
        }
        let mut url = format!(
            "http://127.0.0.1:{}/tapo/{device_id}.ts?from={from}&to={to}&once={token}",
            self.port
        );
        if fast || speed > 1.0 {
            url.push_str("&mode=download");
        }
        if (speed - 1.0).abs() > 1e-6 {
            url.push_str(&format!("&speed={speed:.3}"));
        }
        (token, url)
    }

    /// Where a playback stands (None for an unknown token).
    pub fn playback_state(&self, token: &str) -> Option<PlaybackState> {
        self.playbacks.lock().ok()?.get(token).cloned()
    }

    fn update_playback(&self, token: &str, f: impl FnOnce(&mut PlaybackState)) {
        if let Ok(mut p) = self.playbacks.lock() {
            if let Some(state) = p.get_mut(token) {
                f(state);
            }
        }
    }

    /// Forget a finished playback.
    pub fn forget_playback(&self, token: &str) {
        if let Ok(mut p) = self.playbacks.lock() {
            p.remove(token);
        }
    }

    /// Exchange the refresh token for a new session token, keep it in the
    /// server and in the session file, and return it.
    pub fn refresh_session(&self) -> Result<Session, CloudError> {
        let mut guard = self.session.lock().unwrap_or_else(|p| p.into_inner());
        guard.refresh()?;
        let _ = guard.save(&Session::default_path());
        log("session: token refreshed");
        Ok(guard.clone())
    }

    /// A copy of the session the server uses.
    pub fn session(&self) -> Session {
        self.session
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn cameras(&self) -> Vec<Camera> {
        self.cameras.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn set_cameras(&self, cameras: Vec<Camera>) {
        if let Ok(mut c) = self.cameras.lock() {
            *c = cameras;
        }
    }

    /// Hand this part of a session's stream to whoever listens for its audio.
    fn feed_taps(&self, key: &str, demux: &mut super::audio::AudioDemux, part: &[u8]) {
        let Ok(mut taps) = self.taps.lock() else {
            return;
        };
        let Some(list) = taps.get_mut(key) else {
            return;
        };
        let g711 = demux.push(part);
        if g711.is_empty() {
            return;
        }
        // A listener that stopped reading is dropped; a slow one loses this part.
        list.retain(|tx| {
            !matches!(
                tx.try_send(g711.clone()),
                Err(std::sync::mpsc::TrySendError::Disconnected(_))
            )
        });
        if list.is_empty() {
            taps.remove(key);
        }
    }

    /// Stream the audio of the session `key` as WAV until the listener leaves
    /// or the session ends.
    fn serve_audio(&self, key: &str, mut client: TcpStream) {
        use super::audio::{decode, Law};
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
        if let Ok(mut taps) = self.taps.lock() {
            taps.entry(key.to_string()).or_default().push(tx);
        }
        if client
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nCache-Control: no-store\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
            .is_err()
        {
            return;
        }
        let mut chunk = |data: &[u8]| -> bool {
            client
                .write_all(format!("{:x}\r\n", data.len()).as_bytes())
                .and_then(|()| client.write_all(data))
                .and_then(|()| client.write_all(b"\r\n"))
                .is_ok()
        };
        // A sped-up (or slowed) playback: label the sound's rate to match.
        let speed = self
            .playback_state(key)
            .map(|st| st.speed)
            .filter(|s| *s > 0.0)
            .unwrap_or(1.0);
        let rate = ((8000.0 * speed).round() as u32).clamp(2000, 64000);
        if !chunk(&super::audio::wav_header_at(rate)) {
            return;
        }
        log(&format!("audio {key}: listener joined"));
        // A live key with nothing streaming for it (a tile fed some other
        // way): open a small relay session of our own just for the sound.
        let own_session = if self.cameras().iter().any(|c| c.device_id == key) {
            let server = self.clone_handle();
            let key = key.to_string();
            let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
            std::thread::spawn(move || server.sound_only_session(&key, stop_rx));
            Some(stop_tx)
        } else {
            None
        };
        // Wait up to a minute for the session to produce sound, then give
        // up after a minute of silence.
        let law = Law::ALaw;
        let mut total = 0u64;
        while let Ok(g711) = rx.recv_timeout(std::time::Duration::from_secs(60)) {
            total += g711.len() as u64;
            if !chunk(&decode(law, &g711)) {
                break;
            }
        }
        let _ = client.write_all(b"0\r\n\r\n");
        drop(own_session);
        log(&format!(
            "audio {key}: listener left after {} s of sound",
            total / 8000
        ));
    }

    /// A relay session (VGA, the cheapest) kept only to feed the audio taps
    /// of `device_id` while someone listens; ends when `stop` is dropped or
    /// when another session for the camera is already feeding the taps.
    fn sound_only_session(&self, device_id: &str, stop: std::sync::mpsc::Receiver<()>) {
        // Give a video session a moment to show up before opening our own.
        std::thread::sleep(std::time::Duration::from_millis(1500));
        if self
            .feeding
            .lock()
            .map(|f| f.contains(device_id))
            .unwrap_or(false)
        {
            return;
        }
        let Some(cam) = self
            .cameras()
            .into_iter()
            .find(|c| c.device_id == device_id)
        else {
            return;
        };
        let session = self.session();
        let track = format!("sound-{}-{}", cam.device_id, uuid::Uuid::new_v4().simple());
        let Ok(relay) = request_relay(&session, &cam.device_id, &cam.app_server, &track, "VGA")
        else {
            log(&format!("sound-only {}: relay refused", cam.name));
            return;
        };
        log(&format!("sound-only {}: start", cam.name));
        let mut demux = super::audio::AudioDemux::default();
        let result = stream_preview(
            &relay,
            &session.terminal_uuid,
            &track,
            "VGA",
            &mut |chunk| {
                self.feed_taps(device_id, &mut demux, chunk);
                // Stop when the listener has gone, or when nobody taps any more.
                let listening = self
                    .taps
                    .lock()
                    .map(|t| t.contains_key(device_id))
                    .unwrap_or(false);
                listening
                    && !matches!(
                        stop.try_recv(),
                        Err(std::sync::mpsc::TryRecvError::Disconnected)
                    )
            },
        );
        log(&format!(
            "sound-only {}: {}",
            cam.name,
            match result {
                Ok(_) => "ended".to_string(),
                Err(e) => format!("ended: {e}"),
            }
        ));
    }

    /// Live pictures and sound of a camera for the window's own player.
    /// The first watcher opens the relay session; the last one leaving ends it.
    pub fn watch_live(&self, device_id: &str, hd: bool) -> Result<Watcher, String> {
        let cam = self
            .cameras()
            .into_iter()
            .find(|c| c.device_id == device_id)
            .ok_or("no such camera")?;
        let resolution = if hd { "HD" } else { "VGA" };
        let key = format!("{device_id}:{resolution}");
        let (tx, rx) = std::sync::mpsc::sync_channel::<Media>(256);
        self.media_taps
            .lock()
            .map_err(|_| "state poisoned")?
            .entry(key.clone())
            .or_default()
            .push(MediaTap {
                tx,
                skipping: false,
            });
        let fresh = self
            .media_sessions
            .lock()
            .map_err(|_| "state poisoned")?
            .insert(key.clone());
        if fresh {
            let server = self.clone_handle();
            std::thread::Builder::new()
                .name("kagaz-watch".into())
                .spawn(move || server.media_session(&key, &cam, resolution))
                .map_err(|e| e.to_string())?;
        }
        Ok(Watcher { rx })
    }

    /// Pictures of a stream the video engine reaches by itself (a Xiaomi
    /// camera through go2rtc's own source), for the window's own player:
    /// the engine's MPEG-TS, split the same way as a Tapo relay's.
    /// Watchers of the same `key` share one engine connection.
    pub fn watch_engine(&self, key: &str, stream: &str) -> Result<Watcher, String> {
        let port = self
            .engine_port
            .lock()
            .ok()
            .and_then(|p| *p)
            .ok_or("the video engine is not running")?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<Media>(256);
        self.media_taps
            .lock()
            .map_err(|_| "state poisoned")?
            .entry(key.to_string())
            .or_default()
            .push(MediaTap {
                tx,
                skipping: false,
            });
        let fresh = self
            .media_sessions
            .lock()
            .map_err(|_| "state poisoned")?
            .insert(key.to_string());
        if fresh {
            let server = self.clone_handle();
            let (key, stream) = (key.to_string(), stream.to_string());
            std::thread::Builder::new()
                .name("kagaz-watch-engine".into())
                .spawn(move || {
                    let why = server.engine_session(&key, &stream, port);
                    if let Ok(mut s) = server.media_sessions.lock() {
                        s.remove(&key);
                    }
                    server.end_taps(&key, why);
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(Watcher { rx })
    }

    /// Why an engine session ended: empty when the watchers simply left.
    fn engine_session(&self, key: &str, stream: &str, port: u16) -> String {
        let safe: String = stream
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        let url = format!("http://127.0.0.1:{port}/api/stream.ts?src={safe}");
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(5))
            .timeout_read(std::time::Duration::from_secs(30))
            .build();
        let started = Instant::now();
        log(&format!("watch {stream}: start (engine)"));
        let resp = match agent.get(&url).call() {
            Ok(r) => r,
            Err(e) => {
                log(&format!("watch {stream}: engine refused: {e}"));
                return format!("the video engine could not open the camera: {e}");
            }
        };
        let mut reader = resp.into_reader();
        let mut demux = super::demux::Demux::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        let ended = loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break "the camera stopped sending".to_string(),
                Ok(n) => {
                    total += n as u64;
                    if !self.fan_out(key, demux.push(&buf[..n])) {
                        break String::new();
                    }
                }
                Err(e) => break format!("the camera stopped sending ({e})"),
            }
        };
        let s = &demux.stats;
        log(&format!(
            "watch {stream}: {} after {:.1} s, {} KB; {} pictures ({} key)",
            if ended.is_empty() { "watchers left" } else { ended.as_str() },
            started.elapsed().as_secs_f32(),
            total / 1000,
            s.units,
            s.keyframes,
        ));
        ended
    }

    /// Recorded footage between two unix times for the window's own player,
    /// at `speed` (the timestamps are rescaled; the player paces by them).
    /// The token answers `playback_state`.
    pub fn watch_playback(
        &self,
        device_id: &str,
        from: i64,
        to: i64,
        speed: f64,
    ) -> Result<(String, Watcher), String> {
        let cam = self
            .cameras()
            .into_iter()
            .find(|c| c.device_id == device_id)
            .ok_or("no such camera")?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        if let Ok(mut p) = self.playbacks.lock() {
            p.insert(
                token.clone(),
                PlaybackState {
                    speed,
                    ..PlaybackState::default()
                },
            );
        }
        let (tx, rx) = std::sync::mpsc::sync_channel::<Media>(256);
        self.media_taps
            .lock()
            .map_err(|_| "state poisoned")?
            .entry(token.clone())
            .or_default()
            .push(MediaTap {
                tx,
                skipping: false,
            });
        let span = Span {
            from,
            to,
            token: token.clone(),
            fast: speed > 1.0,
            speed,
        };
        let server = self.clone_handle();
        let session = self.session();
        let key = token.clone();
        std::thread::Builder::new()
            .name("kagaz-watch-playback".into())
            .spawn(move || {
                let mut demux = super::demux::Demux::new();
                server.run_playback(&session, &cam, &span, &mut |chunk| {
                    server.fan_out(&key, demux.push(chunk))
                });
                server.end_taps(&key, String::new());
            })
            .map_err(|e| e.to_string())?;
        Ok((token, Watcher { rx }))
    }

    /// Hand these pieces to everyone watching `key`; false once nobody does.
    fn fan_out(&self, key: &str, items: Vec<Media>) -> bool {
        let Ok(mut taps) = self.media_taps.lock() else {
            return false;
        };
        let Some(list) = taps.get_mut(key) else {
            return false;
        };
        for m in items {
            let delta = matches!(&m, Media::Video { key: false, .. });
            let keyframe = matches!(&m, Media::Video { key: true, .. });
            list.retain_mut(|tap| {
                if delta && tap.skipping {
                    return true;
                }
                match tap.tx.try_send(m.clone()) {
                    Ok(()) => {
                        if keyframe {
                            tap.skipping = false;
                        }
                        true
                    }
                    // A slow watcher misses this and every picture until the next keyframe.
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        if delta || keyframe {
                            tap.skipping = true;
                        }
                        true
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => false,
                }
            });
        }
        if list.is_empty() {
            taps.remove(key);
            return false;
        }
        true
    }

    /// Tell whoever still watches `key` that it is over, and forget them.
    fn end_taps(&self, key: &str, why: String) {
        let taps = self
            .media_taps
            .lock()
            .ok()
            .and_then(|mut t| t.remove(key))
            .unwrap_or_default();
        for tap in taps {
            let _ = tap.tx.try_send(Media::Ended(why.clone()));
        }
    }

    /// A relay session feeding the watchers of `key` until the last leaves.
    fn media_session(&self, key: &str, cam: &Camera, resolution: &str) {
        let why = self.media_session_inner(key, cam, resolution);
        if let Ok(mut s) = self.media_sessions.lock() {
            s.remove(key);
        }
        self.end_taps(key, why);
    }

    /// Why the session ended: empty when the watchers simply left.
    fn media_session_inner(&self, key: &str, cam: &Camera, resolution: &str) -> String {
        let (session, terminal) = match self.session.lock() {
            Ok(s) => (s.clone(), s.terminal_uuid.clone()),
            Err(_) => return "state poisoned".into(),
        };
        let track = format!("watch-{}-{}", cam.device_id, uuid::Uuid::new_v4().simple());
        let relay = match request_relay(
            &session,
            &cam.device_id,
            &cam.app_server,
            &track,
            resolution,
        )
        .or_else(|e| match e {
            RelayError::Cloud(CloudError::Unauthorized) => {
                let session = self.refresh_session()?;
                request_relay(
                    &session,
                    &cam.device_id,
                    &cam.app_server,
                    &track,
                    resolution,
                )
            }
            other => Err(other),
        }) {
            Ok(r) => r,
            Err(e) => {
                log(&format!("watch {} {resolution}: relay refused: {e}", cam.name));
                return e.to_string();
            }
        };
        let started = Instant::now();
        let mut total = 0u64;
        log(&format!("watch {} {resolution}: start", cam.name));
        if let Ok(mut f) = self.feeding.lock() {
            f.insert(cam.device_id.clone());
        }
        let mut demux = super::demux::Demux::new();
        let mut sound = super::audio::AudioDemux::default();
        let mut watched = true;
        let result = stream_preview(&relay, &terminal, &track, resolution, &mut |chunk| {
            total += chunk.len() as u64;
            self.feed_taps(&cam.device_id, &mut sound, chunk);
            watched = self.fan_out(key, demux.push(chunk));
            watched
        });
        if let Ok(mut f) = self.feeding.lock() {
            f.remove(&cam.device_id);
        }
        let s = &demux.stats;
        log(&format!(
            "watch {} {resolution}: {} after {:.1} s, {} KB; {} pictures ({} key), {} held before the first keyframe",
            cam.name,
            match &result {
                Ok(_) if !watched => "watchers left".to_string(),
                Ok(_) => "the relay closed".to_string(),
                Err(e) => format!("ended: {e}"),
            },
            started.elapsed().as_secs_f32(),
            total / 1000,
            s.units,
            s.keyframes,
            s.waiting
        ));
        match result {
            Ok(_) if !watched => String::new(),
            Ok(_) => "the camera stopped sending".into(),
            Err(e) => e.to_string(),
        }
    }

    /// The camera's UTC offset, asked once and remembered.
    fn utc_offset(&self, recordings: &Recordings) -> i32 {
        let id = recordings.camera.device_id.clone();
        if let Some(o) = self.offsets.lock().ok().and_then(|m| m.get(&id).copied()) {
            return o;
        }
        let o = recordings.utc_offset_minutes().unwrap_or(0);
        if let Ok(mut m) = self.offsets.lock() {
            m.insert(id, o);
        }
        o
    }

    /// A handle to this server for another thread.
    fn clone_handle(&self) -> Arc<TsServer> {
        Arc::new(TsServer {
            port: self.port,
            hosts: self.hosts.clone(),
            session: self.session.clone(),
            cameras: self.cameras.clone(),
            engine_port: self.engine_port.clone(),
            playbacks: self.playbacks.clone(),
            taps: self.taps.clone(),
            feeding: self.feeding.clone(),
            offsets: self.offsets.clone(),
            media_taps: self.media_taps.clone(),
            media_sessions: self.media_sessions.clone(),
        })
    }

    /// Pipe the engine's `/api/stream.ts?src=<name>` to this client.
    fn proxy_engine(&self, name: &str, mut client: TcpStream) {
        let Some(port) = self.engine_port.lock().ok().and_then(|p| *p) else {
            let _ = client.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        };
        let Ok(mut upstream) = TcpStream::connect(("127.0.0.1", port)) else {
            let _ = client.write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        };
        let safe: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if upstream
            .write_all(format!("GET /api/stream.ts?src={safe} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())
            .is_err()
        {
            return;
        }
        // Forward the engine's response verbatim (its transfer encoding is
        // what tells the player this is a live stream), then the body.
        let started = Instant::now();
        let peer = client
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        log(&format!("proxy {name} -> {peer}: start"));
        let mut upstream = upstream;
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        let ended = loop {
            match std::io::Read::read(&mut upstream, &mut buf) {
                Ok(0) => break "engine closed",
                Ok(n) => {
                    if client.write_all(&buf[..n]).is_err() {
                        break "player closed";
                    }
                    total += n as u64;
                }
                Err(e) => {
                    break if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut
                    {
                        "engine read timed out"
                    } else {
                        "engine read error"
                    }
                }
            }
        };
        log(&format!(
            "proxy {name} -> {peer}: {ended} after {:.1} s, {} KB",
            started.elapsed().as_secs_f32(),
            total / 1000
        ));
    }

    fn handle(&self, mut stream: TcpStream) {
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        });
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        // Read the headers (a player asking for an MP4 sends Range).
        let mut request_headers = Vec::new();
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) if line.trim().is_empty() => break,
                Ok(_) => request_headers.push(line.trim().to_string()),
                Err(_) => return,
            }
        }
        let path = request_line.split_whitespace().nth(1).unwrap_or("/");
        let (path_only, query) = path.split_once('?').unwrap_or((path, ""));
        if let Some(name) = path_only
            .strip_prefix("/engine/")
            .and_then(|p| p.strip_suffix(".ts"))
        {
            let name = name.to_string();
            self.proxy_engine(&name, stream);
            return;
        }
        if let Some(key) = path_only
            .strip_prefix("/audio/")
            .and_then(|p| p.strip_suffix(".wav"))
        {
            let key = key.to_string();
            self.serve_audio(&key, stream);
            return;
        }
        if let Some(rest) = path_only.strip_prefix("/clip/") {
            self.serve_clip(rest, &request_headers, stream);
            return;
        }
        if let Some(token) = path_only
            .strip_prefix("/playback/")
            .and_then(|p| p.strip_suffix(".json"))
        {
            let body = self
                .playback_state(token)
                .map(|st| serde_json::to_string(&st).unwrap_or_else(|_| "{}".into()));
            match body {
                Some(body) => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
            return;
        }
        let param = |key: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
                .map(str::to_string)
        };
        let resolution = if param("res").is_some_and(|v| v.eq_ignore_ascii_case("vga")) {
            "VGA"
        } else {
            "HD"
        };
        let span = match (
            param("from").and_then(|v| v.parse::<i64>().ok()),
            param("to").and_then(|v| v.parse::<i64>().ok()),
        ) {
            (Some(from), Some(to)) => Some(Span {
                from,
                to,
                token: param("once").unwrap_or_default(),
                fast: param("mode").is_some_and(|m| m == "download"),
                speed: param("speed")
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .unwrap_or(1.0),
            }),
            _ => None,
        };
        let device_id = path_only
            .strip_prefix("/tapo/")
            .and_then(|p| p.strip_suffix(".ts"))
            .map(str::to_string);
        let Some(device_id) = device_id else {
            if path == "/" || path == "/index.json" {
                let cams = self.cameras();
                let body = serde_json::to_string(&cams).unwrap_or_else(|_| "[]".into());
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            } else {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            return;
        };
        let cam = self
            .cameras()
            .into_iter()
            .find(|c| c.device_id == device_id);
        let Some(cam) = cam else {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        };
        let (session, terminal) = match self.session.lock() {
            Ok(s) => (s.clone(), s.terminal_uuid.clone()),
            Err(_) => return,
        };
        if let Some(span) = span {
            self.serve_playback(stream, &session, &cam, &span);
            return;
        }
        let track = format!(
            "preview-{}-{}",
            cam.device_id,
            uuid::Uuid::new_v4().simple()
        );
        let relay = match request_relay(
            &session,
            &cam.device_id,
            &cam.app_server,
            &track,
            resolution,
        )
        .or_else(|e| match e {
            // The token expired while the window was open: refresh and try once more.
            RelayError::Cloud(CloudError::Unauthorized) => {
                let session = self.refresh_session()?;
                request_relay(
                    &session,
                    &cam.device_id,
                    &cam.app_server,
                    &track,
                    resolution,
                )
            }
            other => Err(other),
        }) {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{msg}",
                    msg.len()
                );
                return;
            }
        };
        if stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n")
            .is_err()
        {
            return;
        }
        let started = Instant::now();
        let mut total = 0u64;
        log(&format!("relay {} {resolution}: start", cam.name));
        if let Ok(mut f) = self.feeding.lock() {
            f.insert(cam.device_id.clone());
        }
        let mut demux = super::audio::AudioDemux::default();
        let result = stream_preview(&relay, &terminal, &track, resolution, &mut |chunk| {
            total += chunk.len() as u64;
            self.feed_taps(&cam.device_id, &mut demux, chunk);
            stream.write_all(chunk).is_ok()
        });
        if let Ok(mut f) = self.feeding.lock() {
            f.remove(&cam.device_id);
        }
        log(&format!(
            "relay {} {resolution}: {} after {:.1} s, {} KB",
            cam.name,
            match &result {
                Ok(_) => "reader closed".to_string(),
                Err(e) => format!("ended: {e}"),
            },
            started.elapsed().as_secs_f32(),
            total / 1000
        ));
    }

    /// Stream recorded footage once per token; a reused token is "gone".
    /// A cached clip's MP4 (`/clip/<device-id>/<YYYY-MM-DD>/<start>-<end>.mp4`),
    /// with byte ranges, which a player needs to play, seek and speed it up.
    /// Only files inside the recordings folder, named the way the cache names them.
    fn serve_clip(&self, rest: &str, headers: &[String], mut client: TcpStream) {
        use std::io::{Read, Seek, SeekFrom};
        let not_found = |c: &mut TcpStream| {
            let _ = c.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        };
        let parts: Vec<&str> = rest.split('/').collect();
        let ok_name = |s: &str, extra: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
        };
        if parts.len() != 3
            || !ok_name(parts[0], "-_")
            || !ok_name(parts[1], "-")
            || !parts[2].ends_with(".mp4")
            || !ok_name(parts[2].trim_end_matches(".mp4"), "-")
        {
            return not_found(&mut client);
        }
        let path = crate::paths::recordings_dir()
            .join(parts[0])
            .join(parts[1])
            .join(parts[2]);
        let Ok(mut file) = std::fs::File::open(&path) else {
            return not_found(&mut client);
        };
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        if size == 0 {
            return not_found(&mut client);
        }
        let range = headers
            .iter()
            .find_map(|h| {
                let (k, v) = h.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case("range")
                    .then(|| v.trim().to_string())
            })
            .and_then(|v| {
                let v = v.strip_prefix("bytes=")?;
                let (a, b) = v.split_once('-')?;
                let start: u64 = if a.is_empty() { 0 } else { a.parse().ok()? };
                let end: u64 = if b.is_empty() {
                    size - 1
                } else {
                    b.parse().ok()?
                };
                (start <= end && end < size).then_some((start, end))
            });
        let (start, end) = range.unwrap_or((0, size - 1));
        let head = if range.is_some() {
            format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp4\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{size}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                end - start + 1
            )
        } else {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nAccept-Ranges: bytes\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
            )
        };
        if client.write_all(head.as_bytes()).is_err() || file.seek(SeekFrom::Start(start)).is_err()
        {
            return;
        }
        let mut left = end - start + 1;
        let mut buf = vec![0u8; 1 << 16];
        while left > 0 {
            let want = (left as usize).min(buf.len());
            match file.read(&mut buf[..want]) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if client.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    left -= n as u64;
                }
            }
        }
    }

    /// The URL of a cached clip's MP4 on a tile's own loopback address.
    pub fn clip_url(&self, index: usize, entry: &super::cache::Entry) -> Option<String> {
        let mp4 = super::cache::mp4_path(entry);
        let root = crate::paths::recordings_dir();
        let rel = mp4.strip_prefix(&root).ok()?;
        let rel: Vec<String> = rel
            .iter()
            .map(|c| c.to_string_lossy().to_string())
            .collect();
        Some(format!(
            "http://{}:{}/clip/{}",
            self.host_for(index),
            self.port,
            rel.join("/")
        ))
    }

    fn serve_playback(&self, mut stream: TcpStream, session: &Session, cam: &Camera, span: &Span) {
        let fresh = match self.playbacks.lock() {
            Ok(mut p) => match p.get_mut(&span.token) {
                Some(st) if !st.started => {
                    st.started = true;
                    true
                }
                _ => false,
            },
            Err(_) => false,
        };
        if !fresh {
            let _ = stream
                .write_all(b"HTTP/1.1 410 Gone\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        if stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n")
            .is_err()
        {
            return;
        }
        self.run_playback(session, cam, span, &mut |chunk| stream.write_all(chunk).is_ok());
    }

    /// Pull the footage of `span` (from the disk when it was kept, else from
    /// the camera, keeping it) and hand each part to `out`, retimed when the
    /// speed is not 1; the token's state says how it went.
    fn run_playback(
        &self,
        session: &Session,
        cam: &Camera,
        span: &Span,
        out: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        let Span {
            from,
            to,
            token,
            fast,
            speed,
        } = span;
        let (from, to, fast, speed) = (*from, *to, *fast, *speed);
        let started = Instant::now();
        let mut total = 0u64;
        log(&format!(
            "playback {} {from}-{to}{}: start",
            cam.name,
            if fast { " fast" } else { "" }
        ));
        let recordings = Recordings::new(session, cam);
        let root = crate::paths::recordings_dir();
        // Already on disk: play it from there, never from the camera again.
        let cached = super::cache::find(&root, &cam.device_id, from);
        // Otherwise keep what the camera sends, for next time.
        let mut keep = if cached.is_none() {
            let offset = self.utc_offset(&recordings);
            let day = super::recordings::dash_date(&super::recordings::day_of(from, offset));
            super::cache::Writer::create(&root, &cam.device_id, &day, from, to, false).ok()
        } else {
            None
        };
        let mut demux = super::audio::AudioDemux::default();
        let mut retimer = super::retime::Retimer::new(speed);
        let mut sink = |chunk: &[u8]| {
            total += chunk.len() as u64;
            if let Some(w) = keep.as_mut() {
                if w.write(chunk).is_err() {
                    keep = None;
                }
            }
            self.feed_taps(token, &mut demux, chunk);
            let retimed;
            let chunk: &[u8] = if (speed - 1.0).abs() > 1e-6 {
                retimed = retimer.push(chunk);
                &retimed
            } else {
                chunk
            };
            let ok = out(chunk);
            self.update_playback(token, |st| st.bytes = total);
            ok
        };
        let result = if let Some(entry) = &cached {
            log(&format!(
                "playback {} {from}-{to}: from the cache",
                cam.name
            ));
            let until = to.min(entry.covered_end());
            let pace = if fast && speed <= 1.0 {
                None
            } else {
                Some(speed)
            };
            super::cache::play_file(entry, from, until, pace, &mut sink).map_err(|e| {
                super::recordings::PullError::Relay(super::relay::RelayError::Transport(
                    e.to_string(),
                ))
            })
        } else if fast {
            recordings.pull(from, to, &mut sink)
        } else {
            recordings.play(from, to, &mut sink)
        };
        if let Some(w) = keep {
            match w.finish(result.is_ok()) {
                Ok(Some(e)) => log(&format!(
                    "playback {} {from}-{to}: kept {:.0} s in the cache",
                    cam.name, e.seconds
                )),
                Ok(None) => {}
                Err(e) => log(&format!("playback {}: cache write failed: {e}", cam.name)),
            }
        }
        let outcome = match &result {
            Ok(_) => "finished".to_string(),
            Err(e) => format!("ended: {e}"),
        };
        log(&format!(
            "playback {} {from}-{to}: {outcome} after {:.1} s, {} KB",
            cam.name,
            started.elapsed().as_secs_f32(),
            total / 1000
        ));
        self.update_playback(token, |st| {
            st.finished = true;
            st.bytes = total;
            if let Err(e) = &result {
                st.error = e.to_string();
            }
        });
    }
}
