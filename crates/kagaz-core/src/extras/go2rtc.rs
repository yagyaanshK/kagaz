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
        unzip_single(&bytes, &path)?;
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

/// Extract the first `go2rtc*` file from a zip (stored or deflated).
fn unzip_single(zip: &[u8], dest: &Path) -> Result<(), ExtraError> {
    // Minimal zip reader: walk local file headers.
    let mut pos = 0;
    while pos + 30 <= zip.len() && &zip[pos..pos + 4] == b"PK\x03\x04" {
        let method = u16::from_le_bytes([zip[pos + 8], zip[pos + 9]]);
        let csize = u32::from_le_bytes([zip[pos + 18], zip[pos + 19], zip[pos + 20], zip[pos + 21]])
            as usize;
        let name_len = u16::from_le_bytes([zip[pos + 26], zip[pos + 27]]) as usize;
        let extra_len = u16::from_le_bytes([zip[pos + 28], zip[pos + 29]]) as usize;
        let name = String::from_utf8_lossy(&zip[pos + 30..pos + 30 + name_len]).to_string();
        let data_start = pos + 30 + name_len + extra_len;
        let data = &zip[data_start..(data_start + csize).min(zip.len())];
        if name.starts_with("go2rtc") {
            let bytes = match method {
                0 => data.to_vec(),
                8 => {
                    let mut out = Vec::new();
                    flate2::read::DeflateDecoder::new(data)
                        .read_to_end(&mut out)
                        .map_err(|e| ExtraError::Download(format!("zip: {e}")))?;
                    out
                }
                m => {
                    return Err(ExtraError::Download(format!(
                        "zip method {m} not supported"
                    )))
                }
            };
            return std::fs::write(dest, bytes).map_err(|source| ExtraError::Io {
                path: dest.to_path_buf(),
                source,
            });
        }
        pos = data_start + csize;
    }
    Err(ExtraError::Download("zip without a go2rtc file".into()))
}

/// One stream for go2rtc: a name and its source URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSource {
    pub name: String,
    pub url: String,
}

/// go2rtc's configuration for the given streams; API on 127.0.0.1 only.
pub fn config_yaml(api_port: u16, streams: &[StreamSource]) -> String {
    let mut y = format!("api:\n  listen: \"127.0.0.1:{api_port}\"\nrtsp:\n  listen: \"\"\nwebrtc:\n  listen: \"\"\nlog:\n  level: warn\nstreams:\n");
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

/// A running go2rtc, stopped when dropped.
pub struct Engine {
    child: Child,
    pub api_port: u16,
    pub config_path: PathBuf,
}

impl Engine {
    pub fn start(
        binary: &Path,
        api_port: u16,
        streams: &[StreamSource],
    ) -> Result<Engine, ExtraError> {
        let config_path = extras_dir().join("go2rtc.yaml");
        std::fs::write(&config_path, config_yaml(api_port, streams)).map_err(|source| {
            ExtraError::Io {
                path: config_path.clone(),
                source,
            }
        })?;
        let child = Command::new(binary)
            .arg("-c")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| ExtraError::Start(e.to_string()))?;
        // Wait briefly for the API to come up.
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
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
                });
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let mut child = child;
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

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
