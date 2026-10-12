//! The door stations themselves: which ones this computer knows about,
//! and finding them on the network. A station that has Direct IP calling
//! on answers SIP on its listen port (5060), so one SIP OPTIONS to every
//! address on the local network finds the stations without any account.

use super::sip;
use super::Error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A door station (or indoor unit) this computer knows about.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Station {
    /// What the person calls it ("Front door", "Reception").
    pub name: String,
    /// Its IPv4 address on this network.
    pub address: String,
    /// The port it takes SIP calls on (Direct IP), normally 5060.
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    5060
}

/// The saved list, with the port Kagaz itself listens on for their calls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stations {
    #[serde(default = "default_port")]
    pub listen_port: u16,
    #[serde(default)]
    pub stations: Vec<Station>,
}

impl Default for Stations {
    fn default() -> Self {
        Stations {
            listen_port: default_port(),
            stations: Vec::new(),
        }
    }
}

impl Stations {
    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("door-stations.json")
    }

    /// The saved list; an empty one when nothing was saved yet.
    pub fn load() -> Stations {
        Self::load_from(&Self::default_path()).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Option<Stations> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self) -> Result<(), Error> {
        self.save_to(&Self::default_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), Error> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::Config(e.to_string()))?;
        }
        let text = serde_json::to_string_pretty(self).expect("json");
        std::fs::write(path, text).map_err(|e| Error::Config(e.to_string()))
    }

    /// Add a station, or rename the one already saved at that address.
    pub fn put(&mut self, station: Station) {
        match self
            .stations
            .iter_mut()
            .find(|s| s.address == station.address)
        {
            Some(existing) => *existing = station,
            None => self.stations.push(station),
        }
    }

    pub fn remove(&mut self, address: &str) -> bool {
        let before = self.stations.len();
        self.stations.retain(|s| s.address != address);
        self.stations.len() != before
    }

    /// The saved station at an address, if any.
    pub fn at(&self, address: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.address == address)
    }
}

/// A SIP endpoint that answered on the network.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Found {
    pub address: String,
    pub port: u16,
    /// What it calls itself (its User-Agent or Server header).
    pub agent: String,
}

/// Ask every address on the local IPv4 networks whether it speaks SIP on
/// `port`, and return the ones that answered. Only the /24 around each
/// interface is asked, so this is at most a few hundred small packets.
pub fn discover(port: u16, wait: Duration) -> Result<Vec<Found>, Error> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| Error::Transport(e.to_string()))?;
    let local_port = sock
        .local_addr()
        .map_err(|e| Error::Transport(e.to_string()))?
        .port();
    let mut targets: Vec<(Ipv4Addr, Ipv4Addr)> = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = iface.addr {
                for host in hosts_around(v4.ip, v4.netmask) {
                    targets.push((v4.ip, host));
                }
            }
        }
    }
    for (from, host) in &targets {
        let local = format!("{from}:{local_port}");
        let req = options_request(&host.to_string(), port, &local);
        let _ = sock.send_to(req.as_bytes(), (*host, port));
    }
    Ok(collect(&sock, wait))
}

/// Ask one address whether it speaks SIP; its User-Agent when it does.
pub fn probe(address: &str, port: u16, wait: Duration) -> Result<Option<Found>, Error> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| Error::Transport(e.to_string()))?;
    let target: SocketAddr = format!("{address}:{port}")
        .parse()
        .map_err(|_| Error::Config(format!("not an address: {address}")))?;
    sock.connect(target)
        .map_err(|e| Error::Transport(e.to_string()))?;
    let local = sock
        .local_addr()
        .map_err(|e| Error::Transport(e.to_string()))?
        .to_string();
    let req = options_request(address, port, &local);
    sock.send(req.as_bytes())
        .map_err(|e| Error::Transport(e.to_string()))?;
    Ok(collect(&sock, wait).into_iter().next())
}

/// Gather the answers that arrive within `wait`, one per address.
fn collect(sock: &UdpSocket, wait: Duration) -> Vec<Found> {
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let started = Instant::now();
    let mut buf = vec![0u8; 4096];
    loop {
        let left = wait.saturating_sub(started.elapsed());
        if left.is_zero() {
            break;
        }
        if sock.set_read_timeout(Some(left)).is_err() {
            break;
        }
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(x) => x,
            Err(_) => break,
        };
        let Ok(msg) = sip::parse(&String::from_utf8_lossy(&buf[..n])) else {
            continue;
        };
        if msg.status().is_none() {
            continue;
        }
        let agent = msg
            .header("User-Agent")
            .or_else(|| msg.header("Server"))
            .unwrap_or("SIP device")
            .to_string();
        found.entry(from.ip().to_string()).or_insert(Found {
            address: from.ip().to_string(),
            port: from.port(),
            agent,
        });
    }
    found.into_values().collect()
}

/// A SIP OPTIONS request to `host:port`, from our `local` ip:port.
fn options_request(host: &str, port: u16, local: &str) -> String {
    let local_ip = local.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(local);
    let tag = sip::token();
    let branch = format!("z9hG4bK{}", sip::token());
    let call_id = format!("{}@{local_ip}", sip::token());
    let lines = [
        format!("OPTIONS sip:{host}:{port} SIP/2.0"),
        format!("Via: SIP/2.0/UDP {local};branch={branch};rport"),
        "Max-Forwards: 70".to_string(),
        format!("From: <sip:kagaz@{local_ip}>;tag={tag}"),
        format!("To: <sip:{host}:{port}>"),
        format!("Call-ID: {call_id}"),
        "CSeq: 1 OPTIONS".to_string(),
        format!("Contact: <sip:kagaz@{local}>"),
        "Accept: application/sdp".to_string(),
        "User-Agent: Kagaz".to_string(),
        "Content-Length: 0".to_string(),
    ];
    format!("{}\r\n\r\n", lines.join("\r\n"))
}

/// Every other host in the /24 (or smaller network) around `ip`.
fn hosts_around(ip: Ipv4Addr, netmask: Ipv4Addr) -> Vec<Ipv4Addr> {
    let ip_bits = u32::from(ip);
    // Never wider than a /24: a /16 would be 65k packets.
    let mask = u32::from(netmask) | 0xFFFF_FF00;
    let network = ip_bits & mask;
    let broadcast = network | !mask;
    (network + 1..broadcast)
        .filter(|&h| h != ip_bits)
        .map(Ipv4Addr::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_cover_the_slash_24_but_not_wider() {
        let hosts = hosts_around(Ipv4Addr::new(10, 1, 2, 3), Ipv4Addr::new(255, 255, 0, 0));
        assert_eq!(hosts.len(), 253);
        assert_eq!(hosts[0], Ipv4Addr::new(10, 1, 2, 1));
        assert!(!hosts.contains(&Ipv4Addr::new(10, 1, 2, 3)));
        assert_eq!(*hosts.last().unwrap(), Ipv4Addr::new(10, 1, 2, 254));
    }

    #[test]
    fn a_small_network_is_smaller() {
        let hosts = hosts_around(
            Ipv4Addr::new(10, 1, 2, 3),
            Ipv4Addr::new(255, 255, 255, 240),
        );
        assert_eq!(hosts.len(), 13);
    }

    #[test]
    fn options_request_is_well_formed() {
        let req = options_request("10.0.0.9", 5060, "10.0.0.2:40000");
        assert!(req.starts_with("OPTIONS sip:10.0.0.9:5060 SIP/2.0\r\n"));
        assert!(req.contains("Via: SIP/2.0/UDP 10.0.0.2:40000;branch=z9hG4bK"));
        assert!(req.contains("From: <sip:kagaz@10.0.0.2>;tag="));
        assert!(req.ends_with("Content-Length: 0\r\n\r\n"));
        let parsed = sip::parse(&req).unwrap();
        assert_eq!(parsed.method().as_deref(), Some("OPTIONS"));
    }

    #[test]
    fn stations_round_trip_and_replace_by_address() {
        let dir = std::env::temp_dir().join(format!("kagaz-stations-{}", std::process::id()));
        let path = dir.join("door-stations.json");
        let mut list = Stations::default();
        list.put(Station {
            name: "Door".into(),
            address: "10.0.0.9".into(),
            port: 5060,
        });
        list.put(Station {
            name: "Front door".into(),
            address: "10.0.0.9".into(),
            port: 5060,
        });
        assert_eq!(list.stations.len(), 1);
        assert_eq!(list.stations[0].name, "Front door");
        list.save_to(&path).unwrap();
        let back = Stations::load_from(&path).unwrap();
        assert_eq!(back, list);
        assert!(list.remove("10.0.0.9"));
        assert!(!list.remove("10.0.0.9"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_probe_finds_a_sip_answerer() {
        // A stand-in station: answers OPTIONS with 200 OK and a User-Agent.
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 4096];
            let (n, from) = server.recv_from(&mut buf).unwrap();
            let msg = sip::parse(&String::from_utf8_lossy(&buf[..n])).unwrap();
            let mut reply = msg.response_to(200, "OK", "abc", None, None);
            reply = reply.replacen(
                "SIP/2.0 200 OK\r\n",
                "SIP/2.0 200 OK\r\nUser-Agent: Door 1.0\r\n",
                1,
            );
            server.send_to(reply.as_bytes(), from).unwrap();
        });
        let found = probe("127.0.0.1", port, Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(found.address, "127.0.0.1");
        assert_eq!(found.agent, "Door 1.0");
        assert!(probe("127.0.0.1", 1, Duration::from_millis(200))
            .unwrap()
            .is_none());
    }
}
