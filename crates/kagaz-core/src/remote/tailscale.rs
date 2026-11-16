//! Tailscale on this computer, read only: whether it is running, and the
//! person's devices in it with their addresses. Kagaz asks the `tailscale`
//! command for its status and changes nothing there. A phone gives other
//! apps no such list; there the address is typed, or comes with a device
//! shared from a computer.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::{Command, Stdio};

/// One of the person's devices in their Tailscale network.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Peer {
    /// The device's name there ("travel-laptop").
    pub name: String,
    /// Its full name, which Tailscale's own DNS resolves.
    pub dns_name: String,
    /// "windows", "linux", "android", "macOS", "iOS"…, as Tailscale says it.
    pub os: String,
    /// Its Tailscale address (the IPv4 one).
    pub address: String,
    pub online: bool,
}

/// Tailscale as it is on this computer now.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Tailscale {
    /// The `tailscale` command is not here.
    NotInstalled,
    /// Installed, but not connected (signed out, or switched off).
    Off,
    On { peers: Vec<Peer> },
}

impl Tailscale {
    pub fn is_on(&self) -> bool {
        matches!(self, Tailscale::On { .. })
    }

    pub fn peers(&self) -> &[Peer] {
        match self {
            Tailscale::On { peers } => peers,
            _ => &[],
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Status {
    #[serde(default)]
    backend_state: String,
    #[serde(default)]
    peer: BTreeMap<String, Node>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Node {
    #[serde(default)]
    host_name: String,
    #[serde(default, rename = "DNSName")]
    dns_name: String,
    #[serde(default, rename = "OS")]
    os: String,
    #[serde(default, rename = "TailscaleIPs")]
    ips: Vec<String>,
    #[serde(default)]
    online: bool,
}

/// What `tailscale status --json` printed.
pub fn parse(json: &str) -> Option<Tailscale> {
    let status: Status = serde_json::from_str(json).ok()?;
    if status.backend_state != "Running" {
        return Some(Tailscale::Off);
    }
    let mut peers: Vec<Peer> = status
        .peer
        .into_values()
        .filter_map(|n| {
            let address = n.ips.iter().find(|ip| !ip.contains(':')).or(n.ips.first())?.clone();
            // The name before the first dot of the DNS name is the one the
            // person sees in Tailscale; the host name can differ from it.
            let dns_name = n.dns_name.trim_end_matches('.').to_string();
            let name = dns_name
                .split('.')
                .next()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or(n.host_name);
            Some(Peer {
                name,
                dns_name,
                os: n.os,
                address,
                online: n.online,
            })
        })
        .collect();
    peers.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.name.cmp(&b.name)));
    Some(Tailscale::On { peers })
}

/// Ask the `tailscale` command on this computer.
pub fn status() -> Tailscale {
    if cfg!(any(target_os = "android", target_os = "ios")) {
        return Tailscale::NotInstalled;
    }
    let out = Command::new("tailscale")
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Err(_) => Tailscale::NotInstalled,
        Ok(out) => parse(&String::from_utf8_lossy(&out.stdout)).unwrap_or(Tailscale::Off),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shape of `tailscale status --json`, with invented devices.
    const STATUS: &str = r#"{
      "Version": "1.80.0",
      "BackendState": "Running",
      "Self": { "HostName": "this-one", "DNSName": "this-one.example.ts.net.", "OS": "linux", "TailscaleIPs": ["100.64.0.1"], "Online": true },
      "Peer": {
        "nodekey:aa": { "HostName": "Media Box", "DNSName": "media-box.example.ts.net.", "OS": "windows", "TailscaleIPs": ["100.64.0.9", "fd7a:115c:a1e0::9"], "Online": false },
        "nodekey:bb": { "HostName": "travel-laptop", "DNSName": "travel-laptop.example.ts.net.", "OS": "linux", "TailscaleIPs": ["fd7a:115c:a1e0::7", "100.64.0.7"], "Online": true },
        "nodekey:cc": { "HostName": "no-address", "DNSName": "", "OS": "android", "TailscaleIPs": [], "Online": true }
      }
    }"#;

    #[test]
    fn peers_come_with_their_ipv4_address_online_first() {
        let Tailscale::On { peers } = parse(STATUS).unwrap() else {
            panic!("should be on");
        };
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].name, "travel-laptop");
        assert_eq!(peers[0].address, "100.64.0.7");
        assert!(peers[0].online);
        assert_eq!(peers[1].name, "media-box");
        assert_eq!(peers[1].dns_name, "media-box.example.ts.net");
        assert!(!peers[1].online);
    }

    #[test]
    fn a_stopped_tailscale_is_off() {
        assert_eq!(parse(r#"{"BackendState":"Stopped"}"#), Some(Tailscale::Off));
        assert_eq!(parse(r#"{"BackendState":"NeedsLogin","Peer":{}}"#), Some(Tailscale::Off));
        assert_eq!(parse("not json"), None);
    }
}
