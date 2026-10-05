//! The settings behind a Brother's Scan button, kept exactly where Brother
//! keeps them: one `scanto<action>.config` per action, the system copy under
//! `/etc/opt/brother/scanner/brscan-skey/` and an optional per-user copy
//! under `~/.brscan-skey/` that Brother's scripts read first. Kagaz only
//! ever writes the per-user copy, in Brother's own layout, and "back to
//! default" means deleting it.

use super::vendor::{parse_brother_settings, BrotherSettings};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const ACTIONS: &[&str] = &["file", "image", "ocr", "email"];
pub const RESOLUTIONS: &[u32] = &[100, 150, 200, 300, 400, 600, 1200, 2400, 4800, 9600];
pub const SIZES: &[&str] = &["MAX", "A3", "A4", "A5", "A6", "Letter", "Legal"];
pub const SYSTEM_DIR: &str = "/etc/opt/brother/scanner/brscan-skey";

/// Where a setting currently comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    /// `~/.brscan-skey/<file>`, written by the user or by Kagaz.
    User,
    /// Brother's shipped file under /etc/opt.
    System,
    /// Neither file exists; Brother's script defaults apply.
    BuiltIn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSettings {
    pub action: String,
    pub resolution: u32,
    pub duplex: bool,
    pub size: String,
    pub origin: Origin,
    pub user_file: PathBuf,
}

fn user_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".brscan-skey")
}

fn file_name(action: &str) -> String {
    format!("scanto{action}.config")
}

/// Is Brother's scan-key tool installed here at all?
pub fn available() -> bool {
    std::path::Path::new(SYSTEM_DIR).is_dir()
}

/// The settings in force for one action, and where they come from.
pub fn get(action: &str) -> ActionSettings {
    let user_file = user_dir().join(file_name(action));
    let system_file = PathBuf::from(SYSTEM_DIR).join(file_name(action));
    let (text, origin) = match std::fs::read_to_string(&user_file) {
        Ok(t) => (t, Origin::User),
        Err(_) => match std::fs::read_to_string(&system_file) {
            Ok(t) => (t, Origin::System),
            Err(_) => (String::new(), Origin::BuiltIn),
        },
    };
    let s = parse_brother_settings(&text);
    ActionSettings {
        action: action.to_string(),
        resolution: s.resolution,
        duplex: s.duplex,
        size: s.size,
        origin,
        user_file,
    }
}

pub fn get_all() -> Vec<ActionSettings> {
    ACTIONS.iter().map(|a| get(a)).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("unknown action \"{0}\"; use file, image, ocr or email")]
    Action(String),
    #[error("resolution {0} is not one Brother accepts (100, 150, 200, 300, 400, 600, 1200, 2400, 4800, 9600)")]
    Resolution(u32),
    #[error("size \"{0}\" is not one Brother accepts (MAX, A3, A4, A5, A6, Letter, Legal, or WIDTHxHEIGHT in mm)")]
    Size(String),
    #[error("cannot write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub fn check_action(action: &str) -> Result<(), SettingsError> {
    if ACTIONS.contains(&action) {
        Ok(())
    } else {
        Err(SettingsError::Action(action.to_string()))
    }
}

fn valid_size(size: &str) -> bool {
    SIZES.iter().any(|s| s.eq_ignore_ascii_case(size))
        || size
            .split_once('x')
            .map(|(w, h)| w.parse::<u32>().is_ok() && h.parse::<u32>().is_ok())
            .unwrap_or(false)
}

/// Brother's file, with the values swapped in. Starts from the system
/// file so Brother's own comments stay; falls back to a copy of their layout.
pub fn render(action: &str, s: &BrotherSettings) -> String {
    let system_file = PathBuf::from(SYSTEM_DIR).join(file_name(action));
    let template = std::fs::read_to_string(system_file).unwrap_or_else(|_| {
        "#[brscan-skey]\n\n# resolution=100,150,200,300,400,600,1200,2400,4800,9600\nresolution=100\n\n#duplex=OFF,ON\nduplex=OFF\n\n#size=MAX,A3,A4,A5,A6,Letter,Legal,${width}x${height} (mm)\nsize=A4\n".to_string()
    });
    let mut seen = (false, false, false);
    let mut out: Vec<String> = template
        .lines()
        .map(|line| {
            let t = line.trim();
            if t.starts_with("resolution=") {
                seen.0 = true;
                format!("resolution={}", s.resolution)
            } else if t.starts_with("duplex=") {
                seen.1 = true;
                format!("duplex={}", if s.duplex { "ON" } else { "OFF" })
            } else if t.starts_with("size=") {
                seen.2 = true;
                format!("size={}", s.size)
            } else {
                line.to_string()
            }
        })
        .collect();
    if !seen.0 {
        out.push(format!("resolution={}", s.resolution));
    }
    if !seen.1 {
        out.push(format!("duplex={}", if s.duplex { "ON" } else { "OFF" }));
    }
    if !seen.2 {
        out.push(format!("size={}", s.size));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Write the per-user file for `action` with these values.
pub fn set(
    action: &str,
    resolution: u32,
    duplex: bool,
    size: &str,
) -> Result<ActionSettings, SettingsError> {
    check_action(action)?;
    if !RESOLUTIONS.contains(&resolution) {
        return Err(SettingsError::Resolution(resolution));
    }
    if !valid_size(size) {
        return Err(SettingsError::Size(size.to_string()));
    }
    let size = SIZES
        .iter()
        .find(|s| s.eq_ignore_ascii_case(size))
        .map(|s| s.to_string())
        .unwrap_or_else(|| size.to_string());
    let dir = user_dir();
    std::fs::create_dir_all(&dir).map_err(|source| SettingsError::Io {
        path: dir.clone(),
        source,
    })?;
    let path = dir.join(file_name(action));
    let text = render(
        action,
        &BrotherSettings {
            resolution,
            duplex,
            size,
        },
    );
    std::fs::write(&path, text).map_err(|source| SettingsError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(get(action))
}

/// Remove the per-user file so Brother's own default applies again.
pub fn reset(action: &str) -> Result<ActionSettings, SettingsError> {
    check_action(action)?;
    let path = user_dir().join(file_name(action));
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(SettingsError::Io { path, source }),
    }
    Ok(get(action))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_in_brothers_layout_keeping_comments() {
        let text = render(
            "nosuchaction",
            &BrotherSettings {
                resolution: 300,
                duplex: true,
                size: "Letter".into(),
            },
        );
        assert!(text.starts_with("#[brscan-skey]\n"));
        assert!(text.contains("# resolution=100,150,200,300"));
        assert!(text.contains("\nresolution=300\n"));
        assert!(text.contains("\nduplex=ON\n"));
        assert!(text.contains("\nsize=Letter\n"));
        let back = parse_brother_settings(&text);
        assert_eq!(back.resolution, 300);
        assert!(back.duplex);
        assert_eq!(back.size, "Letter");
    }

    #[test]
    fn validates_like_brother() {
        assert!(check_action("image").is_ok());
        assert!(matches!(
            check_action("photo"),
            Err(SettingsError::Action(_))
        ));
        assert!(valid_size("a4"));
        assert!(valid_size("210x297"));
        assert!(!valid_size("huge"));
        assert!(matches!(
            set("file", 250, false, "A4"),
            Err(SettingsError::Resolution(250))
        ));
    }
}
