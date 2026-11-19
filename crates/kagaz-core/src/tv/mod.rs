//! TVs & displays: finding TVs on the network and over Wi-Fi Direct, and
//! mirroring this computer to one. Plan and findings: docs/tv-displays.md.
//!
//! A TV can answer in several ways at once (Google Cast, AirPlay, Miracast
//! over the network, the maker's remote control, Wi-Fi Direct); each TV is
//! one entry with the ways it was found in.

#[cfg(not(target_os = "ios"))]
pub mod miracast;
#[cfg(not(target_os = "ios"))]
pub mod cast;
#[cfg(not(target_os = "ios"))]
pub mod send;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod picture;
#[cfg(not(target_os = "ios"))]
pub mod switches;
#[cfg(target_os = "linux")]
pub mod sound;
#[cfg(target_os = "linux")]
pub mod keys;
#[cfg(target_os = "linux")]
pub mod capture;
#[cfg(windows)]
pub mod capture_win;
#[cfg(target_os = "macos")]
pub mod capture_mac;
#[cfg(target_os = "linux")]
pub mod p2p;
#[cfg(target_os = "linux")]
mod pw;

/// Keeps a window of Kagaz's own (the strip on top during a session) out
/// of every screen capture, the TV's included: Windows leaves it out of
/// Windows.Graphics.Capture (Windows 10 2004 and later), macOS out of
/// screen sharing. `window` is the HWND or the NSWindow.
#[cfg(windows)]
pub fn keep_off_the_tv(window: *mut std::ffi::c_void) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};
    // SAFETY: the handle is a live window of this process.
    let _ = unsafe { SetWindowDisplayAffinity(HWND(window), WDA_EXCLUDEFROMCAPTURE) };
}

#[cfg(target_os = "macos")]
pub fn keep_off_the_tv(window: *mut std::ffi::c_void) {
    use objc2::runtime::AnyObject;
    if window.is_null() {
        return;
    }
    // SAFETY: the pointer is a live NSWindow of this process; NSWindow's
    // sharingType takes NSWindowSharingNone (0).
    unsafe {
        let w = &*(window as *const AnyObject);
        let _: () = objc2::msg_send![w, setSharingType: 0usize];
    }
}

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// A way a TV answered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Way {
    /// Screen mirroring over the ordinary network (MS-MICE).
    MiracastNetwork,
    /// Screen mirroring over Wi-Fi Direct: works with a TV on no network.
    MiracastDirect,
    GoogleCast,
    AirPlay,
    /// LG's remote control (webOS).
    Webos,
    /// Android TV and Google TV's remote control.
    AndroidTvRemote,
}

impl Way {
    pub fn label(self) -> &'static str {
        match self {
            Way::MiracastNetwork => "screen mirroring over the network",
            Way::MiracastDirect => "screen mirroring over Wi-Fi Direct",
            Way::GoogleCast => "Google Cast",
            Way::AirPlay => "AirPlay",
            Way::Webos => "LG remote control",
            Way::AndroidTvRemote => "Android TV remote control",
        }
    }
}

/// A TV found nearby.
#[derive(Debug, Clone, Serialize)]
pub struct Tv {
    pub name: String,
    pub model: Option<String>,
    /// Its address on this network; `None` for a TV seen only over Wi-Fi
    /// Direct (on no network, or on another one).
    pub address: Option<IpAddr>,
    pub ways: Vec<Way>,
    /// Its Wi-Fi Direct identity, when seen that way.
    #[serde(skip)]
    #[cfg(target_os = "linux")]
    pub direct: Option<p2p::Peer>,
}

impl Tv {
    pub fn has(&self, w: Way) -> bool {
        self.ways.contains(&w)
    }

    /// Whether `query` names this TV: part of its name, or its address.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.name.to_lowercase().contains(&q)
            || self.address.is_some_and(|a| a.to_string() == query)
            || self.model.as_deref().is_some_and(|m| m.to_lowercase().contains(&q))
    }
}

const SERVICES: [(&str, Way); 4] = [
    ("_display._tcp.local.", Way::MiracastNetwork),
    ("_googlecast._tcp.local.", Way::GoogleCast),
    ("_airplay._tcp.local.", Way::AirPlay),
    ("_androidtvremote2._tcp.local.", Way::AndroidTvRemote),
];

/// webOS answers on port 3000 without announcing it.
const WEBOS_PORT: u16 = 3000;

struct Seen {
    name: String,
    model: Option<String>,
    address: IpAddr,
    way: Way,
}

fn browse(wait: Duration) -> Result<Vec<Seen>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
    let mut receivers = Vec::new();
    for (ty, way) in SERVICES {
        receivers.push((daemon.browse(ty).map_err(|e| e.to_string())?, way, ty));
    }
    let deadline = Instant::now() + wait;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        let mut any = false;
        for (rx, way, ty) in &receivers {
            while let Ok(event) = rx.recv_timeout(Duration::from_millis(30)) {
                any = true;
                let ServiceEvent::ServiceResolved(info) = event else { continue };
                let txt: BTreeMap<String, String> = info
                    .txt_properties
                    .iter()
                    .map(|p| (p.key().to_string(), p.val_str().to_string()))
                    .collect();
                let instance = info
                    .fullname
                    .strip_suffix(ty)
                    .map(|s| s.trim_end_matches('.').to_string())
                    .unwrap_or_else(|| info.fullname.clone())
                    .replace("\\032", " ");
                // Cast names its instance by model and id; the shown name is `fn`.
                let name = txt.get("fn").cloned().unwrap_or(instance);
                let model = txt.get("md").or_else(|| txt.get("model")).map(|m| {
                    // Cast adds a region code after a dot: MODEL123.ABCDEFG
                    m.split('.').next().unwrap_or(m).to_string()
                });
                for a in info.get_addresses() {
                    let ip = a.to_ip_addr();
                    if ip.is_ipv4() {
                        seen.push(Seen { name: name.clone(), model: model.clone(), address: ip, way: *way });
                    }
                }
            }
        }
        if !any {
            std::thread::sleep(Duration::from_millis(40));
        }
    }
    for (ty, _) in SERVICES {
        let _ = daemon.stop_browse(ty);
    }
    let _ = daemon.shutdown();
    Ok(seen)
}

fn answers(address: IpAddr, port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::new(address, port), Duration::from_millis(400)).is_ok()
}

/// Finds TVs on this network for `wait`, and over Wi-Fi Direct too when
/// `direct` is set (Linux).
pub fn nearby(wait: Duration, direct: bool) -> Result<Vec<Tv>, String> {
    let (seen, peers) = std::thread::scope(|s| {
        #[cfg(target_os = "linux")]
        let p2p = direct.then(|| s.spawn(move || p2p::find(wait.max(Duration::from_secs(20)), |_| false)));
        #[cfg(not(target_os = "linux"))]
        let _ = (direct, s);
        let seen = browse(wait);
        #[cfg(target_os = "linux")]
        let peers = p2p.map(|h| h.join().unwrap_or_else(|_| Err("Wi-Fi Direct search failed".into())));
        #[cfg(not(target_os = "linux"))]
        let peers: Option<Result<Vec<()>, String>> = None;
        (seen, peers)
    });
    let seen = seen?;

    let mut tvs: Vec<Tv> = Vec::new();
    for s in seen {
        let tv = match tvs.iter_mut().find(|t| t.address == Some(s.address)) {
            Some(t) => t,
            None => {
                tvs.push(Tv {
                    name: s.name.clone(),
                    model: None,
                    address: Some(s.address),
                    ways: Vec::new(),
                    #[cfg(target_os = "linux")]
                    direct: None,
                });
                tvs.last_mut().unwrap()
            }
        };
        if !tv.ways.contains(&s.way) {
            tv.ways.push(s.way);
        }
        if tv.model.is_none() {
            tv.model = s.model;
        }
    }
    for tv in &mut tvs {
        if let Some(a) = tv.address {
            if answers(a, WEBOS_PORT) {
                tv.ways.push(Way::Webos);
            }
            // The announcement of network mirroring comes and goes; the
            // port says it for sure.
            #[cfg(not(target_os = "ios"))]
            if !tv.has(Way::MiracastNetwork) && answers(a, miracast::mice::PORT) {
                tv.ways.push(Way::MiracastNetwork);
            }
        }
    }

    #[cfg(target_os = "linux")]
    if let Some(peers) = peers {
        // A TV on no network still shows here. Wi-Fi Direct failing (no
        // such Wi-Fi, NetworkManager not running) leaves the network list.
        for peer in peers.unwrap_or_default().into_iter().filter(|p| p.display) {
            match tvs.iter_mut().find(|t| t.name == peer.name) {
                Some(t) => {
                    t.ways.push(Way::MiracastDirect);
                    t.direct = Some(peer);
                }
                None => tvs.push(Tv {
                    name: peer.name.clone(),
                    model: None,
                    address: None,
                    ways: vec![Way::MiracastDirect],
                    direct: Some(peer),
                }),
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = peers;

    for tv in &mut tvs {
        tv.ways.sort();
    }
    // A Cast speaker is listed too: Cast covers both, and only the
    // device knows which it is.
    tvs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tvs)
}

/// Opens the system's own way of sending the screen to a TV, beside
/// Kagaz's: Windows' Cast panel (Win+K, Miracast, also to TVs on no
/// network), GNOME Network Displays on Linux where installed. Says what
/// was opened, or why nothing could be.
pub fn open_system_sender() -> Result<String, String> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_LWIN,
        };
        let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let k = VIRTUAL_KEY(b'K' as u16);
        let inputs = [key(VK_LWIN, false), key(k, false), key(k, true), key(VK_LWIN, true)];
        // SAFETY: plain key presses, sized as SendInput wants.
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            return Err("Windows did not take the Win+K keys: press them yourself to open Cast".into());
        }
        Ok("Windows' Cast panel is open (Win+K): pick the TV there.".into())
    }
    #[cfg(target_os = "linux")]
    {
        let tries: [(&str, &[&str]); 2] =
            [("gnome-network-displays", &[]), ("flatpak", &["run", "org.gnome.NetworkDisplays"])];
        for (program, args) in tries {
            let found = std::env::var_os("PATH")
                .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()));
            if !found {
                continue;
            }
            let ok = std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok();
            if ok && (program != "flatpak" || flatpak_has("org.gnome.NetworkDisplays")) {
                return Ok("GNOME Network Displays is open: pick the TV there.".into());
            }
        }
        Err("this desktop has no screen sender of its own (GNOME Network Displays is one to install); Kagaz's own is `kagaz tv NAME mirror`".into())
    }
    #[cfg(target_os = "macos")]
    {
        Ok("On a Mac, the system's own is AirPlay: Control Centre, Screen Mirroring, then the TV (TVs with AirPlay only).".into())
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Err("this system has no screen sender Kagaz knows of".into())
    }
}

#[cfg(target_os = "linux")]
fn flatpak_has(app: &str) -> bool {
    std::process::Command::new("flatpak")
        .args(["info", app])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
