//! go2rtc, the video engine for cameras: an MIT-licensed single binary that
//! takes RTSP, ONVIF or MPEG-TS-over-HTTP in and gives the window streams it
//! can play. Never shipped with Kagaz; downloaded once on first use, pinned
//! by version and checksum, kept in the extras folder.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub const VERSION: &str = "v1.9.14";

/// (asset name, sha256) per platform, from the GitHub release of `VERSION`.
pub fn asset_for(os: &str, arch: &str) -> Option<(&'static str, &'static str)> {
    match (os, arch) {
        ("linux", "x86_64") => Some((
            "go2rtc_linux_amd64",
            "32d616af226bd731678ffde328b94cfb94e30339bfefc469cfb76323144615a6",
        )),
        ("linux", "aarch64") => Some((
            "go2rtc_linux_arm64",
            "359fabade8a7a51e81a55fe6df6b0ef81764a5e1d63179577534eaaa71904b50",
        )),
        ("windows", "x86_64") => Some((
            "go2rtc_win64.zip",
            "dd4167d75cb04abe618855b7c71f8658bd009f60c1a71835d134d2c11c939907",
        )),
        ("macos", "x86_64") => Some((
            "go2rtc_mac_amd64.zip",
            "9b0b9a27a4dc3a5b8b93376e7e8fc2787c6af624a512842622be84aec0171c7a",
        )),
        ("macos", "aarch64") => Some((
            "go2rtc_mac_arm64.zip",
            "919b78adc759d6b3883d1e1b2ac915ac0985bb903ff1897b4d228527bd64690c",
        )),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtraError {
    #[error("no go2rtc build for {0}")]
    Unsupported(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error("the downloaded go2rtc does not match the pinned checksum (got {0}); not keeping it")]
    Checksum(String),
    #[error("cannot write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("go2rtc did not start: {0}")]
    Start(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Downloading { bytes: u64 },
    Verified,
    Ready(PathBuf),
}

/// Where extras live: `KAGAZ_EXTRAS_DIR`, else `<cache>/extras`.
pub fn extras_dir() -> PathBuf {
    std::env::var_os("KAGAZ_EXTRAS_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| crate::paths::cache_dir().join("extras"))
}

fn binary_path() -> PathBuf {
    let name = if cfg!(windows) {
        "go2rtc.exe"
    } else {
        "go2rtc"
    };
    extras_dir().join(format!("go2rtc-{VERSION}")).join(name)
}

/// The go2rtc binary, downloaded and verified if not already there.
pub fn ensure(on_event: &mut dyn FnMut(Event)) -> Result<PathBuf, ExtraError> {
    let path = binary_path();
    if path.is_file() {
        on_event(Event::Ready(path.clone()));
        return Ok(path);
    }
    let (asset, sha) =
        asset_for(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
            ExtraError::Unsupported(format!(
                "{} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
        })?;
    let url = format!("https://github.com/AlexxIT/go2rtc/releases/download/{VERSION}/{asset}");
    let response = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(300))
        .build()
        .get(&url)
        .call()
        .map_err(|e| ExtraError::Download(e.to_string()))?;
    let size = response
        .header("Content-Length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    on_event(Event::Downloading { bytes: size });
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| ExtraError::Download(e.to_string()))?;
    let got = {
        use sha2::Digest;
        sha2::Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    if got != sha {
        return Err(ExtraError::Checksum(got));
    }
    on_event(Event::Verified);
    let dir = path.parent().expect("binary has a parent");
    std::fs::create_dir_all(dir).map_err(|source| ExtraError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    if asset.ends_with(".zip") {
        super::unzip_member(&bytes, &|name| name.starts_with("go2rtc"), &path)?;
    } else {
        std::fs::File::create(&path)
            .and_then(|mut f| f.write_all(&bytes))
            .map_err(|source| ExtraError::Io {
                path: path.clone(),
                source,
            })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    on_event(Event::Ready(path.clone()));
    Ok(path)
}

/// One stream for go2rtc: a name and its source URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSource {
    pub name: String,
    pub url: String,
}

/// go2rtc's configuration for the given streams; API on 127.0.0.1 only.
pub fn config_yaml(api_port: u16, streams: &[StreamSource]) -> String {
    let level = std::env::var("KAGAZ_GO2RTC_LOG").unwrap_or_else(|_| "info".into());
    let mut y = format!("api:\n  listen: \"127.0.0.1:{api_port}\"\n  origin: \"*\"\nrtsp:\n  listen: \"\"\nwebrtc:\n  listen: \"\"\nlog:\n  level: {level}\n  format: text\nstreams:\n");
    for s in streams {
        y.push_str(&format!(
            "  {}: \"{}\"\n",
            yaml_key(&s.name),
            s.url.replace('"', "")
        ));
    }
    y
}

/// A safe stream name: letters, digits, underscores.
pub fn yaml_key(name: &str) -> String {
    let k: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if k.is_empty() {
        "stream".into()
    } else {
        k
    }
}

/// A free TCP port on 127.0.0.1, so a stale engine from an earlier run can
/// never be mistaken for ours.
pub fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()?
        .port())
}

/// A running go2rtc, stopped when dropped.
pub struct Engine {
    child: Child,
    pub api_port: u16,
    pub config_path: PathBuf,
    /// Keeps the spawning thread alive for as long as the engine runs: on
    /// Linux the parent-death signal is tied to the spawning *thread*, and a
    /// pool thread that retires would take the engine down with it.
    keeper: Option<std::sync::mpsc::Sender<()>>,
    /// Stream additions and removals go one at a time: go2rtc 1.9.14 can
    /// crash ("concurrent map writes") when several arrive together.
    api_lock: std::sync::Mutex<()>,
}

impl Engine {
    /// Start the engine on `api_port` (0 = pick a free one) with these streams.
    pub fn start(
        binary: &Path,
        api_port: u16,
        streams: &[StreamSource],
    ) -> Result<Engine, ExtraError> {
        let api_port = if api_port == 0 {
            free_port().map_err(|e| ExtraError::Start(e.to_string()))?
        } else {
            api_port
        };
        let config_path = extras_dir().join(format!("go2rtc-{api_port}.yaml"));
        std::fs::write(&config_path, config_yaml(api_port, streams)).map_err(|source| {
            ExtraError::Io {
                path: config_path.clone(),
                source,
            }
        })?;
        let log_path = extras_dir().join("go2rtc.log");
        let log = std::fs::File::create(&log_path).map_err(|source| ExtraError::Io {
            path: log_path.clone(),
            source,
        })?;
        let log_err = log.try_clone().map_err(|source| ExtraError::Io {
            path: log_path.clone(),
            source,
        })?;
        let mut command = Command::new(binary);
        command
            .arg("-c")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // If Kagaz dies without cleaning up, the engine goes with it.
            // SAFETY: prctl only changes this child's own death signal.
            unsafe {
                command.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    Ok(())
                });
            }
        }
        // Spawn from a dedicated thread that stays alive until the engine is
        // dropped (see `keeper`), then hand the child back.
        let (child_tx, child_rx) = std::sync::mpsc::channel();
        let (keeper_tx, keeper_rx) = std::sync::mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("kagaz-engine-keeper".into())
            .spawn(move || {
                let spawned = command.spawn();
                let _ = child_tx.send(spawned);
                // Block until the engine is dropped; only then may this thread end.
                let _ = keeper_rx.recv();
            })
            .map_err(|e| ExtraError::Start(e.to_string()))?;
        let mut child = child_rx
            .recv()
            .map_err(|_| ExtraError::Start("engine thread died".into()))?
            .map_err(|e| ExtraError::Start(e.to_string()))?;
        // Ready when the port opens while our child is still alive.
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
            if let Ok(Some(status)) = child.try_wait() {
                let tail = std::fs::read_to_string(&log_path).unwrap_or_default();
                let tail: String = tail.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
                return Err(ExtraError::Start(format!("exited with {status}: {tail}")));
            }
            if std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], api_port)),
                Duration::from_millis(200),
            )
            .is_ok()
            {
                return Ok(Engine {
                    child,
                    api_port,
                    config_path,
                    keeper: Some(keeper_tx),
                    api_lock: std::sync::Mutex::new(()),
                });
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = child.kill();
        Err(ExtraError::Start(format!("port {api_port} did not open")))
    }

    /// The URL of a stream as fragmented MP4 over HTTP, for a `<video>` element.
    pub fn mp4_url(&self, name: &str) -> String {
        format!(
            "http://127.0.0.1:{}/api/stream.mp4?src={}",
            self.api_port,
            yaml_key(name)
        )
    }

    /// The URL of a stream as MPEG-TS over HTTP: what WebKit's media stack
    /// plays best in a plain `<video>` (its MP4 output is refused there).
    pub fn ts_url(&self, name: &str) -> String {
        format!(
            "http://127.0.0.1:{}/api/stream.ts?src={}",
            self.api_port,
            yaml_key(name)
        )
    }

    /// The WebSocket URL go2rtc's MSE/WebRTC player uses.
    pub fn ws_url(&self, name: &str) -> String {
        format!(
            "ws://127.0.0.1:{}/api/ws?src={}",
            self.api_port,
            yaml_key(name)
        )
    }

    pub fn ui_url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.api_port)
    }
}

impl Engine {
    /// Add a stream while the engine runs (go2rtc's `PUT /api/streams`).
    /// The name may hold letters, digits, `_` and `-`.
    pub fn add_stream(&self, name: &str, url: &str) -> Result<(), ExtraError> {
        let api = format!(
            "http://127.0.0.1:{}/api/streams?name={}&src={}",
            self.api_port,
            name,
            url_encode(url)
        );
        let _one_at_a_time = self.api_lock.lock().unwrap_or_else(|p| p.into_inner());
        match ureq::put(&api).call() {
            Ok(_) => Ok(()),
            Err(e) => Err(ExtraError::Start(format!("engine refused the stream: {e}"))),
        }
    }

    /// Is the engine process still running?
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Remove a stream added with `add_stream`, ending its source.
    pub fn remove_stream(&self, name: &str) {
        let api = format!("http://127.0.0.1:{}/api/streams?src={name}", self.api_port);
        let _one_at_a_time = self.api_lock.lock().unwrap_or_else(|p| p.into_inner());
        let _ = ureq::delete(&api).call();
    }

    /// Read a stream from the engine as fragmented MP4 and hand the bytes to
    /// `sink` until the source has finished (`done` says so) and the engine
    /// has gone quiet, or `sink` returns `false`. Returns the bytes written.
    ///
    /// Plain HTTP/1.0 by hand: the engine answers only once the source has
    /// produced its first frames (a relay session takes several seconds),
    /// and HTTP/1.0 makes it send the body unchunked and close at the end.
    pub fn save_mp4(
        &self,
        name: &str,
        done: &mut dyn FnMut() -> bool,
        sink: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<u64, ExtraError> {
        let err = |e: std::io::Error| ExtraError::Start(format!("engine stream: {e}"));
        let mut sock = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], self.api_port)),
            Duration::from_secs(5),
        )
        .map_err(err)?;
        sock.write_all(
            format!(
                "GET /api/stream.mp4?src={name} HTTP/1.0\r\nHost: 127.0.0.1:{}\r\n\r\n",
                self.api_port
            )
            .as_bytes(),
        )
        .map_err(err)?;
        // The head: up to a minute for the source to start.
        sock.set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(err)?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match sock.read(&mut byte) {
                Ok(1) => {
                    head.push(byte[0]);
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    if head.len() > 16 * 1024 {
                        return Err(ExtraError::Start("engine sent an oversized head".into()));
                    }
                }
                Ok(_) => return Err(ExtraError::Start("engine closed before answering".into())),
                Err(e) => {
                    return Err(ExtraError::Start(format!(
                        "the source produced no video: {e}"
                    )))
                }
            }
        }
        let head = String::from_utf8_lossy(&head);
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if status != 200 {
            return Err(ExtraError::Start(format!(
                "engine answered {}",
                head.lines().next().unwrap_or("")
            )));
        }
        // The body: short reads, so the end of the source is noticed quickly.
        sock.set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(err)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        let mut quiet = 0u32;
        loop {
            match sock.read(&mut buf) {
                Ok(0) => return Ok(total),
                Ok(n) => {
                    quiet = 0;
                    total += n as u64;
                    if !sink(&buf[..n]) {
                        return Ok(total);
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    quiet += 1;
                    if done() {
                        return Ok(total);
                    }
                    if quiet >= 45 {
                        return Err(ExtraError::Start("no video arrived for 90 s".into()));
                    }
                }
                Err(e) => {
                    if done() {
                        return Ok(total);
                    }
                    return Err(ExtraError::Start(format!("engine stream ended: {e}")));
                }
            }
        }
    }
}

/// Percent-encode a query value.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Release the keeper thread now that the engine is gone.
        self.keeper.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_keys() {
        let y = config_yaml(
            1984,
            &[
                StreamSource {
                    name: "front door".into(),
                    url: "http://127.0.0.1:5000/tapo/X.ts".into(),
                },
                StreamSource {
                    name: "".into(),
                    url: "rtsp://u:p@10.0.0.2/stream1".into(),
                },
            ],
        );
        assert!(y.contains("listen: \"127.0.0.1:1984\""));
        assert!(y.contains("  front_door: \"http://127.0.0.1:5000/tapo/X.ts\"\n"));
        assert!(y.contains("  stream: \"rtsp://u:p@10.0.0.2/stream1\"\n"));
        assert_eq!(yaml_key("Cam_2"), "Cam_2");
        assert!(asset_for("linux", "x86_64").is_some());
        assert!(asset_for("freebsd", "x86_64").is_none());
    }
}
