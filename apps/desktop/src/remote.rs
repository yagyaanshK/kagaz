//! Computers & phones for the window (kagaz-core `remote`): the devices
//! the person added, kept in `<config>/remote.toml`, how each is reached
//! now, what is nearby, and add, rename, forget and each device's paths and
//! settings. The viewer (remote_view.rs) comes on top of this.
//!
//! `LIST`: every change to the list is load, change, save in one go, so a
//! look around the network (seconds) cannot undo a rename made meanwhile.

use super::blocking;
use kagaz_core::remote::tailscale::{self, Tailscale};
use kagaz_core::remote::{self, Described, Device, Devices, PathReport, PathTo, Settings, StoreError, Way};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;

static LIST: Mutex<()> = Mutex::new(());
const QUICK: Duration = Duration::from_millis(2500);

fn load() -> Result<Devices, String> {
    let _list = LIST.lock().map_err(|e| e.to_string())?;
    Devices::load(&Devices::default_path()).map_err(|e| e.to_string())
}

fn update<T>(change: impl FnOnce(&mut Devices) -> Result<T, StoreError>) -> Result<(T, Devices), String> {
    let _list = LIST.lock().map_err(|e| e.to_string())?;
    Devices::update(&Devices::default_path(), change).map_err(|e| e.to_string())
}

/// How one path does, for the page.
#[derive(Debug, Clone, Serialize)]
pub struct PathState {
    /// "Local network", "Tailscale", "Name", "Its hotspot".
    pub label: String,
    pub host: String,
    /// "Remote Desktop", "VNC on port 5901".
    pub way: String,
    pub answers: bool,
    /// Where it answered, when that differs from `host` (a name, the gateway).
    pub address: Option<String>,
    pub millis: Option<u32>,
    pub trouble: Option<String>,
}

/// How one device does now.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    pub id: String,
    pub within: bool,
    /// The path in use: its label and how long it took to open.
    pub path: Option<String>,
    pub millis: Option<u32>,
    /// Why it is out of reach, in words.
    pub why: Option<String>,
    pub paths: Vec<PathState>,
}

/// How each path of a device did, for the page.
pub(crate) fn path_states(d: &Device, reports: &[PathReport]) -> Vec<PathState> {
    reports
        .iter()
        .map(|r| {
            let p = &d.paths[r.path];
            let answer = r.outcome.as_ref().ok();
            PathState {
                label: p.kind.label().to_string(),
                host: p.host.clone(),
                way: r.way.words(),
                answers: answer.is_some(),
                address: answer.map(|a| a.addr.ip().to_string()).filter(|ip| *ip != p.host),
                millis: answer.map(|a| a.millis),
                trouble: r.outcome.as_ref().err().map(|t| t.to_string()),
            }
        })
        .collect()
}

fn state(d: &Device, reports: &[PathReport], tailscale: &Tailscale) -> State {
    let best = remote::best(d, reports);
    State {
        id: d.id.clone(),
        within: best.is_some(),
        path: best.as_ref().map(|a| d.paths[a.path].kind.label().to_string()),
        millis: best.as_ref().map(|a| a.millis),
        why: best.is_none().then(|| remote::why_not(d, reports, tailscale)),
        paths: path_states(d, reports),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Checked {
    pub devices: Devices,
    pub states: Vec<State>,
}

#[tauri::command]
pub fn remote_saved() -> Result<Devices, String> {
    load()
}

/// How every saved device does now (`only`: just that one): all their
/// paths at once, so a device that is off costs one wait, not one each.
#[tauri::command]
pub async fn remote_check(only: Option<String>) -> Result<Checked, String> {
    blocking(move || {
        let devices = load()?;
        let wanted: Vec<&Device> = devices
            .devices
            .iter()
            .filter(|d| only.as_deref().is_none_or(|id| d.id == id))
            .collect();
        let (checks, tailscale) = std::thread::scope(|s| {
            let jobs: Vec<_> = wanted.iter().map(|d| s.spawn(move || remote::check(d, QUICK))).collect();
            let tailscale = tailscale::status();
            let checks: Vec<Vec<PathReport>> = jobs.into_iter().map(|j| j.join().unwrap_or_default()).collect();
            (checks, tailscale)
        });
        let at = remote::store::now();
        let ((), saved) = update(|all| {
            for (d, reports) in wanted.iter().zip(&checks) {
                if let Some(a) = remote::best(d, reports) {
                    all.note_reached(&d.id, a.path, at);
                    all.note_identity(&d.id, &a.seen.identity);
                }
            }
            Ok(())
        })?;
        let states = wanted.iter().zip(&checks).map(|(d, r)| state(d, r, &tailscale)).collect();
        Ok(Checked { devices: saved, states })
    })
    .await
}

/// A device on this network that shows its screen.
#[derive(Debug, Clone, Serialize)]
pub struct Near {
    pub address: String,
    pub way: Way,
    pub words: String,
    /// The name in its certificate, when it showed one.
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NearPeer {
    pub name: String,
    pub address: String,
    pub os: String,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Nearby {
    /// Not saved yet.
    pub network: Vec<Near>,
    /// "on", "off" or "none" (not installed here).
    pub tailscale: &'static str,
    /// Tailscale devices not saved yet.
    pub peers: Vec<NearPeer>,
}

/// Look over this network and in Tailscale for devices not added yet.
#[tauri::command]
pub async fn remote_nearby() -> Result<Nearby, String> {
    blocking(move || {
        let (found, tailscale) = std::thread::scope(|s| {
            let search = s.spawn(|| remote::search(Duration::from_millis(400)));
            let tailscale = tailscale::status();
            (search.join().unwrap_or_default(), tailscale)
        });
        let saved = load()?;
        let known = |host: &str| saved.devices.iter().any(|d| d.paths.iter().any(|p| p.host.eq_ignore_ascii_case(host)));
        let identity_known =
            |f: &remote::Found| !f.seen.identity.is_empty() && saved.devices.iter().any(|d| d.identity.matches(&f.seen.identity));
        Ok(Nearby {
            network: found
                .iter()
                .filter(|f| !known(&f.ip.to_string()) && !identity_known(f))
                .map(|f| Near {
                    address: f.ip.to_string(),
                    way: f.way,
                    words: f.way.words(),
                    name: f.seen.identity.name.clone(),
                })
                .collect(),
            tailscale: match tailscale {
                Tailscale::NotInstalled => "none",
                Tailscale::Off => "off",
                Tailscale::On { .. } => "on",
            },
            peers: tailscale
                .peers()
                .iter()
                .filter(|p| !known(&p.address))
                .map(|p| NearPeer {
                    name: p.name.clone(),
                    address: p.address.clone(),
                    os: p.os.to_ascii_lowercase(),
                    online: p.online,
                })
                .collect(),
        })
    })
    .await
}

/// Ask a host what it speaks, before it is added.
#[tauri::command]
pub async fn remote_describe(host: String) -> Result<Described, String> {
    blocking(move || {
        if host.trim().is_empty() {
            return Err("an address or a name is needed".into());
        }
        Ok(remote::describe(&host, &[], &tailscale::status(), QUICK))
    })
    .await
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewDevice {
    pub name: String,
    /// What [`remote_describe`] found, with the ways the person kept.
    pub found: Described,
    pub user: Option<String>,
    /// A phone that also shares its hotspot.
    #[serde(default)]
    pub hotspot: bool,
    /// "computer", "laptop" or "phone".
    #[serde(default)]
    pub shape: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Added {
    pub id: String,
    pub devices: Devices,
}

#[tauri::command]
pub fn remote_add(new: NewDevice) -> Result<Added, String> {
    if new.found.ways.is_empty() {
        return Err("say what the device speaks: Remote Desktop or VNC".into());
    }
    let mut paths = vec![PathTo::new(&new.found.host)];
    if new.hotspot {
        paths.push(PathTo::gateway());
    }
    let device = Device {
        name: new.name,
        shape: new.shape,
        system: new.found.system,
        ways: new.found.ways,
        paths,
        identity: new.found.identity,
        user: new.user.filter(|u| !u.trim().is_empty()),
        // Offered ticked the first time a password is asked for.
        remember: super::remote_vault::SUPPORTED,
        ..Default::default()
    };
    let (id, devices) = update(|d| d.add(device))?;
    Ok(Added { id, devices })
}

#[tauri::command]
pub fn remote_rename(id: String, name: String) -> Result<Devices, String> {
    update(|d| d.rename(&id, &name)).map(|(_, d)| d)
}

#[tauri::command]
pub fn remote_forget(id: String) -> Result<Devices, String> {
    let (gone, devices) = update(|d| d.forget(&id))?;
    // Its password goes with it; a store that cannot be reached now does
    // not keep the device.
    let _ = super::remote_vault::forget(&gone.id);
    Ok(devices)
}

/// Can this build keep a password in the system's password store?
#[tauri::command]
pub fn remote_can_remember() -> bool {
    super::remote_vault::SUPPORTED
}

/// Remember this device's password from now on, or stop and forget the
/// one that is kept.
#[tauri::command]
pub async fn remote_remember(id: String, on: bool) -> Result<Devices, String> {
    blocking(move || {
        let (id, devices) = update(|d| {
            let id = d.find(&id)?.id.clone();
            if let Some(dev) = d.devices.iter_mut().find(|x| x.id == id) {
                dev.remember = on;
            }
            Ok(id)
        })?;
        if !on {
            super::remote_vault::forget(&id)?;
        }
        Ok(devices)
    })
    .await
}

/// One more way to reach a device; `host` empty for "its hotspot".
#[tauri::command]
pub fn remote_path_add(id: String, host: String) -> Result<Devices, String> {
    let path = if host.trim().is_empty() { PathTo::gateway() } else { PathTo::new(&host) };
    update(|d| d.add_path(&id, path)).map(|(_, d)| d)
}

#[tauri::command]
pub fn remote_path_remove(id: String, host: String) -> Result<Devices, String> {
    update(|d| d.remove_path(&id, &host)).map(|(_, d)| d)
}

/// The device's screen, mouse and sound settings, its shape and its account.
#[tauri::command]
pub fn remote_settings(id: String, settings: Settings, user: Option<String>, shape: Option<String>) -> Result<Devices, String> {
    update(|d| {
        let id = d.find(&id)?.id.clone();
        if let Some(dev) = d.devices.iter_mut().find(|x| x.id == id) {
            dev.settings = settings;
            dev.user = user.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
            if let Some(shape) = shape.filter(|s| ["computer", "laptop", "phone"].contains(&s.as_str())) {
                dev.shape = shape;
            }
        }
        Ok(())
    })
    .map(|(_, d)| d)
}
