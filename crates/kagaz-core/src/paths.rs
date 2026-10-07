//! Where Kagaz keeps things on each OS, without a directories crate.

use std::path::PathBuf;

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The per-user cache directory for Kagaz: `$XDG_CACHE_HOME/kagaz` or
/// `~/.cache/kagaz` on Linux, `~/Library/Caches/kagaz` on macOS,
/// `%LOCALAPPDATA%\kagaz\cache` on Windows.
pub fn cache_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(home)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("kagaz")
            .join("cache")
    } else if cfg!(target_os = "macos") {
        home()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Caches/kagaz")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home().map(|h| h.join(".cache")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("kagaz")
    }
}

/// Where downloaded vendor packages go: `KAGAZ_DOWNLOAD_DIR` when set, else
/// `<cache>/drivers`.
pub fn download_dir() -> PathBuf {
    std::env::var_os("KAGAZ_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| cache_dir().join("drivers"))
}

/// The default folder for scans started from a device's own button.
pub fn scans_dir() -> PathBuf {
    home().unwrap_or_else(|| PathBuf::from(".")).join("Scans")
}

/// The per-user configuration directory: `$XDG_CONFIG_HOME/kagaz` or
/// `~/.config/kagaz` on Linux, `~/Library/Application Support/kagaz` on
/// macOS, `%APPDATA%\\kagaz` on Windows.
pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(home)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("kagaz")
    } else if cfg!(target_os = "macos") {
        home()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/kagaz")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home().map(|h| h.join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("kagaz")
    }
}

/// Where fetched camera recordings are kept: `KAGAZ_RECORDINGS_DIR`, else
/// `recordings_dir` in `<config>/settings.toml`, else `<cache>/recordings`.
pub fn recordings_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KAGAZ_RECORDINGS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    setting("recordings_dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| cache_dir().join("recordings"))
}

/// One string value from `<config>/settings.toml`, if set.
pub fn setting(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_dir().join("settings.toml")).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
