//! How the Cameras view is arranged: named groups ("rooms") of cameras,
//! kept in `<config>/cameras.toml`. Cameras are referred to by device id;
//! anything not in a group shows under "Ungrouped".

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    #[serde(default)]
    pub cameras: Vec<String>,
    #[serde(default)]
    pub collapsed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Layout {
    #[serde(default)]
    pub groups: Vec<Group>,
    /// Order of the ungrouped cameras, by device id (others follow).
    #[serde(default)]
    pub ungrouped: Vec<String>,
    #[serde(default)]
    pub ungrouped_collapsed: bool,
    /// Cameras shown in HD even as small tiles.
    #[serde(default)]
    pub hd: Vec<String>,
    /// Sound volume per camera, 0 to 100 (absent: 100).
    #[serde(default)]
    pub volume: std::collections::BTreeMap<String, u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("cannot read or write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not a valid layout: {0}", path = .1.display())]
    Parse(String, PathBuf),
}

impl Layout {
    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("cameras.toml")
    }

    /// The saved layout, or an empty one when there is none yet.
    pub fn load(path: &Path) -> Result<Layout, LayoutError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Layout::parse(&text).map_err(|e| LayoutError::Parse(e, path.to_path_buf())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Layout::default()),
            Err(source) => Err(LayoutError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn parse(text: &str) -> Result<Layout, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn save(&self, path: &Path) -> Result<(), LayoutError> {
        let io = |source| LayoutError::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| LayoutError::Parse(e.to_string(), path.to_path_buf()))?;
        std::fs::write(path, text).map_err(io)
    }

    /// Drop cameras that no longer exist and make every camera appear once.
    pub fn reconcile(&mut self, known: &[String]) {
        let mut seen = std::collections::HashSet::new();
        for g in &mut self.groups {
            g.cameras
                .retain(|id| known.contains(id) && seen.insert(id.clone()));
        }
        self.ungrouped
            .retain(|id| known.contains(id) && seen.insert(id.clone()));
        for id in known {
            if !seen.contains(id) {
                self.ungrouped.push(id.clone());
            }
        }
        self.hd.retain(|id| known.contains(id));
        self.volume.retain(|id, _| known.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_round_trips_and_reconciles() {
        let mut l = Layout {
            groups: vec![Group {
                name: "Hall".into(),
                cameras: vec!["a".into(), "gone".into()],
                collapsed: true,
            }],
            ungrouped: vec!["b".into(), "a".into()],
            ungrouped_collapsed: false,
            hd: vec!["a".into()],
            volume: [("a".to_string(), 40u8), ("gone".to_string(), 10u8)].into(),
        };
        let text = toml::to_string_pretty(&l).unwrap();
        assert_eq!(Layout::parse(&text).unwrap(), l);
        l.reconcile(&["a".into(), "b".into(), "c".into()]);
        assert_eq!(l.groups[0].cameras, vec!["a"]);
        assert_eq!(l.ungrouped, vec!["b", "c"]);
        assert_eq!(l.volume.get("a"), Some(&40));
        assert!(!l.volume.contains_key("gone"));
        assert_eq!(Layout::parse("").unwrap(), Layout::default());
    }
}
