//! The fans the person added, by room, with how to reach each and how it
//! was last seen, in `<config>/fans.toml`. A remembered fan is listed even
//! when it is out of reach, as a remembered printer is.

use super::atomberg::FanState;
use super::Found;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Fan {
    /// The fan's own id: its Wi-Fi hardware address in lowercase hex.
    pub id: String,
    /// The name the person gave it ("Bedroom fan").
    pub name: String,
    /// The room it is in; empty for none.
    #[serde(default)]
    pub room: String,
    /// The series code ("R1"), which says what the fan can do.
    #[serde(default)]
    pub series: String,
    /// Its Bluetooth address, when seen that way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ble_address: Option<String>,
    /// Its address on the network, when seen that way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// How it was when last reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_state: Option<FanState>,
    /// When it was last reached (seconds since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
    /// "bluetooth" or "wifi": how it was last reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reach: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Fans {
    /// Rooms in the order they are shown.
    #[serde(default)]
    pub rooms: Vec<String>,
    #[serde(default)]
    pub fans: Vec<Fan>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot read or write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not a valid fan list: {0}", path = .1.display())]
    Parse(String, PathBuf),
    #[error("no fan called \"{0}\"")]
    NoSuchFan(String),
    #[error("a fan's name cannot be empty")]
    EmptyName,
    #[error("there is already a room called \"{0}\"")]
    RoomExists(String),
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Fans {
    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("fans.toml")
    }

    pub fn load(path: &Path) -> Result<Fans, StoreError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| StoreError::Parse(e.to_string(), path.to_path_buf())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Fans::default()),
            Err(source) => Err(StoreError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        let io = |source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| StoreError::Parse(e.to_string(), path.to_path_buf()))?;
        std::fs::write(path, text).map_err(io)
    }

    /// Load the list from `path`, change it and save it: the change lands
    /// on the list as it is on disk now, not on a copy read earlier. A
    /// long job (a scan, a command over Bluetooth) must work on a copy and
    /// note its findings through here at the end, or it undoes the renames
    /// and moves made while it ran.
    pub fn update<T>(
        path: &Path,
        change: impl FnOnce(&mut Fans) -> Result<T, StoreError>,
    ) -> Result<(T, Fans), StoreError> {
        let mut fans = Fans::load(path)?;
        let out = change(&mut fans)?;
        fans.save(path)?;
        Ok((out, fans))
    }

    /// The fan called `which`: its name (any case), its id, or the end of its id.
    pub fn find(&self, which: &str) -> Option<&Fan> {
        let w = which.trim().to_ascii_lowercase();
        self.fans
            .iter()
            .find(|f| f.name.to_ascii_lowercase() == w)
            .or_else(|| self.fans.iter().find(|f| f.id == w))
            .or_else(|| {
                (w.len() >= 4)
                    .then(|| self.fans.iter().find(|f| f.id.ends_with(&w)))
                    .flatten()
            })
    }

    fn find_mut(&mut self, which: &str) -> Result<&mut Fan, StoreError> {
        let id = self
            .find(which)
            .map(|f| f.id.clone())
            .ok_or_else(|| StoreError::NoSuchFan(which.to_string()))?;
        Ok(self
            .fans
            .iter_mut()
            .find(|f| f.id == id)
            .expect("just found"))
    }

    pub fn is_remembered(&self, id: &str) -> bool {
        self.fans.iter().any(|f| f.id == id)
    }

    /// Remember a found fan under `name` in `room` (the room is created
    /// when new). A fan already remembered keeps its place and takes the
    /// new name and room.
    pub fn add(&mut self, found: &Found, name: &str, room: &str) -> Result<&Fan, StoreError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::EmptyName);
        }
        let room = room.trim().to_string();
        if !room.is_empty() && !self.rooms.iter().any(|r| r == &room) {
            self.rooms.push(room.clone());
        }
        let seen = now();
        match self.fans.iter_mut().find(|f| f.id == found.id) {
            Some(f) => {
                f.name = name.to_string();
                f.room = room;
            }
            None => self.fans.push(Fan {
                id: found.id.clone(),
                name: name.to_string(),
                room,
                series: found.series.clone(),
                ble_address: None,
                ip: None,
                last_state: None,
                last_seen: None,
                last_reach: None,
            }),
        }
        let f = self
            .fans
            .iter_mut()
            .find(|f| f.id == found.id)
            .expect("just put");
        f.note(found, seen);
        Ok(f)
    }

    pub fn forget(&mut self, which: &str) -> Result<Fan, StoreError> {
        let id = self
            .find(which)
            .map(|f| f.id.clone())
            .ok_or_else(|| StoreError::NoSuchFan(which.to_string()))?;
        let i = self
            .fans
            .iter()
            .position(|f| f.id == id)
            .expect("just found");
        Ok(self.fans.remove(i))
    }

    pub fn rename(&mut self, which: &str, name: &str) -> Result<(), StoreError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::EmptyName);
        }
        self.find_mut(which)?.name = name.to_string();
        Ok(())
    }

    /// Put the fan in `room` (empty: no room); a new room is created.
    pub fn set_room(&mut self, which: &str, room: &str) -> Result<(), StoreError> {
        let room = room.trim().to_string();
        if !room.is_empty() && !self.rooms.iter().any(|r| r == &room) {
            self.rooms.push(room.clone());
        }
        self.find_mut(which)?.room = room;
        Ok(())
    }

    pub fn room_add(&mut self, room: &str) -> Result<(), StoreError> {
        let room = room.trim();
        if room.is_empty() {
            return Err(StoreError::EmptyName);
        }
        if self.rooms.iter().any(|r| r.eq_ignore_ascii_case(room)) {
            return Err(StoreError::RoomExists(room.to_string()));
        }
        self.rooms.push(room.to_string());
        Ok(())
    }

    pub fn room_rename(&mut self, old: &str, new: &str) -> Result<(), StoreError> {
        let new = new.trim();
        if new.is_empty() {
            return Err(StoreError::EmptyName);
        }
        if old != new && self.rooms.iter().any(|r| r.eq_ignore_ascii_case(new)) {
            return Err(StoreError::RoomExists(new.to_string()));
        }
        for r in self.rooms.iter_mut().filter(|r| r.as_str() == old) {
            *r = new.to_string();
        }
        for f in self.fans.iter_mut().filter(|f| f.room == old) {
            f.room = new.to_string();
        }
        Ok(())
    }

    /// Remove the room; its fans stay, with no room.
    pub fn room_remove(&mut self, room: &str) {
        self.rooms.retain(|r| r != room);
        for f in self.fans.iter_mut().filter(|f| f.room == room) {
            f.room.clear();
        }
    }

    /// Reorder the rooms (names not listed keep their place at the end).
    pub fn room_order(&mut self, order: &[String]) {
        let mut rooms: Vec<String> = order
            .iter()
            .filter(|r| self.rooms.contains(r))
            .cloned()
            .collect();
        for r in &self.rooms {
            if !rooms.contains(r) {
                rooms.push(r.clone());
            }
        }
        self.rooms = rooms;
    }

    /// A scan saw these fans: the remembered ones take their fresh
    /// addresses and states. True when anything changed.
    pub fn note_seen(&mut self, found: &[Found]) -> bool {
        let seen = now();
        let mut changed = false;
        for f in self.fans.iter_mut() {
            if let Some(x) = found.iter().find(|x| x.id == f.id) {
                let before = f.clone();
                f.note(x, seen);
                changed |= before != *f;
            }
        }
        changed
    }

    /// The fan answered with a state, by `reach` ("bluetooth" or "wifi").
    pub fn note_state(&mut self, id: &str, state: &FanState, reach: &str) {
        if let Some(f) = self.fans.iter_mut().find(|f| f.id == id) {
            f.last_state = Some(state.clone());
            f.last_seen = Some(now());
            f.last_reach = Some(reach.to_string());
            if let Some(s) = &state.series {
                f.series = s.clone();
            }
        }
    }

    /// The fans in `room` ("" for none), in order of name.
    pub fn in_room(&self, room: &str) -> Vec<&Fan> {
        let mut v: Vec<&Fan> = self.fans.iter().filter(|f| f.room == room).collect();
        v.sort_by_key(|f| f.name.to_ascii_lowercase());
        v
    }
}

impl Fan {
    fn note(&mut self, found: &Found, seen: u64) {
        if !found.series.is_empty() {
            self.series = found.series.clone();
        }
        if let Some(a) = &found.ble_address {
            self.ble_address = Some(a.clone());
        }
        if let Some(ip) = &found.ip {
            self.ip = Some(ip.clone());
        }
        if let Some(s) = &found.state {
            self.last_state = Some(s.clone());
            self.last_seen = Some(seen);
            self.last_reach = Some(
                if found.ip.is_some() {
                    "wifi"
                } else {
                    "bluetooth"
                }
                .to_string(),
            );
        }
    }

    /// What the fan can do, from its series.
    pub fn profile(&self) -> super::atomberg::Profile {
        super::atomberg::profile(&self.series)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(id: &str, series: &str) -> Found {
        Found {
            id: id.into(),
            series: series.into(),
            ble_address: Some("AA:BB:CC:DD:EE:02".into()),
            ip: None,
            rssi: Some(-60),
            state: FanState::decode("19,0,0,0,0,0,0,R1"),
        }
    }

    #[test]
    fn add_rename_move_forget_round_trip() {
        let dir = std::env::temp_dir().join(format!("kagaz-fans-{}", std::process::id()));
        let path = dir.join("fans.toml");
        let mut fans = Fans::load(&path).unwrap();
        fans.add(&found("a1b2c3d4e5f6", "R1"), "Bedroom fan", "Bedroom")
            .unwrap();
        fans.add(&found("0011223344ff", "M1"), "Hall fan", "")
            .unwrap();
        assert_eq!(fans.rooms, vec!["Bedroom"]);
        assert_eq!(fans.find("bedroom FAN").unwrap().id, "a1b2c3d4e5f6");
        assert_eq!(fans.find("e5f6").unwrap().name, "Bedroom fan");
        assert!(fans.find("zzzz").is_none());
        fans.rename("Hall fan", "Living room fan").unwrap();
        fans.set_room("Living room fan", "Living room").unwrap();
        fans.room_rename("Bedroom", "Main bedroom").unwrap();
        assert_eq!(fans.find("bedroom fan").unwrap().room, "Main bedroom");
        fans.save(&path).unwrap();
        let back = Fans::load(&path).unwrap();
        assert_eq!(back, fans);
        assert_eq!(back.in_room("Living room")[0].name, "Living room fan");
        assert_eq!(back.fans[0].last_state.as_ref().unwrap().speed, 3);
        let mut back = back;
        back.room_remove("Living room");
        assert_eq!(back.find("living room fan").unwrap().room, "");
        back.forget("44ff").unwrap();
        assert_eq!(back.fans.len(), 1);
        assert!(matches!(back.forget("44ff"), Err(StoreError::NoSuchFan(_))));
        assert!(matches!(
            back.room_add("main BEDROOM"),
            Err(StoreError::RoomExists(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_long_scan_does_not_undo_a_rename_made_while_it_ran() {
        let dir = std::env::temp_dir().join(format!("kagaz-fans-race-{}", std::process::id()));
        let path = dir.join("fans.toml");
        let _ = std::fs::remove_dir_all(&dir);
        Fans::update(&path, |f| {
            f.add(&found("a1b2c3d4e5f6", "R1"), "Renesa e5f6", "")
                .map(|_| ())
        })
        .unwrap();
        // A scan starts: it works on a copy while it asks the fans.
        let copy = Fans::load(&path).unwrap();
        let seen = vec![found("a1b2c3d4e5f6", "R1")];
        // Meanwhile the person renames the fan and puts it in a room.
        Fans::update(&path, |f| f.rename("e5f6", "Bedroom fan")).unwrap();
        Fans::update(&path, |f| f.set_room("e5f6", "Bedroom")).unwrap();
        // The scan ends and notes what it learnt.
        let state = FanState::decode("0,0,0,0,0,0,0,R1").unwrap();
        let ((), after) = Fans::update(&path, |f| {
            f.note_seen(&seen);
            f.note_state("a1b2c3d4e5f6", &state, "bluetooth");
            Ok(())
        })
        .unwrap();
        assert_eq!(after.fans[0].name, "Bedroom fan");
        assert_eq!(after.fans[0].room, "Bedroom");
        assert!(!after.fans[0].last_state.as_ref().unwrap().power);
        assert_eq!(
            copy.fans[0].name, "Renesa e5f6",
            "the copy was taken before the rename"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_scan_refreshes_what_it_saw() {
        let mut fans = Fans::default();
        fans.add(&found("a1b2c3d4e5f6", "R1"), "Bedroom fan", "")
            .unwrap();
        let mut again = found("a1b2c3d4e5f6", "R1");
        again.ble_address = Some("AA:BB:CC:DD:EE:03".into());
        again.state = FanState::decode("0,0,0,0,0,0,0,R1");
        assert!(fans.note_seen(&[again.clone(), found("other0000000", "M1")]));
        assert_eq!(
            fans.fans[0].ble_address.as_deref(),
            Some("AA:BB:CC:DD:EE:03")
        );
        assert!(!fans.fans[0].last_state.as_ref().unwrap().power);
        assert_eq!(fans.fans.len(), 1, "a scan adds nothing by itself");
    }
}
