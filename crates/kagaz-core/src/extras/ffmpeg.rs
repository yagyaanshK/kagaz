//! ffmpeg as an optional extra, for the one job nothing else does here:
//! putting the camera's sound into a saved MP4 as AAC. Downloaded on first
//! use from the static builds below, checked against a pinned checksum,
//! kept in the extras folder; never bundled.

pub use super::go2rtc::{Event, ExtraError};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which build to fetch per platform: (download URL, sha256, path inside
/// the archive). Linux: John Van Sickle's static builds (versioned
/// addresses under old-releases stay put); Windows: gyan.dev essentials;
/// macOS: evermeet.cx (x86_64, runs on Apple silicon through Rosetta).
pub fn asset_for(os: &str, arch: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match (os, arch) {
        ("linux", "x86_64") => Some((
            "https://johnvansickle.com/ffmpeg/old-releases/ffmpeg-6.0.1-amd64-static.tar.xz",
            SHA_LINUX_AMD64,
            "ffmpeg-6.0.1-amd64-static/ffmpeg",
        )),
        ("linux", "aarch64") => Some((
            "https://johnvansickle.com/ffmpeg/old-releases/ffmpeg-6.0.1-arm64-static.tar.xz",
            SHA_LINUX_ARM64,
            "ffmpeg-6.0.1-arm64-static/ffmpeg",
        )),
        ("windows", "x86_64") => Some((
            "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0.2-essentials_build.zip",
            SHA_WINDOWS,
            "ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe",
        )),
        ("macos", _) => Some((
            "https://evermeet.cx/ffmpeg/ffmpeg-9.0.2.zip",
            SHA_MACOS,
            "ffmpeg",
        )),
        _ => None,
    }
}

const SHA_LINUX_AMD64: &str = "28268bf402f1083833ea269331587f60a242848880073be8016501d864bd07a5";
const SHA_LINUX_ARM64: &str = "7dbd8e2f47bd83de591b9d6ea70e67d32d9aa97e7d47ae402b60c2fe3fd4d0ab";
const SHA_WINDOWS: &str = "__SHA_WINDOWS__";
const SHA_MACOS: &str = "__SHA_MACOS__";

/// Where the binary lives once fetched.
pub fn binary_path() -> PathBuf {
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    super::go2rtc::extras_dir().join("ffmpeg").join(name)
}

/// Is it already here?
pub fn present() -> bool {
    binary_path().is_file()
}

/// The ffmpeg binary, downloading and verifying it first when needed.
pub fn ensure(on_event: &mut dyn FnMut(Event)) -> Result<PathBuf, ExtraError> {
    let path = binary_path();
    if path.is_file() {
        on_event(Event::Ready(path.clone()));
        return Ok(path);
    }
    let (url, sha, member) =
        asset_for(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
            ExtraError::Unsupported(format!(
                "{} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
        })?;
    let response = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(600))
        .build()
        .get(url)
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
    if url.ends_with(".zip") {
        super::unzip_member(&bytes, &|name| name == member, &path)?;
    } else {
        untar_xz_member(&bytes, member, &path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    on_event(Event::Ready(path.clone()));
    Ok(path)
}

/// Pull one file out of a .tar.xz (pure Rust, so it takes a few seconds).
pub fn untar_xz_member(xz: &[u8], member: &str, dest: &Path) -> Result<(), ExtraError> {
    let mut tar = Vec::new();
    lzma_rs::xz_decompress(&mut std::io::Cursor::new(xz), &mut tar)
        .map_err(|e| ExtraError::Download(format!("xz: {e}")))?;
    let mut archive = tar::Archive::new(std::io::Cursor::new(tar));
    let entries = archive
        .entries()
        .map_err(|e| ExtraError::Download(format!("tar: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| ExtraError::Download(format!("tar: {e}")))?;
        let path = entry
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if path.trim_start_matches("./") == member {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| ExtraError::Download(format!("tar: {e}")))?;
            return std::fs::write(dest, bytes).map_err(|source| ExtraError::Io {
                path: dest.to_path_buf(),
                source,
            });
        }
    }
    Err(ExtraError::Download(format!("archive without {member}")))
}

/// Mux a camera's MPEG-TS (video copied) with its raw G.711 A-law sound
/// (encoded to AAC) into an MP4. `audio_offset` is how many seconds after
/// the picture the sound starts.
pub fn mux_mp4(
    ffmpeg: &Path,
    ts: &Path,
    alaw: Option<(&Path, f64)>,
    out: &Path,
) -> Result<(), ExtraError> {
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.arg("-y")
        .arg("-nostdin")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(ts);
    if let Some((alaw, offset)) = alaw {
        cmd.arg("-itsoffset")
            .arg(format!("{offset:.3}"))
            .arg("-f")
            .arg("alaw")
            .arg("-ar")
            .arg("8000")
            .arg("-ac")
            .arg("1")
            .arg("-i")
            .arg(alaw)
            .arg("-map")
            .arg("0:v:0")
            .arg("-map")
            .arg("1:a:0")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("64k");
    } else {
        cmd.arg("-map").arg("0:v:0");
    }
    cmd.arg("-c:v")
        .arg("copy")
        .arg("-movflags")
        .arg("+faststart")
        .arg(out);
    let output = cmd
        .output()
        .map_err(|e| ExtraError::Start(format!("ffmpeg: {e}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        return Err(ExtraError::Start(format!("ffmpeg failed: {tail}")));
    }
    Ok(())
}
