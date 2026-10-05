//! WS-Discovery: how Windows finds "Web Services for Devices" printers and
//! scanners. A multicast Probe on UDP 3702, then a metadata Get over HTTP to
//! learn the model and the hosted print/scan services.

use crate::device::{Device, Protocol, Service};
use roxmltree::Document;
use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const MULTICAST: &str = "239.255.255.250:3702";

pub fn probe(timeout: Duration) -> io::Result<Vec<Device>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    socket.set_multicast_ttl_v4(1)?;

    let message_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    let probe = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope" xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:wsd="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:wsdp="http://schemas.xmlsoap.org/ws/2006/02/devprof"><soap:Header><wsa:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</wsa:To><wsa:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</wsa:Action><wsa:MessageID>{message_id}</wsa:MessageID></soap:Header><soap:Body><wsd:Probe><wsd:Types>wsdp:Device</wsd:Types></wsd:Probe></soap:Body></soap:Envelope>"#
    );
    socket.send_to(probe.as_bytes(), MULTICAST)?;

    let deadline = Instant::now() + timeout;
    let mut buf = vec![0u8; 65536];
    let mut matches: Vec<(SocketAddr, ProbeMatch)> = Vec::new();
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                    for m in parse_probe_matches(text) {
                        if !matches.iter().any(|(_, e)| e.address == m.address) {
                            matches.push((from, m));
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

    Ok(matches
        .into_iter()
        .filter_map(|(from, m)| device_from_match(from.ip(), m))
        .collect())
}

#[derive(Debug, Clone)]
struct ProbeMatch {
    address: String,
    types: Vec<String>,
    xaddrs: Vec<String>,
}

fn parse_probe_matches(xml: &str) -> Vec<ProbeMatch> {
    let Ok(doc) = Document::parse(xml) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("ProbeMatch"))
        .filter_map(|pm| {
            let text = |tag: &str| {
                pm.descendants()
                    .find(|n| n.has_tag_name(tag))
                    .and_then(|n| n.text())
                    .map(|t| t.trim().to_string())
            };
            Some(ProbeMatch {
                address: text("Address")?,
                types: text("Types")
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(String::from)
                    .collect(),
                xaddrs: text("XAddrs")
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(String::from)
                    .collect(),
            })
        })
        .collect()
}

/// Build a device from a probe match; `None` for things that answer
/// WS-Discovery but are neither printers nor scanners (cameras, NAS boxes).
fn device_from_match(from: IpAddr, m: ProbeMatch) -> Option<Device> {
    let mut device = Device {
        name: String::new(),
        addresses: vec![from],
        uuid: m.address.strip_prefix("urn:uuid:").map(String::from),
        ..Default::default()
    };
    // Prefer an XAddr on the address that answered (devices list IPv6 too).
    let xaddr = m
        .xaddrs
        .iter()
        .find(|x| x.contains(&from.to_string()))
        .or(m.xaddrs.first())
        .cloned();

    let mut protocols: Vec<Protocol> = Vec::new();
    let mut attributes = BTreeMap::new();
    attributes.insert("types".into(), m.types.join(" "));
    if let Some(x) = &xaddr {
        attributes.insert("xaddr".into(), x.clone());
        if let Some(meta) = get_metadata(x, &m.address) {
            device.name = meta.friendly_name.clone().unwrap_or_default();
            device.manufacturer = meta.manufacturer;
            device.model = meta.model;
            for hosted in meta.hosted_types {
                if hosted.contains("PrinterServiceType") {
                    protocols.push(Protocol::WsdPrint);
                } else if hosted.contains("ScannerServiceType") {
                    protocols.push(Protocol::WsdScan);
                }
            }
        }
    }
    // Fall back to the probe's own type list when metadata is unavailable.
    if protocols.is_empty() {
        for t in &m.types {
            if t.contains("PrintDeviceType") {
                protocols.push(Protocol::WsdPrint);
            } else if t.contains("ScanDeviceType") {
                protocols.push(Protocol::WsdScan);
            }
        }
    }
    if protocols.is_empty() {
        return None;
    }
    device.services = protocols
        .into_iter()
        .map(|p| Service {
            protocol: p,
            source: "wsd".into(),
            port: xaddr.as_deref().and_then(port_of),
            endpoint: xaddr.clone(),
            attributes: attributes.clone(),
        })
        .collect();
    Some(device)
}

fn port_of(url: &str) -> Option<u16> {
    let rest = url.split("://").nth(1)?;
    let host_port = rest.split('/').next()?;
    host_port.rsplit(':').next()?.parse().ok()
}

#[derive(Debug, Default)]
struct Metadata {
    friendly_name: Option<String>,
    manufacturer: Option<String>,
    model: Option<String>,
    hosted_types: Vec<String>,
}

/// WS-Transfer Get: asks the device for its model and hosted services.
fn get_metadata(xaddr: &str, to: &str) -> Option<Metadata> {
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope" xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing"><soap:Header><wsa:To>{to}</wsa:To><wsa:Action>http://schemas.xmlsoap.org/ws/2004/09/transfer/Get</wsa:Action><wsa:MessageID>urn:uuid:{}</wsa:MessageID><wsa:ReplyTo><wsa:Address>http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</wsa:Address></wsa:ReplyTo></soap:Header><soap:Body/></soap:Envelope>"#,
        uuid::Uuid::new_v4()
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .build();
    let resp = agent
        .post(xaddr)
        .set("Content-Type", "application/soap+xml; charset=utf-8")
        .send_string(&body)
        .ok()?;
    let text = resp.into_string().ok()?;
    let doc = Document::parse(&text).ok()?;
    let find = |tag: &str| {
        doc.descendants()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    let hosted_types = doc
        .descendants()
        .filter(|n| n.has_tag_name("Hosted"))
        .filter_map(|h| {
            h.descendants()
                .find(|n| n.has_tag_name("Types"))
                .and_then(|n| n.text())
        })
        .map(|t| t.trim().to_string())
        .collect();
    Some(Metadata {
        friendly_name: find("FriendlyName"),
        manufacturer: find("Manufacturer"),
        model: find("ModelName"),
        hosted_types,
    })
}
