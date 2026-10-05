//! SNMP: the last resort for printers that announce nothing. Almost every
//! network printer answers an SNMP GET with community `public`, including one
//! sent to the broadcast address, which is how CUPS and Windows find printers
//! that do not speak mDNS or WS-Discovery.
//!
//! Only what is needed lives here: a BER encoder for one GetRequest and a
//! decoder for its GetResponse, no MIB machinery. A v2c request carries the
//! full set of OIDs (v2c answers unknown ones per varbind); a v1 request with
//! just `sysDescr` catches the oldest devices, which reject a v1 request as a
//! whole if any OID is unknown.

use crate::device::{Device, Protocol, Service};
use crate::discovery::usb::parse_device_id;
use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const PORT: u16 = 161;
const COMMUNITY: &[u8] = b"public";

/// OIDs asked for, with the attribute name each answer is stored under.
pub const OIDS: &[(&str, &str)] = &[
    ("sysDescr", "1.3.6.1.2.1.1.1.0"),
    ("sysObjectID", "1.3.6.1.2.1.1.2.0"),
    ("sysName", "1.3.6.1.2.1.1.5.0"),
    ("hrDeviceDescr", "1.3.6.1.2.1.25.3.2.1.3.1"),
    ("prtGeneralPrinterName", "1.3.6.1.2.1.43.5.1.1.16.1"),
    ("prtGeneralSerialNumber", "1.3.6.1.2.1.43.5.1.1.17.1"),
    (
        "ppmPrinterIEEE1284DeviceId",
        "1.3.6.1.4.1.2699.1.2.1.2.1.1.3.1",
    ),
    ("prtMarkerLifeCount", "1.3.6.1.2.1.43.10.2.1.4.1.1"),
    ("prtInterpreterLangFamily", "1.3.6.1.2.1.43.15.1.1.2.1.1"),
    ("hrPrinterStatus", "1.3.6.1.2.1.25.3.5.1.1.1"),
    ("hrDeviceStatus", "1.3.6.1.2.1.25.3.2.1.5.1"),
    ("hrDeviceType", "1.3.6.1.2.1.25.3.2.1.2.1"),
];

/// The subset sent in the v1 request: one OID every SNMP agent has.
const V1_OIDS: &[&str] = &["1.3.6.1.2.1.1.1.0"];

const HR_DEVICE_PRINTER: &str = "1.3.6.1.2.1.25.3.1.5";

/// Send a GET to every IPv4 broadcast address and collect the printers that answer.
pub fn probe(timeout: Duration) -> io::Result<Vec<Device>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_broadcast(true)?;
    let mut targets: Vec<Ipv4Addr> = vec![Ipv4Addr::BROADCAST];
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if let if_addrs::IfAddr::V4(v4) = iface.addr {
                if let Some(b) = v4.broadcast {
                    if !targets.contains(&b) {
                        targets.push(b);
                    }
                }
            }
        }
    }
    let v2c = encode_get(1, 0x4b41, &all_oids());
    let v1 = encode_get(0, 0x4b42, V1_OIDS);
    for t in &targets {
        // A segment without a broadcast route is not an error, just silence.
        let _ = socket.send_to(&v2c, SocketAddr::new(IpAddr::V4(*t), PORT));
        let _ = socket.send_to(&v1, SocketAddr::new(IpAddr::V4(*t), PORT));
    }
    Ok(collect(&socket, timeout)
        .into_iter()
        .filter_map(|(ip, answers)| device_from_answers(ip, &answers))
        .collect())
}

/// Ask the devices that were found another way but did not answer the
/// broadcast, so their serial number, exact model and status are known too.
/// Returns at once when there is nobody to ask.
pub fn enrich(devices: &mut [Device], timeout: Duration) -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let v2c = encode_get(1, 0x4b43, &all_oids());
    let mut asked = 0;
    for d in devices.iter() {
        if d.services.iter().any(|s| s.source == "snmp") {
            continue;
        }
        for ip in d.addresses.iter().filter(|a| a.is_ipv4()) {
            socket.send_to(&v2c, SocketAddr::new(*ip, PORT))?;
            asked += 1;
        }
    }
    if asked == 0 {
        return Ok(());
    }
    for (ip, answers) in collect(&socket, timeout) {
        let Some(sighting) = device_from_answers(ip, &answers) else {
            continue;
        };
        if let Some(d) = devices.iter_mut().find(|d| d.addresses.contains(&ip)) {
            d.merge(sighting);
        }
    }
    Ok(())
}

fn all_oids() -> Vec<&'static str> {
    OIDS.iter().map(|(_, oid)| *oid).collect()
}

/// Read replies until `timeout`, keyed by sender, folding v1 and v2c answers
/// from the same device together.
fn collect(socket: &UdpSocket, timeout: Duration) -> Vec<(IpAddr, BTreeMap<String, Value>)> {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let deadline = Instant::now() + timeout;
    let mut buf = vec![0u8; 65536];
    let mut by_sender: Vec<(IpAddr, BTreeMap<String, Value>)> = Vec::new();
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let Some(answers) = decode_response(&buf[..n]) else {
                    continue;
                };
                let ip = from.ip();
                match by_sender.iter_mut().find(|(a, _)| *a == ip) {
                    Some((_, existing)) => {
                        for (k, v) in answers {
                            existing.entry(k).or_insert(v);
                        }
                    }
                    None => by_sender.push((ip, answers)),
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    by_sender
}

/// Build a device from the answers; `None` for SNMP agents that are not printers.
pub fn device_from_answers(ip: IpAddr, answers: &BTreeMap<String, Value>) -> Option<Device> {
    let text = |name: &str| -> Option<String> {
        let oid = OIDS.iter().find(|(n, _)| *n == name)?.1;
        match answers.get(oid)? {
            Value::Text(s) => Some(s.clone()),
            Value::Oid(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Absent => None,
        }
    };
    let is_printer = text("hrDeviceType").as_deref() == Some(HR_DEVICE_PRINTER)
        || [
            "prtGeneralSerialNumber",
            "prtMarkerLifeCount",
            "prtInterpreterLangFamily",
            "hrPrinterStatus",
            "ppmPrinterIEEE1284DeviceId",
            "prtGeneralPrinterName",
        ]
        .iter()
        .any(|n| text(n).is_some());
    if !is_printer {
        return None;
    }

    let mut attributes = BTreeMap::new();
    for (name, _) in OIDS {
        if let Some(v) = text(name) {
            attributes.insert((*name).to_string(), v);
        }
    }
    let mut manufacturer = text("sysObjectID").and_then(|o| vendor_from_object_id(&o));
    let mut model = None;
    if let Some(id) = text("ppmPrinterIEEE1284DeviceId") {
        let fields = parse_device_id(&id);
        if let Some(m) = fields.get("MFG") {
            manufacturer = Some(m.clone());
        }
        if let Some(m) = fields.get("MDL") {
            model = Some(m.clone());
        }
        attributes.insert("device_id".to_string(), id);
        attributes.extend(fields);
    }
    let descr = text("hrDeviceDescr").filter(|s| !s.trim().is_empty());
    if model.is_none() {
        model = descr.clone().map(|d| match &manufacturer {
            Some(mf)
                if d.len() > mf.len()
                    && d.to_ascii_lowercase().starts_with(&mf.to_ascii_lowercase()) =>
            {
                d[mf.len()..].trim().to_string()
            }
            _ => d,
        });
    }
    if manufacturer.is_none() {
        manufacturer = descr
            .as_deref()
            .and_then(|d| d.split_whitespace().next())
            .map(str::to_string);
    }
    let name = descr
        .or_else(|| text("prtGeneralPrinterName"))
        .or_else(|| text("sysName"))
        .unwrap_or_default();
    if let Some(s) = text("prtGeneralSerialNumber") {
        attributes.insert("serial".to_string(), s);
    }
    if let Some(p) = text("prtMarkerLifeCount") {
        attributes.insert("pages_printed".to_string(), p);
    }

    Some(Device {
        name,
        manufacturer,
        model,
        hostname: None,
        addresses: vec![ip],
        uuid: None,
        usb: None,
        services: vec![Service {
            protocol: Protocol::Snmp,
            source: "snmp".into(),
            port: Some(PORT),
            endpoint: None,
            attributes,
        }],
    })
}

/// The vendor behind a `sysObjectID` (`1.3.6.1.4.1.<enterprise>...`).
pub fn vendor_from_object_id(oid: &str) -> Option<String> {
    let enterprise: u32 = oid
        .strip_prefix("1.3.6.1.4.1.")?
        .split('.')
        .next()?
        .parse()
        .ok()?;
    let name = match enterprise {
        11 => "HP",
        236 => "Samsung",
        253 => "Xerox",
        367 => "Ricoh",
        641 => "Lexmark",
        674 => "Dell",
        1248 => "Epson",
        1347 => "Kyocera",
        1602 => "Canon",
        2001 => "OKI",
        2385 => "Sharp",
        2435 => "Brother",
        18334 => "Konica Minolta",
        _ => return None,
    };
    Some(name.to_string())
}

/// A decoded varbind value, reduced to what the device model needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Oid(String),
    Number(i64),
    /// noSuchObject / noSuchInstance / endOfMibView / Null.
    Absent,
}

// ---- BER: just enough for an SNMP GET round trip ----

fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = body.len();
    if n < 128 {
        out.push(n as u8);
    } else {
        let len_bytes = n.to_be_bytes();
        let first = len_bytes.iter().position(|b| *b != 0).unwrap_or(7);
        out.push(0x80 | (len_bytes.len() - first) as u8);
        out.extend_from_slice(&len_bytes[first..]);
    }
    out.extend_from_slice(body);
    out
}

fn integer(v: i64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    // Shortest two's-complement form.
    let mut start = 0;
    while start < 7
        && ((bytes[start] == 0 && bytes[start + 1] & 0x80 == 0)
            || (bytes[start] == 0xff && bytes[start + 1] & 0x80 != 0))
    {
        start += 1;
    }
    tlv(0x02, &bytes[start..])
}

fn oid_bytes(oid: &str) -> Vec<u8> {
    let parts: Vec<u64> = oid.split('.').filter_map(|p| p.parse().ok()).collect();
    let mut out = Vec::new();
    if parts.len() >= 2 {
        out.push((40 * parts[0] + parts[1]) as u8);
    }
    for &p in parts.iter().skip(2) {
        let mut chunks = vec![(p & 0x7f) as u8];
        let mut rest = p >> 7;
        while rest > 0 {
            chunks.push(0x80 | (rest & 0x7f) as u8);
            rest >>= 7;
        }
        chunks.reverse();
        out.extend(chunks);
    }
    tlv(0x06, &out)
}

/// Encode a GetRequest (`version` 0 = v1, 1 = v2c) for `oids`.
pub fn encode_get(version: i64, request_id: i64, oids: &[&str]) -> Vec<u8> {
    let mut varbinds = Vec::new();
    for oid in oids {
        let mut vb = oid_bytes(oid);
        vb.extend(tlv(0x05, &[]));
        varbinds.extend(tlv(0x30, &vb));
    }
    let mut pdu = integer(request_id);
    pdu.extend(integer(0));
    pdu.extend(integer(0));
    pdu.extend(tlv(0x30, &varbinds));
    let mut msg = integer(version);
    msg.extend(tlv(0x04, COMMUNITY));
    msg.extend(tlv(0xa0, &pdu));
    tlv(0x30, &msg)
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    /// Read one TLV, returning its tag and body.
    fn tlv(&mut self) -> Option<(u8, &'a [u8])> {
        let tag = self.byte()?;
        let first = self.byte()?;
        let len = if first & 0x80 == 0 {
            first as usize
        } else {
            let count = (first & 0x7f) as usize;
            if count == 0 || count > 4 {
                return None;
            }
            let mut len = 0usize;
            for _ in 0..count {
                len = (len << 8) | self.byte()? as usize;
            }
            len
        };
        let body = self.data.get(self.pos..self.pos + len)?;
        self.pos += len;
        Some((tag, body))
    }

    fn done(&self) -> bool {
        self.pos >= self.data.len()
    }
}

fn decode_integer(body: &[u8]) -> Option<i64> {
    if body.is_empty() || body.len() > 8 {
        return None;
    }
    let mut v: i64 = if body[0] & 0x80 != 0 { -1 } else { 0 };
    for b in body {
        v = (v << 8) | *b as i64;
    }
    Some(v)
}

fn decode_unsigned(body: &[u8]) -> Option<i64> {
    if body.is_empty() || body.len() > 8 {
        return None;
    }
    let mut v: u64 = 0;
    for b in body {
        v = (v << 8) | *b as u64;
    }
    i64::try_from(v).ok()
}

fn decode_oid(body: &[u8]) -> Option<String> {
    let first = *body.first()?;
    let mut parts = vec![(first / 40) as u64, (first % 40) as u64];
    let mut acc: u64 = 0;
    for b in &body[1..] {
        acc = (acc << 7) | (*b & 0x7f) as u64;
        if b & 0x80 == 0 {
            parts.push(acc);
            acc = 0;
        }
    }
    Some(
        parts
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// Decode a GetResponse into `oid -> value`. `None` when the packet is not a
/// well-formed response or reports an error for the whole request.
pub fn decode_response(packet: &[u8]) -> Option<BTreeMap<String, Value>> {
    let mut outer = Reader {
        data: packet,
        pos: 0,
    };
    let (0x30, msg) = outer.tlv()? else {
        return None;
    };
    let mut msg = Reader { data: msg, pos: 0 };
    let (0x02, _version) = msg.tlv()? else {
        return None;
    };
    let (0x04, _community) = msg.tlv()? else {
        return None;
    };
    let (0xa2, pdu) = msg.tlv()? else {
        return None;
    };
    let mut pdu = Reader { data: pdu, pos: 0 };
    let (0x02, _request_id) = pdu.tlv()? else {
        return None;
    };
    let (0x02, error_status) = pdu.tlv()? else {
        return None;
    };
    if decode_integer(error_status)? != 0 {
        return None;
    }
    let (0x02, _error_index) = pdu.tlv()? else {
        return None;
    };
    let (0x30, varbinds) = pdu.tlv()? else {
        return None;
    };
    let mut varbinds = Reader {
        data: varbinds,
        pos: 0,
    };
    let mut out = BTreeMap::new();
    while !varbinds.done() {
        let (0x30, vb) = varbinds.tlv()? else {
            return None;
        };
        let mut vb = Reader { data: vb, pos: 0 };
        let (0x06, oid) = vb.tlv()? else {
            return None;
        };
        let (tag, body) = vb.tlv()?;
        let value = match tag {
            0x04 => Value::Text(String::from_utf8_lossy(body).trim().to_string()),
            0x06 => Value::Oid(decode_oid(body)?),
            0x02 => Value::Number(decode_integer(body)?),
            0x41..=0x43 => Value::Number(decode_unsigned(body)?),
            _ => Value::Absent,
        };
        out.insert(decode_oid(oid)?, value);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded from the Brother DCP-L2540DW at 198.51.100.167 on 2026-10-05;
    // see tests/fixtures/snmp/README.md.
    const V2C_REQUEST: &[u8] =
        include_bytes!("../../tests/fixtures/snmp/brother-dcp-l2540dw.v2c.request.bin");
    const V2C_RESPONSE: &[u8] =
        include_bytes!("../../tests/fixtures/snmp/brother-dcp-l2540dw.v2c.response.bin");
    const V1_RESPONSE: &[u8] =
        include_bytes!("../../tests/fixtures/snmp/brother-dcp-l2540dw.v1-rejected.response.bin");

    /// The OIDs the recording was made with, in its order (no hrDeviceType).
    const RECORDED_OIDS: &[&str] = &[
        "1.3.6.1.2.1.1.1.0",
        "1.3.6.1.2.1.1.2.0",
        "1.3.6.1.2.1.1.5.0",
        "1.3.6.1.2.1.25.3.2.1.3.1",
        "1.3.6.1.2.1.43.5.1.1.16.1",
        "1.3.6.1.2.1.43.5.1.1.17.1",
        "1.3.6.1.4.1.2699.1.2.1.2.1.1.3.1",
        "1.3.6.1.2.1.43.10.2.1.4.1.1",
        "1.3.6.1.2.1.43.15.1.1.2.1.1",
        "1.3.6.1.2.1.25.3.5.1.1.1",
        "1.3.6.1.2.1.25.3.2.1.5.1",
    ];

    #[test]
    fn encodes_the_recorded_request_byte_for_byte() {
        assert_eq!(encode_get(1, 0x4b41, RECORDED_OIDS), V2C_REQUEST);
    }

    #[test]
    fn long_form_lengths_and_integers_round_trip() {
        let body = vec![0u8; 300];
        let t = tlv(0x04, &body);
        assert_eq!(&t[..4], &[0x04, 0x82, 0x01, 0x2c]);
        let mut r = Reader { data: &t, pos: 0 };
        assert_eq!(r.tlv().unwrap().1.len(), 300);
        for v in [
            0,
            1,
            127,
            128,
            255,
            256,
            -1,
            -128,
            -129,
            0x4b41,
            i32::MAX as i64,
        ] {
            let enc = integer(v);
            let mut r = Reader { data: &enc, pos: 0 };
            let (0x02, body) = r.tlv().unwrap() else {
                panic!()
            };
            assert_eq!(decode_integer(body), Some(v), "{v}");
        }
        assert_eq!(integer(128), vec![0x02, 0x02, 0x00, 0x80]);
        assert_eq!(integer(-128), vec![0x02, 0x01, 0x80]);
    }

    #[test]
    fn oids_round_trip() {
        for oid in RECORDED_OIDS {
            let enc = oid_bytes(oid);
            assert_eq!(decode_oid(&enc[2..]).as_deref(), Some(*oid));
        }
        let enc = oid_bytes("1.3.6.1.4.1.18334.1");
        assert_eq!(
            decode_oid(&enc[2..]).as_deref(),
            Some("1.3.6.1.4.1.18334.1")
        );
    }

    #[test]
    fn decodes_the_brother_response() {
        let answers = decode_response(V2C_RESPONSE).expect("well-formed");
        assert_eq!(
            answers["1.3.6.1.2.1.1.1.0"],
            Value::Text("Brother NC-8300w, Firmware Ver.Z  ,MID 8C5-H27,FID 2".into())
        );
        assert_eq!(
            answers["1.3.6.1.2.1.1.2.0"],
            Value::Oid("1.3.6.1.4.1.2435.2.3.9.1".into())
        );
        assert_eq!(answers["1.3.6.1.2.1.43.5.1.1.16.1"], Value::Absent); // noSuchObject
        assert_eq!(answers["1.3.6.1.2.1.43.10.2.1.4.1.1"], Value::Number(302));
        assert_eq!(answers["1.3.6.1.2.1.25.3.5.1.1.1"], Value::Number(3)); // idle
        assert_eq!(answers.len(), RECORDED_OIDS.len());
    }

    #[test]
    fn rejected_v1_response_is_dropped() {
        // error-status 2 (noSuchName), error-index 5: no usable answers.
        assert_eq!(decode_response(V1_RESPONSE), None);
        assert_eq!(decode_response(&[]), None);
        assert_eq!(decode_response(&V2C_RESPONSE[..100]), None);
    }

    #[test]
    fn builds_the_brother_device() {
        let ip: IpAddr = "198.51.100.167".parse().unwrap();
        let answers = decode_response(V2C_RESPONSE).unwrap();
        let d = device_from_answers(ip, &answers).expect("a printer");
        assert_eq!(d.name, "Brother DCP-L2540DW series");
        assert_eq!(d.manufacturer.as_deref(), Some("Brother"));
        assert_eq!(d.model.as_deref(), Some("DCP-L2540DW series"));
        assert_eq!(d.addresses, vec![ip]);
        let attrs = &d.services[0].attributes;
        assert_eq!(d.services[0].protocol, Protocol::Snmp);
        assert_eq!(attrs["serial"], "KAGAZ0000000001");
        assert_eq!(attrs["pages_printed"], "302");
        assert_eq!(attrs["CMD"], "PJL,PCL,PCLXL,URF");
        assert_eq!(attrs["sysName"], "BRWEXAMPLE00001");
    }

    #[test]
    fn non_printers_are_dropped_and_vendor_falls_back_to_object_id() {
        let ip: IpAddr = "198.51.100.1".parse().unwrap();
        let mut answers = BTreeMap::new();
        answers.insert(
            "1.3.6.1.2.1.1.1.0".to_string(),
            Value::Text("Linux router".into()),
        );
        answers.insert(
            "1.3.6.1.2.1.1.2.0".to_string(),
            Value::Oid("1.3.6.1.4.1.8072.3.2.10".into()),
        );
        assert!(device_from_answers(ip, &answers).is_none());

        // A printer that only has the Host Resources MIB and an HP object id.
        let mut answers = BTreeMap::new();
        answers.insert(
            "1.3.6.1.2.1.1.2.0".to_string(),
            Value::Oid("1.3.6.1.4.1.11.2.3.9.1".into()),
        );
        answers.insert(
            "1.3.6.1.2.1.25.3.2.1.2.1".to_string(),
            Value::Oid(HR_DEVICE_PRINTER.into()),
        );
        answers.insert(
            "1.3.6.1.2.1.25.3.2.1.3.1".to_string(),
            Value::Text("HP LaserJet 1020".into()),
        );
        let d = device_from_answers(ip, &answers).expect("a printer");
        assert_eq!(d.manufacturer.as_deref(), Some("HP"));
        assert_eq!(d.model.as_deref(), Some("LaserJet 1020"));
        assert_eq!(d.name, "HP LaserJet 1020");
    }
}
