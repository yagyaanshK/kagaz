//! TVs & displays: finding TVs on the network and over Wi-Fi Direct, and
//! mirroring this computer to one. Plan and findings: docs/tv-displays.md.
//!
//! A TV can answer in several ways at once (Google Cast, AirPlay, Miracast
//! over the network, the maker's remote control, Wi-Fi Direct); each TV is
//! one entry with the ways it was found in.

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod miracast;
#[cfg(target_os = "linux")]
pub mod p2p;

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
