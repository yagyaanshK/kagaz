//! A direct mDNS query for one service instance's TXT record.
//!
//! mdns-sd only asks for the TXT record while the SRV record is still
//! missing; a device that sends its (large) TXT record late, as some Brother
//! printers do, is reported resolved with no TXT data and never queried
//! again. This sends one unicast-response ("QU") TXT question for the
//! instance and parses the answer, so the details still arrive.

use std::collections::BTreeMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

const MDNS_GROUP: &str = "224.0.0.251:5353";
const TYPE_TXT: u16 = 16;

/// Query the TXT record of `fullname` (e.g. "My Printer._ipp._tcp.local.")
/// and return its key/value pairs. Empty map if nothing answered in time.
pub fn fetch_txt(fullname: &str, timeout: Duration) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return out;
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
    let query = build_query(fullname);
    if socket.send_to(&query, MDNS_GROUP).is_err() {
        return out;
    }
    let deadline = Instant::now() + timeout;
    let mut buf = vec![0u8; 9000];
    while Instant::now() < deadline {
        let Ok((n, _)) = socket.recv_from(&mut buf) else {
            continue;
        };
        if let Some(txt) = parse_txt_answer(&buf[..n], fullname) {
            for entry in txt {
                match entry.split_once('=') {
                    Some((k, v)) => out.insert(k.to_string(), v.to_string()),
                    None => out.insert(entry, String::new()),
                };
            }
            if !out.is_empty() {
                break;
            }
        }
    }
    out
}

fn encode_name(name: &str, buf: &mut Vec<u8>) {
    for label in name.trim_end_matches('.').split('.') {
        let bytes = label.as_bytes();
        buf.push(bytes.len().min(63) as u8);
        buf.extend_from_slice(&bytes[..bytes.len().min(63)]);
    }
    buf.push(0);
}

fn build_query(fullname: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(&[0x4b, 0x47, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]); // id, flags, 1 question
    encode_name(fullname, &mut q);
    q.extend_from_slice(&TYPE_TXT.to_be_bytes());
    q.extend_from_slice(&(0x8000u16 | 1).to_be_bytes()); // class IN, QU bit: answer by unicast
    q
}

/// Read a (possibly compressed) DNS name at `pos`; returns (name, next position).
fn read_name(pkt: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut next = None;
    let mut hops = 0;
    loop {
        let len = *pkt.get(pos)? as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            let ptr = ((len & 0x3F) << 8) | *pkt.get(pos + 1)? as usize;
            if next.is_none() {
                next = Some(pos + 2);
            }
            pos = ptr;
            hops += 1;
            if hops > 16 {
                return None;
            }
            continue;
        }
        labels.push(String::from_utf8_lossy(pkt.get(pos + 1..pos + 1 + len)?).into_owned());
        pos += 1 + len;
    }
    Some((labels.join("."), next.unwrap_or(pos)))
}

fn parse_txt_answer(pkt: &[u8], fullname: &str) -> Option<Vec<String>> {
    if pkt.len() < 12 {
        return None;
    }
    let count = |i: usize| u16::from_be_bytes([pkt[i], pkt[i + 1]]) as usize;
    let (qd, an, ns, ar) = (count(4), count(6), count(8), count(10));
    let wanted = fullname.trim_end_matches('.');
    let mut pos = 12;
    for _ in 0..qd {
        let (_, next) = read_name(pkt, pos)?;
        pos = next + 4;
    }
    for _ in 0..(an + ns + ar) {
        let (name, next) = read_name(pkt, pos)?;
        let rtype = u16::from_be_bytes([*pkt.get(next)?, *pkt.get(next + 1)?]);
        let rdlen = u16::from_be_bytes([*pkt.get(next + 8)?, *pkt.get(next + 9)?]) as usize;
        let data = pkt.get(next + 10..next + 10 + rdlen)?;
        pos = next + 10 + rdlen;
        if rtype == TYPE_TXT && name.eq_ignore_ascii_case(wanted) {
            let mut entries = Vec::new();
            let mut i = 0;
            while i < data.len() {
                let len = data[i] as usize;
                let s = data.get(i + 1..i + 1 + len)?;
                if len > 0 {
                    entries.push(String::from_utf8_lossy(s).into_owned());
                }
                i += 1 + len;
            }
            return Some(entries);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_txt_answer_with_compressed_name() {
        // One question, one TXT answer whose name is a pointer to the question name.
        let mut pkt = vec![0, 0, 0x84, 0, 0, 1, 0, 1, 0, 0, 0, 0];
        encode_name("P._ipp._tcp.local.", &mut pkt);
        pkt.extend_from_slice(&[0, 16, 0, 1]);
        pkt.extend_from_slice(&[0xC0, 12]); // pointer to offset 12
        pkt.extend_from_slice(&[0, 16, 0x80, 1, 0, 0, 0, 10]);
        let rdata = b"\x05pdl=x\x02ty";
        pkt.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        pkt.extend_from_slice(rdata);
        let txt = parse_txt_answer(&pkt, "P._ipp._tcp.local.").unwrap();
        assert_eq!(txt, vec!["pdl=x".to_string(), "ty".to_string()]);
    }
}
