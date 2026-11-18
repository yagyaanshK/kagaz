//! The computers and phones the person added, with every way to reach
//! each, in `<config>/remote.toml`. A remembered device is listed even when
//! it is out of reach, as a remembered printer is. Passwords are never
//! here: they go in the system's password store, when the person says so.

use super::{Speaks, Way};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where a path leads through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    /// An address on a local network (it changes when the router says so).
    Address,
    /// A name the network resolves ("study-laptop.local").
    Name,
    /// The device's address in the person's Tailscale network.
    Tailscale,
    /// Whatever gateway this device has now: a phone sharing its hotspot
    /// is the gateway of everything joined to it, at a random address.
    Gateway,
}

impl PathKind {
    pub fn label(self) -> &'static str {
        match self {
            PathKind::Address => "Local network",
            PathKind::Name => "Name",
            PathKind::Tailscale => "Tailscale",
            PathKind::Gateway => "Its hotspot",
        }
    }

    /// The kind a typed host is: Tailscale's addresses are 100.64.0.0/10
    /// (and its names end in .ts.net), any other address is local.
    pub fn of(host: &str) -> PathKind {
        match host.parse::<IpAddr>() {
            Ok(IpAddr::V4(v4)) if v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 => PathKind::Tailscale,
            Ok(IpAddr::V6(v6)) if v6.segments()[0] == 0xfd7a && v6.segments()[1] == 0x115c => PathKind::Tailscale,
            Ok(_) => PathKind::Address,
            Err(_) if host.to_ascii_lowercase().trim_end_matches('.').ends_with(".ts.net") => PathKind::Tailscale,
            Err(_) => PathKind::Name,
        }
    }
}

/// One way to reach a device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathTo {
    pub kind: PathKind,
    /// The address or name; empty for [`PathKind::Gateway`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// When it last answered (seconds since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok: Option<u64>,
}

impl PathTo {
    pub fn new(host: &str) -> PathTo {
        let host = host.trim();
        PathTo {
            kind: PathKind::of(host),
            host: host.to_string(),
            last_ok: None,
        }
    }

    pub fn gateway() -> PathTo {
        PathTo {
            kind: PathKind::Gateway,
            host: String::new(),
            last_ok: None,
        }
    }

    /// "Local network · 192.0.2.20", "Its hotspot".
    pub fn words(&self) -> String {
        if self.host.is_empty() {
            self.kind.label().to_string()
        } else {
            format!("{} · {}", self.kind.label(), self.host)
        }
    }
}

/// What tells a device from another at the same address: a Remote Desktop
/// computer shows a certificate before any sign-in. Windows renews that
/// certificate every few months but keeps the name in it, so the name finds
/// the device again and the fingerprint notices a change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The name in the certificate (the computer's own name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// SHA-256 of the certificate, in hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
}

impl Identity {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.certificate.is_none()
    }

    /// Is `seen` this device? The same certificate, or the same name.
    pub fn matches(&self, seen: &Identity) -> bool {
        match (&self.certificate, &seen.certificate) {
            (Some(a), Some(b)) if a == b => return true,
            _ => {}
        }
        match (&self.name, &seen.name) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => false,
        }
    }
}

/// Where the keys go that this computer normally keeps for itself (Print
/// Screen, the Windows key, Alt + Tab, Ctrl + Alt + Del) while the
/// device's screen is being used. Each press goes to one side only, never
/// both: a screenshot of one screen, a menu on one computer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemKeys {
    /// To the device in full screen (nothing of this computer shows
    /// then); to this computer in a window.
    #[default]
    FullScreen,
    /// To the device whenever Kagaz is in front with the pointer on it.
    Window,
    /// To this computer always; the viewer's key menu sends them.
    Never,
}

impl SystemKeys {
    fn is_default(&self) -> bool {
        *self == SystemKeys::default()
    }
}

/// How a device's screen is shown and driven; the defaults are what the
/// person gets without touching anything.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Keep the remote's own screen size and scale the picture, instead of
    /// asking the device to redraw at the viewer's size.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub own_size: bool,
    /// One monitor (counted from 1); none for all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<u32>,
    /// 0 (sharpest) to 10 (smoothest); none lets Kagaz follow the link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<u8>,
    /// The device has its mouse buttons swapped: send the other button, so
    /// a left click here is a left click there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub swapped_buttons: bool,
    /// Do not play the device's sound here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mute: bool,
    /// Remote Desktop's older picture (changed areas as plain bitmaps)
    /// instead of the newer one (H.264 and its companions, each frame
    /// acknowledged): for a computer whose picture comes out wrong.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub older_picture: bool,
    /// Where this computer's own keys go during a session.
    #[serde(default, skip_serializing_if = "SystemKeys::is_default")]
    pub system_keys: SystemKeys,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Kagaz's own id for it (random; the device knows nothing of it).
    pub id: String,
    /// The name the person gave it ("Study laptop").
    pub name: String,
    /// "computer", "laptop" or "phone": which picture it gets.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shape: String,
    /// What it runs, when known ("windows", "linux", "android", "macos").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub system: String,
    #[serde(default)]
    pub ways: Vec<Way>,
    #[serde(default)]
    pub paths: Vec<PathTo>,
    #[serde(default, skip_serializing_if = "Identity::is_empty")]
    pub identity: Identity,
    /// The account to sign in with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Keep the password in the system's password store once one is given
    /// (the store is the window's affair; nothing of it is in this list).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remember: bool,
    #[serde(default, skip_serializing_if = "settings_default")]
    pub settings: Settings,
    /// When it last answered, and over which path ("Tailscale").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_path: Option<String>,
}

fn settings_default(s: &Settings) -> bool {
    *s == Settings::default()
}

impl Device {
    /// The way in that shows its screen: Remote Desktop before VNC.
    pub fn screen_way(&self) -> Option<Way> {
        [Speaks::Rdp, Speaks::Vnc]
            .iter()
            .find_map(|s| self.ways.iter().find(|w| w.speaks == *s).copied())
    }

    pub fn way(&self, speaks: Speaks) -> Option<Way> {
        self.ways.iter().find(|w| w.speaks == speaks).copied()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Devices {
    #[serde(default)]
    pub devices: Vec<Device>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot read or write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not a valid device list: {0}", path = .1.display())]
    Parse(String, PathBuf),
    #[error("no computer or phone called \"{0}\"")]
    NoSuchDevice(String),
    #[error("\"{0}\" fits more than one device: {1}")]
    Ambiguous(String, String),
    #[error("a device's name cannot be empty")]
    EmptyName,
    #[error("an address or a name is needed to reach the device")]
    NoPath,
    #[error("that is the device's only way to reach it; add another first, or forget the device")]
    LastPath,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Devices {
    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("remote.toml")
    }

    pub fn load(path: &Path) -> Result<Devices, StoreError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| StoreError::Parse(e.to_string(), path.to_path_buf())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Devices::default()),
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
        let text = toml::to_string_pretty(self).map_err(|e| StoreError::Parse(e.to_string(), path.to_path_buf()))?;
        std::fs::write(path, text).map_err(io)
    }

    /// Load, change, save in one go, on the list as it is on disk now, so
    /// a long job (a look around the network) cannot undo a rename made
    /// meanwhile. Returns what `change` returned and the list as saved.
    pub fn update<T>(
        path: &Path,
        change: impl FnOnce(&mut Devices) -> Result<T, StoreError>,
    ) -> Result<(T, Devices), StoreError> {
        let mut devices = Devices::load(path)?;
        let out = change(&mut devices)?;
        devices.save(path)?;
        Ok((out, devices))
    }

    /// The device `which` means: its id, its exact name, or the start of
    /// its name, whichever fits one device (names without regard to case).
    pub fn find(&self, which: &str) -> Result<&Device, StoreError> {
        let want = which.trim().to_lowercase();
        if let Some(d) = self.devices.iter().find(|d| d.id == which) {
            return Ok(d);
        }
        if let Some(d) = self.devices.iter().find(|d| d.name.to_lowercase() == want) {
            return Ok(d);
        }
        let fits: Vec<&Device> = self
            .devices
            .iter()
            .filter(|d| !want.is_empty() && d.name.to_lowercase().starts_with(&want))
            .collect();
        match fits.as_slice() {
            [one] => Ok(one),
            [] => Err(StoreError::NoSuchDevice(which.to_string())),
            many => Err(StoreError::Ambiguous(
                which.to_string(),
                many.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", "),
            )),
        }
    }

    fn find_mut(&mut self, which: &str) -> Result<&mut Device, StoreError> {
        let id = self.find(which)?.id.clone();
        Ok(self.devices.iter_mut().find(|d| d.id == id).expect("just found"))
    }

    /// Remember a device; returns its id. A name already taken gets a
    /// number after it, so two can always be told apart.
    pub fn add(&mut self, mut device: Device) -> Result<String, StoreError> {
        let name = device.name.trim().to_string();
        if name.is_empty() {
            return Err(StoreError::EmptyName);
        }
        if device.paths.is_empty() {
            return Err(StoreError::NoPath);
        }
        device.name = self.free_name(&name, None);
        if device.id.is_empty() {
            device.id = uuid::Uuid::new_v4().simple().to_string();
        }
        if device.shape.is_empty() {
            device.shape = if device.system == "android" || device.system == "ios" {
                "phone"
            } else {
                "computer"
            }
            .to_string();
        }
        let id = device.id.clone();
        self.devices.push(device);
        Ok(id)
    }

    fn free_name(&self, name: &str, except: Option<&str>) -> String {
        let taken = |n: &str| {
            self.devices
                .iter()
                .any(|d| Some(d.id.as_str()) != except && d.name.eq_ignore_ascii_case(n))
        };
        if !taken(name) {
            return name.to_string();
        }
        (2..)
            .map(|i| format!("{name} {i}"))
            .find(|n| !taken(n))
            .expect("some number is free")
    }

    pub fn forget(&mut self, which: &str) -> Result<Device, StoreError> {
        let id = self.find(which)?.id.clone();
        let at = self.devices.iter().position(|d| d.id == id).expect("just found");
        Ok(self.devices.remove(at))
    }

    pub fn rename(&mut self, which: &str, name: &str) -> Result<(), StoreError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::EmptyName);
        }
        let id = self.find(which)?.id.clone();
        let name = self.free_name(name, Some(&id));
        self.find_mut(&id)?.name = name;
        Ok(())
    }

    /// One more way to reach a device (nothing happens when it is already
    /// there).
    pub fn add_path(&mut self, which: &str, path: PathTo) -> Result<(), StoreError> {
        if path.kind != PathKind::Gateway && path.host.is_empty() {
            return Err(StoreError::NoPath);
        }
        let d = self.find_mut(which)?;
        if !d.paths.iter().any(|p| p.kind == path.kind && p.host.eq_ignore_ascii_case(&path.host)) {
            d.paths.push(path);
        }
        Ok(())
    }

    pub fn remove_path(&mut self, which: &str, host: &str) -> Result<(), StoreError> {
        let d = self.find_mut(which)?;
        if d.paths.len() <= 1 {
            return Err(StoreError::LastPath);
        }
        let gateway = host.is_empty() || host.eq_ignore_ascii_case("hotspot");
        d.paths
            .retain(|p| !(p.host.eq_ignore_ascii_case(host) || (gateway && p.kind == PathKind::Gateway)));
        Ok(())
    }

    /// A path answered: when, and through what.
    pub fn note_reached(&mut self, id: &str, path: usize, at: u64) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.id == id) {
            d.last_seen = Some(at);
            if let Some(p) = d.paths.get_mut(path) {
                p.last_ok = Some(at);
                d.last_path = Some(p.kind.label().to_string());
            }
        }
    }

    /// The device was found again at a new local address: that address
    /// takes the place of the local addresses that no longer answer.
    pub fn note_moved(&mut self, id: &str, host: &str, at: u64) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.id == id) {
            d.paths.retain(|p| p.kind != PathKind::Address);
            d.paths.insert(
                0,
                PathTo {
                    kind: PathKind::Address,
                    host: host.to_string(),
                    last_ok: Some(at),
                },
            );
            d.last_seen = Some(at);
            d.last_path = Some(PathKind::Address.label().to_string());
        }
    }

    /// What the device showed of itself; `true` when it had another
    /// certificate before (worth telling the person).
    pub fn note_identity(&mut self, id: &str, seen: &Identity) -> bool {
        let Some(d) = self.devices.iter_mut().find(|d| d.id == id) else {
            return false;
        };
        if seen.is_empty() {
            return false;
        }
        let changed = matches!((&d.identity.certificate, &seen.certificate), (Some(a), Some(b)) if a != b);
        // Another certificate than the one remembered is not taken on
        // here: the person is asked when they connect (`trust_identity`).
        if !changed {
            d.identity = seen.clone();
        }
        changed
    }

    /// The person accepted the certificate the device shows now.
    pub fn trust_identity(&mut self, id: &str, seen: &Identity) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.id == id) {
            if !seen.is_empty() {
                d.identity = seen.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laptop() -> Device {
        Device {
            name: "Study laptop".into(),
            system: "windows".into(),
            ways: vec![Way::new(Speaks::Rdp)],
            paths: vec![PathTo::new("192.0.2.20"), PathTo::new("100.64.0.7")],
            ..Default::default()
        }
    }

    #[test]
    fn a_typed_host_knows_its_kind() {
        assert_eq!(PathKind::of("192.0.2.20"), PathKind::Address);
        assert_eq!(PathKind::of("100.64.0.7"), PathKind::Tailscale);
        assert_eq!(PathKind::of("100.127.255.254"), PathKind::Tailscale);
        assert_eq!(PathKind::of("100.128.0.1"), PathKind::Address);
        assert_eq!(PathKind::of("study-laptop.local"), PathKind::Name);
        assert_eq!(PathKind::of("study-laptop.example.ts.net"), PathKind::Tailscale);
        assert_eq!(PathKind::of("fd7a:115c:a1e0::7"), PathKind::Tailscale);
    }

    #[test]
    fn devices_are_found_by_id_name_or_its_start() {
        let mut all = Devices::default();
        let id = all.add(laptop()).unwrap();
        all.add(Device {
            name: "Study phone".into(),
            system: "android".into(),
            paths: vec![PathTo::gateway()],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(all.find(&id).unwrap().name, "Study laptop");
        assert_eq!(all.find("study LAPTOP").unwrap().id, id);
        assert_eq!(all.find("study l").unwrap().id, id);
        assert!(matches!(all.find("study"), Err(StoreError::Ambiguous(..))));
        assert!(matches!(all.find("kitchen"), Err(StoreError::NoSuchDevice(_))));
        assert_eq!(all.find("study phone").unwrap().shape, "phone");
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let mut all = Devices::default();
        all.add(laptop()).unwrap();
        let second = all.add(laptop()).unwrap();
        assert_eq!(all.find(&second).unwrap().name, "Study laptop 2");
        // Renaming a device to its own name leaves it alone.
        all.rename(&second, "study laptop 2").unwrap();
        assert_eq!(all.find(&second).unwrap().name, "study laptop 2");
    }

    #[test]
    fn a_device_needs_a_name_and_a_path() {
        let mut all = Devices::default();
        assert!(matches!(
            all.add(Device { name: " ".into(), ..laptop() }),
            Err(StoreError::EmptyName)
        ));
        assert!(matches!(
            all.add(Device { paths: vec![], ..laptop() }),
            Err(StoreError::NoPath)
        ));
    }

    #[test]
    fn paths_come_and_go_but_never_all() {
        let mut all = Devices::default();
        let id = all.add(laptop()).unwrap();
        all.add_path(&id, PathTo::new("study-laptop.local")).unwrap();
        all.add_path(&id, PathTo::new("STUDY-LAPTOP.local")).unwrap();
        assert_eq!(all.find(&id).unwrap().paths.len(), 3);
        all.remove_path(&id, "192.0.2.20").unwrap();
        all.remove_path(&id, "100.64.0.7").unwrap();
        assert!(matches!(all.remove_path(&id, "study-laptop.local"), Err(StoreError::LastPath)));
    }

    #[test]
    fn a_new_local_address_replaces_the_old_ones() {
        let mut all = Devices::default();
        let id = all.add(laptop()).unwrap();
        all.note_moved(&id, "192.0.2.57", 1000);
        let d = all.find(&id).unwrap();
        let hosts: Vec<&str> = d.paths.iter().map(|p| p.host.as_str()).collect();
        assert_eq!(hosts, ["192.0.2.57", "100.64.0.7"]);
        assert_eq!(d.last_path.as_deref(), Some("Local network"));
    }

    #[test]
    fn identity_matches_by_certificate_or_name_and_notices_a_change() {
        let first = Identity {
            name: Some("STUDY-LAPTOP".into()),
            certificate: Some("aa".into()),
        };
        let renewed = Identity {
            name: Some("study-laptop".into()),
            certificate: Some("bb".into()),
        };
        let other = Identity {
            name: Some("OFFICE".into()),
            certificate: Some("cc".into()),
        };
        assert!(first.matches(&renewed));
        assert!(!first.matches(&other));
        let mut all = Devices::default();
        let id = all.add(laptop()).unwrap();
        assert!(!all.note_identity(&id, &first));
        // Another certificate is noticed every time and not taken on...
        assert!(all.note_identity(&id, &renewed));
        assert!(all.note_identity(&id, &renewed));
        assert_eq!(all.devices[0].identity, first);
        // ...until the person says so.
        all.trust_identity(&id, &renewed);
        assert_eq!(all.devices[0].identity, renewed);
        assert!(!all.note_identity(&id, &renewed));
    }

    #[test]
    fn the_list_survives_a_round_trip_and_stays_short() {
        let dir = std::env::temp_dir().join(format!("kagaz-remote-{}", uuid::Uuid::new_v4().simple()));
        let path = dir.join("remote.toml");
        let ((), saved) = Devices::update(&path, |d| d.add(laptop()).map(|_| ())).unwrap();
        assert_eq!(Devices::load(&path).unwrap(), saved);
        let text = std::fs::read_to_string(&path).unwrap();
        // Defaults are not written out.
        assert!(!text.contains("settings"), "{text}");
        assert!(!text.contains("remember"), "{text}");
        // A choice that is not the default is, in words the page shares.
        let (_, saved) = Devices::update(&path, |d| {
            d.devices[0].settings.system_keys = SystemKeys::Window;
            Ok(())
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("system_keys = \"window\""), "{text}");
        assert_eq!(Devices::load(&path).unwrap(), saved);
        let from_page: Settings = serde_json::from_str(r#"{"system_keys":"full-screen"}"#).unwrap();
        assert_eq!(from_page, Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
