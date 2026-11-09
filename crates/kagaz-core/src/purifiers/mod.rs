//! Air purifiers (Philips) on the Wi-Fi network: finding them, and the
//! list of the ones the person added, kept in `<config>/purifiers.toml`.
//!
//! Finding is by SSDP: a Philips purifier answers a search for its device
//! type with where its description is, and the description names it and
//! its model. How it talks is probed too, so the page can say: CoAP on
//! UDP 5683 for purifiers from about 2020 on, HTTP on port 80 before.
//! Switching a purifier and reading its air comes once that is checked
//! against a real one.

use serde::{Deserialize, Serialize};
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SSDP: &str = "239.255.255.250:1900";
const PHILIPS: &str = "urn:philips-com:device:DiProduct:1";

/// A purifier that answered on the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    /// The purifier's own id (its UDN), else its address.
    pub id: String,
    /// What it calls itself.
    pub name: String,
    /// The model, when its description says ("AC3033/30").
    pub model: String,
    pub address: String,
    /// "coap", "http", or "unknown".
    pub protocol: String,
}

/// A purifier the person added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Purifier {
    pub id: String,
    /// The name the person gave it.
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub protocol: String,
    /// When it last answered (seconds since the epoch), 0 for never.
    #[serde(default)]
    pub last_seen: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Purifiers {
    #[serde(default)]
    pub purifiers: Vec<Purifier>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot read or write {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path} is not a purifiers list: {reason}")]
    Format { path: PathBuf, reason: String },
    #[error("no purifier with id {0}")]
    Unknown(String),
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Purifiers {
    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("purifiers.toml")
    }

    /// The list; an empty one when there is no file yet.
    pub fn load(path: &Path) -> Result<Purifiers, StoreError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Purifiers::default()),
            Err(e) => return Err(StoreError::Io { path: path.to_path_buf(), source: e }),
        };
        toml::from_str(&text).map_err(|e| StoreError::Format { path: path.to_path_buf(), reason: e.to_string() })
    }

    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        let io = |source| StoreError::Io { path: path.to_path_buf(), source };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| StoreError::Format { path: path.to_path_buf(), reason: e.to_string() })?;
        let tmp = path.with_extension("toml.new");
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    pub fn find(&self, id: &str) -> Option<&Purifier> {
        self.purifiers.iter().find(|p| p.id == id)
    }

    /// Add one found (or bring its details up to date if it is there).
    pub fn add(&mut self, found: &Found, name: &str) -> &Purifier {
        let name = if name.trim().is_empty() { found.name.as_str() } else { name.trim() };
        let i = match self.purifiers.iter().position(|p| p.id == found.id) {
            Some(i) => i,
            None => {
                self.purifiers.push(Purifier {
                    id: found.id.clone(),
                    name: String::new(),
                    model: String::new(),
                    address: String::new(),
                    protocol: String::new(),
                    last_seen: 0,
                });
                self.purifiers.len() - 1
            }
        };
        let p = &mut self.purifiers[i];
        p.name = name.to_string();
        p.model = found.model.clone();
        p.address = found.address.clone();
        p.protocol = found.protocol.clone();
        p.last_seen = now();
        p
    }

    /// One answered a search: its address and the time, if it is on the list.
    pub fn seen(&mut self, found: &Found) {
        if let Some(p) = self.purifiers.iter_mut().find(|p| p.id == found.id) {
            p.address = found.address.clone();
            if found.protocol != "unknown" {
                p.protocol = found.protocol.clone();
            }
            if !found.model.is_empty() {
                p.model = found.model.clone();
            }
            p.last_seen = now();
        }
    }

    pub fn forget(&mut self, id: &str) -> Result<Purifier, StoreError> {
        let i = self.purifiers.iter().position(|p| p.id == id).ok_or_else(|| StoreError::Unknown(id.into()))?;
        Ok(self.purifiers.remove(i))
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), StoreError> {
        let p = self.purifiers.iter_mut().find(|p| p.id == id).ok_or_else(|| StoreError::Unknown(id.into()))?;
        if !name.trim().is_empty() {
            p.name = name.trim().to_string();
        }
        Ok(())
    }
}

/// Ask the network for Philips purifiers, for about `timeout`.
pub fn discover(timeout: Duration) -> io::Result<Vec<Found>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    socket.set_multicast_ttl_v4(2)?;
    let search = format!("M-SEARCH * HTTP/1.1\r\nHOST: {SSDP}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {PHILIPS}\r\n\r\n");
    socket.send_to(search.as_bytes(), SSDP)?;
    let start = Instant::now();
    let mut again = false;
    let mut answers: Vec<(IpAddr, String)> = Vec::new();
    let mut buf = [0u8; 2048];
    while start.elapsed() < timeout {
        if !again && start.elapsed() > timeout / 2 {
            let _ = socket.send_to(search.as_bytes(), SSDP);
            again = true;
        }
        let Ok((n, from)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let text = String::from_utf8_lossy(&buf[..n]);
        if !is_philips(&text) || answers.iter().any(|(a, _)| *a == from.ip()) {
            continue;
        }
        answers.push((from.ip(), header(&text, "location").unwrap_or_default()));
    }
    Ok(answers.into_iter().map(|(ip, location)| describe(ip, &location)).collect())
}

/// An SSDP answer from a Philips purifier (its device type, or Philips named in ST or SERVER).
fn is_philips(answer: &str) -> bool {
    let st = header(answer, "st").unwrap_or_default();
    let server = header(answer, "server").unwrap_or_default();
    st == PHILIPS || st.to_ascii_lowercase().contains("philips") || server.to_ascii_lowercase().contains("philips")
}

fn header(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
    })
}

/// The purifier's description (name, model, id), then how it talks.
fn describe(ip: IpAddr, location: &str) -> Found {
    let mut found = Found {
        id: ip.to_string(),
        name: "Philips purifier".into(),
        model: String::new(),
        address: ip.to_string(),
        protocol: "unknown".into(),
    };
    if !location.is_empty() {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(3)).build();
        if let Ok(xml) = agent.get(location).call().and_then(|r| r.into_string().map_err(ureq::Error::from)) {
            if let Some(name) = tag(&xml, "friendlyName") {
                found.name = name;
            }
            if let Some(model) = tag(&xml, "modelName").or_else(|| tag(&xml, "modelNumber")) {
                found.model = model;
            }
            if let Some(udn) = tag(&xml, "UDN") {
                found.id = udn.trim_start_matches("uuid:").to_string();
            }
        }
    }
    found.protocol = protocol(ip);
    found
}

/// The text inside the first `<name>…</name>` of a small XML document.
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let value = xml[start..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// How it talks: CoAP (a GET on UDP 5683 is answered), else the old HTTP
/// interface (its device page answers), else unknown.
fn protocol(ip: IpAddr) -> String {
    if coap_answers(ip) {
        "coap".into()
    } else if http_answers(ip) {
        "http".into()
    } else {
        "unknown".into()
    }
}

/// A confirmable CoAP GET of /sys/dev/status; any CoAP reply counts.
fn coap_answers(ip: IpAddr) -> bool {
    let mut message = vec![0x40, 0x01, 0x12, 0x34];
    let mut previous = 0u8;
    for segment in ["sys", "dev", "status"] {
        // Uri-Path is option 11; option deltas are from the previous option.
        let delta = 11 - previous;
        previous = 11;
        message.push((delta << 4) | segment.len() as u8);
        message.extend_from_slice(segment.as_bytes());
    }
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return false;
    };
    let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
    if socket.send_to(&message, SocketAddr::new(ip, 5683)).is_err() {
        return false;
    }
    let mut buf = [0u8; 1500];
    matches!(socket.recv_from(&mut buf), Ok((n, _)) if n >= 4 && buf[0] >> 6 == 1)
}

fn http_answers(ip: IpAddr) -> bool {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build();
    matches!(agent.get(&format!("http://{ip}/di/v1/products/1/device")).call(), Ok(r) if r.status() == 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_headers_and_tags() {
        let answer = "HTTP/1.1 200 OK\r\nLOCATION: http://192.0.2.9:80/desc.xml\r\nST: urn:philips-com:device:DiProduct:1\r\n\r\n";
        assert!(is_philips(answer));
        assert_eq!(header(answer, "location").as_deref(), Some("http://192.0.2.9:80/desc.xml"));
        assert!(!is_philips("HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\nSERVER: Linux UPnP/1.0\r\n\r\n"));
        let xml = "<root><device><friendlyName>Living room</friendlyName><modelName>AC3033/30</modelName><UDN>uuid:12345678-abcd</UDN></device></root>";
        assert_eq!(tag(xml, "friendlyName").as_deref(), Some("Living room"));
        assert_eq!(tag(xml, "modelName").as_deref(), Some("AC3033/30"));
        assert_eq!(tag(xml, "serialNumber"), None);
    }

    #[test]
    fn list_round_trip() {
        let dir = std::env::temp_dir().join(format!("kagaz-purifiers-test-{}", std::process::id()));
        let path = dir.join("purifiers.toml");
        let found = Found { id: "12345678-abcd".into(), name: "Living room".into(), model: "AC3033/30".into(), address: "192.0.2.9".into(), protocol: "coap".into() };
        let mut list = Purifiers::load(&path).unwrap();
        assert!(list.purifiers.is_empty());
        list.add(&found, "Hall purifier");
        list.save(&path).unwrap();
        let again = Purifiers::load(&path).unwrap();
        assert_eq!(again.purifiers.len(), 1);
        assert_eq!(again.purifiers[0].name, "Hall purifier");
        assert_eq!(again.purifiers[0].model, "AC3033/30");
        let mut again = again;
        again.rename("12345678-abcd", "Study").unwrap();
        assert_eq!(again.find("12345678-abcd").unwrap().name, "Study");
        assert!(again.forget("nope").is_err());
        again.forget("12345678-abcd").unwrap();
        assert!(again.purifiers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
