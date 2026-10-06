//! Cameras on the local network: an ONVIF WS-Discovery probe (what Tapo,
//! Xiaomi with RTSP firmware, Reolink and most IP cameras answer), and the
//! neighbour table to learn each answerer's hardware address, so a camera
//! on the LAN can be paired with its cloud entry.

use roxmltree::Document;
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};

const MULTICAST: &str = "239.255.255.250:3702";

/// One camera that answered on the local network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCamera {
    pub address: Ipv4Addr,
    /// ONVIF scopes, e.g. `onvif://www.onvif.org/hardware/C100`.
    pub scopes: Vec<String>,
    /// Service addresses, e.g. `http://192.0.2.9:2020/onvif/device_service`.
    pub xaddrs: Vec<String>,
}

impl LocalCamera {
    fn scope(&self, kind: &str) -> Option<String> {
        let prefix = format!("onvif://www.onvif.org/{kind}/");
        self.scopes
            .iter()
            .find_map(|s| s.strip_prefix(&prefix))
            .map(percent_decode)
    }

    /// The hardware/model scope, when the camera gives one.
    pub fn hardware(&self) -> Option<String> {
        self.scope("hardware")
    }

    pub fn name(&self) -> Option<String> {
        self.scope("name")
    }

    /// A hardware address the camera itself put in its scopes, if any.
    pub fn mac_from_scopes(&self) -> Option<String> {
        self.scope("mac").map(|m| normalise_mac(&m))
    }
}

/// Ask the network for ONVIF cameras and listen for `timeout`.
pub fn probe(timeout: Duration) -> io::Result<Vec<LocalCamera>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    socket.set_multicast_ttl_v4(1)?;
    let message_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    let probe = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope" xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:wsd="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl"><soap:Header><wsa:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</wsa:To><wsa:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</wsa:Action><wsa:MessageID>{message_id}</wsa:MessageID></soap:Header><soap:Body><wsd:Probe><wsd:Types>dn:NetworkVideoTransmitter</wsd:Types></wsd:Probe></soap:Body></soap:Envelope>"#
    );
    socket.send_to(probe.as_bytes(), MULTICAST)?;
    let deadline = Instant::now() + timeout;
    let resend_at = Instant::now() + Duration::from_secs(1);
    let mut resent = false;
    let mut buf = vec![0u8; 65536];
    let mut found: Vec<LocalCamera> = Vec::new();
    while Instant::now() < deadline {
        if !resent && Instant::now() >= resend_at {
            resent = true;
            let _ = socket.send_to(probe.as_bytes(), MULTICAST);
        }
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let IpAddr::V4(address) = from.ip() else {
                    continue;
                };
                if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                    for (scopes, xaddrs) in parse_matches(text) {
                        if !found.iter().any(|c| c.address == address) {
                            found.push(LocalCamera {
                                address,
                                scopes,
                                xaddrs,
                            });
                        }
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(found)
}

fn parse_matches(xml: &str) -> Vec<(Vec<String>, Vec<String>)> {
    let Ok(doc) = Document::parse(xml) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("ProbeMatch"))
        .map(|pm| {
            let text = |tag: &str| {
                pm.descendants()
                    .find(|n| n.has_tag_name(tag))
                    .and_then(|n| n.text())
                    .unwrap_or("")
                    .split_whitespace()
                    .map(String::from)
                    .collect::<Vec<_>>()
            };
            (text("Scopes"), text("XAddrs"))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// "54:AF:97:00:11:22" / "54-af-97-..." / "54af97001122" → "54af97001122".
pub fn normalise_mac(mac: &str) -> String {
    mac.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The neighbour (ARP) table: address → hardware address, normalised.
pub fn neighbours() -> HashMap<Ipv4Addr, String> {
    let mut out = HashMap::new();
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/net/arp") {
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() >= 4 {
                    if let Ok(ip) = cols[0].parse::<Ipv4Addr>() {
                        let mac = normalise_mac(cols[3]);
                        if mac.len() == 12 && mac != "000000000000" {
                            out.insert(ip, mac);
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        if let Ok(output) = std::process::Command::new("arp").arg("-a").output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let ip = line
                    .split(|c: char| c == '(' || c == ')' || c.is_whitespace())
                    .find_map(|w| w.parse::<Ipv4Addr>().ok());
                let mac = line
                    .split_whitespace()
                    .map(normalise_mac)
                    .find(|m| m.len() == 12);
                if let (Some(ip), Some(mac)) = (ip, mac) {
                    out.insert(ip, mac);
                }
            }
        }
    }
    out
}

/// Pair cameras known by hardware address with those found on the LAN:
/// returns mac → address for every match. Probing first makes the
/// neighbour table fresh for the answerers.
pub fn find_by_mac(macs: &[String], timeout: Duration) -> HashMap<String, Ipv4Addr> {
    let wanted: Vec<String> = macs.iter().map(|m| normalise_mac(m)).collect();
    let cameras = probe(timeout).unwrap_or_default();
    let table = neighbours();
    let mut out = HashMap::new();
    for cam in &cameras {
        let mac = cam
            .mac_from_scopes()
            .or_else(|| table.get(&cam.address).cloned());
        if let Some(mac) = mac {
            if wanted.contains(&mac) {
                out.insert(mac, cam.address);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onvif_answers_parse_into_scopes_and_addresses() {
        let xml = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery"><s:Body><d:ProbeMatches><d:ProbeMatch><d:Scopes>onvif://www.onvif.org/type/video_encoder onvif://www.onvif.org/hardware/C100 onvif://www.onvif.org/name/Tapo%20C100 onvif://www.onvif.org/mac/54:AF:97:00:11:22</d:Scopes><d:XAddrs>http://192.0.2.9:2020/onvif/device_service</d:XAddrs></d:ProbeMatch></d:ProbeMatches></s:Body></s:Envelope>"#;
        let m = parse_matches(xml);
        assert_eq!(m.len(), 1);
        let cam = LocalCamera {
            address: "192.0.2.9".parse().unwrap(),
            scopes: m[0].0.clone(),
            xaddrs: m[0].1.clone(),
        };
        assert_eq!(cam.hardware().as_deref(), Some("C100"));
        assert_eq!(cam.name().as_deref(), Some("Tapo C100"));
        assert_eq!(cam.mac_from_scopes().as_deref(), Some("54af97001122"));
        assert_eq!(normalise_mac("54-AF-97-00-11-22"), "54af97001122");
    }
}
