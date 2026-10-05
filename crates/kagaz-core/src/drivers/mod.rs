//! The driver database: which official vendor package a device needs on
//! each OS, and the steps around installing it. Files live in `drivers/`
//! at the repository root and are embedded at build time; set
//! `KAGAZ_DRIVERS_DIR` to read a folder instead while editing an entry.

pub mod brother;
pub mod install;

use crate::Device;
use serde::{Deserialize, Serialize};
use std::path::Path;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/drivers_embedded.rs"));
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSection {
    pub manufacturer: String,
    pub models: Vec<String>,
    #[serde(default)]
    pub functions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    /// deb | rpm | exe | pkg | dmg | sh
    pub kind: String,
    #[serde(default = "any")]
    pub arch: String,
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

fn any() -> String {
    "any".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsEntry {
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default)]
    pub admin_steps: Vec<String>,
    #[serde(default)]
    pub user_steps: Vec<String>,
    #[serde(default)]
    pub remove_steps: Vec<String>,
    #[serde(default)]
    pub remove_user_steps: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub model: ModelSection,
    #[serde(default)]
    pub linux: Option<OsEntry>,
    #[serde(default)]
    pub windows: Option<OsEntry>,
    #[serde(default)]
    pub macos: Option<OsEntry>,
    /// Where the entry came from, for messages.
    #[serde(skip)]
    pub source: String,
}

impl Entry {
    pub fn for_os(&self, os: crate::Os) -> Option<&OsEntry> {
        match os {
            crate::Os::Linux => self.linux.as_ref(),
            crate::Os::Windows => self.windows.as_ref(),
            crate::Os::MacOs => self.macos.as_ref(),
            crate::Os::Other => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    #[error("cannot read {0}")]
    Io(String),
}

/// "Brother DCP-L2540DW series" -> "DCPL2540DW": the key both the database
/// and Brother's own servers use.
pub fn model_key(manufacturer: Option<&str>, model: &str) -> String {
    let mut m = model.trim().to_string();
    if let Some(mf) = manufacturer {
        if m.to_ascii_lowercase().starts_with(&mf.to_ascii_lowercase()) {
            m = m[mf.len()..].to_string();
        }
    }
    let lower = m.to_ascii_lowercase();
    let stripped = lower.strip_suffix(" series").unwrap_or(&lower);
    stripped
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Load every entry: from `KAGAZ_DRIVERS_DIR` when set, else the embedded copy.
pub fn load() -> Result<Vec<Entry>, DbError> {
    if let Some(dir) = std::env::var_os("KAGAZ_DRIVERS_DIR") {
        return load_dir(Path::new(&dir));
    }
    embedded::EMBEDDED
        .iter()
        .map(|(path, text)| parse(path, text))
        .collect()
}

pub fn load_dir(dir: &Path) -> Result<Vec<Entry>, DbError> {
    let mut files = Vec::new();
    collect(dir, &mut files).map_err(|e| DbError::Io(format!("{}: {e}", dir.display())))?;
    files.sort();
    files
        .iter()
        .map(|p| {
            let text = std::fs::read_to_string(p)
                .map_err(|e| DbError::Io(format!("{}: {e}", p.display())))?;
            parse(&p.display().to_string(), &text)
        })
        .collect()
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "toml") {
            out.push(p);
        }
    }
    Ok(())
}

pub fn parse(path: &str, text: &str) -> Result<Entry, DbError> {
    let mut entry: Entry = toml::from_str(text).map_err(|e| DbError::Parse {
        path: path.to_string(),
        message: e.to_string(),
    })?;
    entry.source = path.to_string();
    Ok(entry)
}

/// The entry for `device`, if the database has one.
pub fn find<'a>(entries: &'a [Entry], device: &Device) -> Option<&'a Entry> {
    let model = device.model.as_deref().or(if device.name.is_empty() {
        None
    } else {
        Some(device.name.as_str())
    })?;
    let key = model_key(device.manufacturer.as_deref(), model);
    if key.is_empty() {
        return None;
    }
    entries.iter().find(|e| {
        device
            .manufacturer
            .as_deref()
            .map(|mf| mf.eq_ignore_ascii_case(&e.model.manufacturer))
            .unwrap_or(true)
            && e.model
                .models
                .iter()
                .any(|m| model_key(Some(&e.model.manufacturer), m) == key)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_keys_normalise() {
        assert_eq!(
            model_key(Some("Brother"), "Brother DCP-L2540DW series"),
            "DCPL2540DW"
        );
        assert_eq!(model_key(None, "DCP-L2540DW"), "DCPL2540DW");
        assert_eq!(model_key(Some("HP"), "HP LaserJet 1020"), "LASERJET1020");
        assert_eq!(model_key(None, ""), "");
    }

    #[test]
    fn embedded_database_parses_and_matches_the_brother() {
        let entries = load().unwrap();
        assert!(!entries.is_empty());
        let device = Device {
            name: "Brother DCP-L2540DW series".into(),
            manufacturer: Some("Brother".into()),
            model: Some("DCP-L2540DW series".into()),
            ..Device::default()
        };
        let e = find(&entries, &device).expect("entry");
        assert_eq!(e.model.manufacturer, "Brother");
        let linux = e.linux.as_ref().unwrap();
        assert_eq!(linux.packages.len(), 2);
        assert_eq!(linux.packages[0].name, "brscan4");
        assert_eq!(linux.packages[0].sha256.as_deref().map(str::len), Some(64));
        assert!(linux
            .admin_steps
            .iter()
            .any(|s| s.contains("brsaneconfig4")));
        assert!(e.windows.is_none());

        let other = Device {
            name: "HP LaserJet 1020".into(),
            manufacturer: Some("HP".into()),
            ..Device::default()
        };
        assert!(find(&entries, &other).is_none());
    }

    #[test]
    fn parse_errors_name_the_file() {
        let err = parse("x.toml", "[model]\nmanufacturer = 1\n").unwrap_err();
        assert!(matches!(err, DbError::Parse { ref path, .. } if path == "x.toml"));
    }
}
