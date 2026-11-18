//! Computers and phones: view and control your own devices from each
//! other. This first part is about reaching them, which is the fragile bit:
//! a device is remembered with every way to it (its addresses on the local
//! network, its name, its Tailscale address, "the gateway" when it is a
//! phone sharing its hotspot), all of them are tried at once, and the first
//! to answer is used. A session (`session`; over `vnc` or `rdp`) then
//! brings the device's screen into a shared `screen` and takes the mouse
//! and keyboard there.
//!
//! Plan and limits: `docs/remote-connection.md`.

pub mod keys;
pub mod link;
pub mod pair;
pub mod pattern;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod rdp;
pub mod reach;
pub mod screen;
pub mod session;
pub mod store;
pub mod tailscale;
pub mod vnc;

pub use reach::{check, detect, reach, search, Answer, Found, PathReport, Seen, Trouble};
pub use screen::{Event, Piece, Pointer, Rect, Screen, Sending, Taken};
pub use session::{Info, Input, InputSender, OpenError, Reading, Session, SignIn, Trust, View};
pub use store::{Device, Devices, Identity, PathKind, PathTo, Settings, StoreError};

use serde::{Deserialize, Serialize};

/// What a device speaks on one of its ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaks {
    /// Windows' Remote Desktop, also GNOME's.
    Rdp,
    Vnc,
    Ssh,
}

impl Speaks {
    pub const ALL: [Speaks; 3] = [Speaks::Rdp, Speaks::Vnc, Speaks::Ssh];

    pub fn default_port(self) -> u16 {
        match self {
            Speaks::Rdp => 3389,
            Speaks::Vnc => 5900,
            Speaks::Ssh => 22,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Speaks::Rdp => "Remote Desktop",
            Speaks::Vnc => "VNC",
            Speaks::Ssh => "SSH",
        }
    }

    /// Does it carry the device's screen (as opposed to a terminal)?
    pub fn shows_screen(self) -> bool {
        !matches!(self, Speaks::Ssh)
    }

    pub fn parse(word: &str) -> Option<Speaks> {
        match word.to_ascii_lowercase().as_str() {
            "rdp" => Some(Speaks::Rdp),
            "vnc" => Some(Speaks::Vnc),
            "ssh" => Some(Speaks::Ssh),
            _ => None,
        }
    }
}

/// One way into a device: what it speaks, and on which port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Way {
    pub speaks: Speaks,
    pub port: u16,
}

impl Way {
    pub fn new(speaks: Speaks) -> Way {
        Way {
            speaks,
            port: speaks.default_port(),
        }
    }

    /// "Remote Desktop", or "VNC on port 5901" off the usual port.
    pub fn words(&self) -> String {
        if self.port == self.speaks.default_port() {
            self.speaks.label().to_string()
        } else {
            format!("{} on port {}", self.speaks.label(), self.port)
        }
    }
}

use std::path::Path;
use std::time::Duration;

/// The device could not be reached: how each path did, and why in words.
#[derive(Debug, Clone)]
pub struct NotReached {
    pub reports: Vec<PathReport>,
    pub why: String,
}

impl std::fmt::Display for NotReached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

/// Reach a saved device for `speaks` and remember what was learnt: every
/// path at once; when none answers and the device is known by its
/// certificate, a look over the local network for it at a new address,
/// which then replaces the old one. The list on disk is read and updated
/// here, in short steps, never held while the network is tried.
pub fn locate(list: &Path, which: &str, speaks: Speaks, timeout: Duration) -> Result<(Device, Answer), LocateError> {
    let device = Devices::load(list)?.find(which)?.clone();
    if device.way(speaks).is_none() {
        return Err(LocateError::NoWay {
            name: device.name.clone(),
            speaks,
        });
    }
    let failed = match reach(&device, speaks, timeout) {
        Ok(answer) => {
            let at = store::now();
            let ((), saved) = Devices::update(list, |d| {
                d.note_reached(&device.id, answer.path, at);
                d.note_identity(&device.id, &answer.seen.identity);
                Ok(())
            })?;
            let device = saved.find(&device.id)?.clone();
            return Ok((device, answer));
        }
        Err(reports) => reports,
    };
    // Moved? Only a device that showed a certificate can be told apart.
    if !device.identity.is_empty() && speaks == Speaks::Rdp {
        let way = device.way(speaks).expect("checked above");
        let again = search(Duration::from_millis(400))
            .into_iter()
            .find(|f| f.way.speaks == speaks && device.identity.matches(&f.seen.identity));
        if let Some(found) = again {
            let at = store::now();
            let host = found.ip.to_string();
            let ((), saved) = Devices::update(list, |d| {
                d.note_moved(&device.id, &host, at);
                d.note_identity(&device.id, &found.seen.identity);
                Ok(())
            })?;
            let device = saved.find(&device.id)?.clone();
            let answer = Answer {
                path: 0,
                way,
                addr: std::net::SocketAddr::new(found.ip, way.port),
                millis: 0,
                seen: found.seen,
            };
            return Ok((device, answer));
        }
    }
    let why = why_not(&device, &failed, &tailscale::status());
    Err(LocateError::NotReached(NotReached { reports: failed, why }))
}

#[derive(Debug, thiserror::Error)]
pub enum LocateError {
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("{name} has no way in for {}", .speaks.label())]
    NoWay { name: String, speaks: Speaks },
    #[error("{0}")]
    NotReached(NotReached),
}

/// Why a device is out of reach, in plain words, from how its paths did
/// and what is known of this computer's own networks.
pub fn why_not(device: &Device, reports: &[PathReport], tailscale: &tailscale::Tailscale) -> String {
    let mut parts: Vec<String> = Vec::new();
    let another = reports.iter().find_map(|r| match &r.outcome {
        Err(Trouble::AnotherDevice(name)) => Some(name.clone()),
        _ => None,
    });
    let refused = reports.iter().any(|r| r.outcome == Err(Trouble::Refused));
    let has = |kind: PathKind| device.paths.iter().any(|p| p.kind == kind);

    if has(PathKind::Address) || has(PathKind::Name) {
        let here = reach::on_same_network(device);
        if let Some(name) = another {
            parts.push(format!("its address on the local network now belongs to another device ({name})"));
        } else if here && refused {
            parts.push("it is on this network, but is not accepting connections (remote access is switched off there)".into());
        } else if here {
            parts.push("it does not answer on this network (switched off, asleep, or it has a new address)".into());
        } else {
            parts.push("this device is not on its network".into());
        }
    }
    if let Some(p) = device.paths.iter().find(|p| p.kind == PathKind::Tailscale) {
        parts.push(match tailscale {
            tailscale::Tailscale::NotInstalled => "Tailscale is not on this device".into(),
            tailscale::Tailscale::Off => "Tailscale is turned off here".into(),
            tailscale::Tailscale::On { peers } => {
                let peer = peers
                    .iter()
                    .find(|x| x.address == p.host || x.dns_name.eq_ignore_ascii_case(p.host.trim_end_matches('.')));
                match peer {
                    Some(x) if !x.online => "it is offline in Tailscale (switched off, asleep, or Tailscale is off there)".into(),
                    Some(_) => "it is online in Tailscale but does not answer there".into(),
                    None => "it is not among your Tailscale devices".into(),
                }
            }
        });
    } else if !has(PathKind::Gateway) || !reach::gateways().is_empty() {
        parts.push("it has no Tailscale address saved".into());
    }
    if has(PathKind::Gateway) && reach::gateways().is_empty() {
        parts.push("this device is not on its hotspot".into());
    }
    if parts.is_empty() {
        return "no way to reach it is saved".into();
    }
    parts.join("; ")
}

/// The path to show for a device: one that answered for the way that
/// shows its screen (else for any way), the quickest of them.
pub fn best(device: &Device, reports: &[PathReport]) -> Option<Answer> {
    let screen = device.screen_way();
    let mut ok: Vec<&Answer> = reports.iter().filter_map(|r| r.outcome.as_ref().ok()).collect();
    ok.sort_by_key(|a| (Some(a.way) != screen, a.millis));
    ok.first().map(|a| (*a).clone())
}

/// What is known of a host before it is added.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Described {
    /// Where to reach it: the host as given, or its Tailscale address when
    /// it was given by its Tailscale name.
    pub host: String,
    pub ways: Vec<Way>,
    /// What each way's server said of itself, in the same order.
    pub banners: Vec<String>,
    pub identity: Identity,
    /// From Tailscale, when the host is one of its devices.
    pub system: String,
    /// A name to suggest: Tailscale's, else the certificate's, else the host.
    pub name: String,
}

/// Ask a host what it speaks (unless `ways` already says), and gather what
/// else is known of it.
pub fn describe(host: &str, ways: &[Way], tailscale: &tailscale::Tailscale, timeout: Duration) -> Described {
    let host = host.trim();
    let peer = tailscale.peers().iter().find(|p| {
        p.address == host || p.name.eq_ignore_ascii_case(host) || p.dns_name.eq_ignore_ascii_case(host.trim_end_matches('.'))
    });
    let reach_host = peer.map(|p| p.address.clone()).unwrap_or_else(|| host.to_string());
    let mut out = Described {
        host: reach_host.clone(),
        ways: ways.to_vec(),
        banners: ways.iter().map(|_| String::new()).collect(),
        system: peer.map(|p| p.os.to_ascii_lowercase()).unwrap_or_default(),
        ..Default::default()
    };
    if ways.is_empty() {
        for (way, _, seen) in detect(&reach_host, timeout) {
            if !seen.identity.is_empty() {
                out.identity = seen.identity.clone();
            }
            out.ways.push(way);
            out.banners.push(if way.speaks == Speaks::Rdp { String::new() } else { seen.banner });
        }
    }
    out.name = peer
        .map(|p| p.name.clone())
        .or_else(|| out.identity.name.clone())
        .unwrap_or_else(|| host.to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_best_answer_is_the_screen_way_then_the_quickest() {
        let d = Device {
            ways: vec![Way::new(Speaks::Ssh), Way::new(Speaks::Rdp)],
            paths: vec![PathTo::new("192.0.2.20"), PathTo::new("100.64.0.7")],
            ..Default::default()
        };
        let answer = |path: usize, speaks: Speaks, millis: u32| PathReport {
            path,
            way: Way::new(speaks),
            outcome: Ok(Answer {
                path,
                way: Way::new(speaks),
                addr: "192.0.2.20:1".parse().unwrap(),
                millis,
                seen: Seen {
                    speaks,
                    banner: String::new(),
                    identity: Default::default(),
                },
            }),
        };
        let reports = vec![answer(0, Speaks::Ssh, 2), answer(1, Speaks::Rdp, 40), answer(0, Speaks::Rdp, 9)];
        let b = best(&d, &reports).unwrap();
        assert_eq!((b.path, b.way.speaks, b.millis), (0, Speaks::Rdp, 9));
    }

    #[test]
    fn a_tailscale_name_is_reached_at_its_address() {
        let ts = tailscale::Tailscale::On {
            peers: vec![tailscale::Peer {
                name: "travel-laptop".into(),
                dns_name: "travel-laptop.example.ts.net".into(),
                os: "Linux".into(),
                address: "100.64.0.7".into(),
                online: true,
            }],
        };
        let d = describe("Travel-Laptop", &[Way::new(Speaks::Ssh)], &ts, Duration::from_millis(100));
        assert_eq!(d.host, "100.64.0.7");
        assert_eq!(d.system, "linux");
        assert_eq!(d.name, "travel-laptop");
        // Not a Tailscale device: the host as given, named after itself.
        let d = describe("192.0.2.20", &[Way::new(Speaks::Rdp)], &ts, Duration::from_millis(100));
        assert_eq!((d.host.as_str(), d.name.as_str(), d.system.as_str()), ("192.0.2.20", "192.0.2.20", ""));
    }

    #[test]
    fn out_of_reach_says_why() {
        let d = Device {
            ways: vec![Way::new(Speaks::Rdp)],
            paths: vec![PathTo::new("100.64.0.7")],
            ..Default::default()
        };
        assert_eq!(why_not(&d, &[], &tailscale::Tailscale::Off), "Tailscale is turned off here");
        let ts = tailscale::Tailscale::On {
            peers: vec![tailscale::Peer {
                name: "x".into(),
                dns_name: String::new(),
                os: String::new(),
                address: "100.64.0.7".into(),
                online: false,
            }],
        };
        assert!(why_not(&d, &[], &ts).starts_with("it is offline in Tailscale"));
    }
}
