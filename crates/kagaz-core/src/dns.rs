//! Name lookup with a public fallback. Some home routers and ISPs fail to
//! resolve particular domains (a SERVFAIL for one vendor's zone while the
//! rest of the web works). Kagaz first asks the system, as every program
//! does; only when that fails does it ask Cloudflare (1.1.1.1), then Google
//! (8.8.8.8), directly over plain DNS. Used for the OneTouch cloud, whose
//! lookups decide whether the doorbell can ring.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

const PUBLIC: [&str; 2] = ["1.1.1.1:53", "8.8.8.8:53"];

/// Addresses for `host:port`: the system's answer, else a public resolver's.
pub fn resolve(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let system = (host, port)
        .to_socket_addrs()
        .map(|a| a.collect::<Vec<_>>());
    if let Ok(found) = &system {
        if !found.is_empty() {
            return Ok(found.clone());
        }
    }
    for server in PUBLIC {
        if let Ok(ips) = query_a(host, server, Duration::from_secs(3)) {
            if !ips.is_empty() {
                return Ok(ips
                    .into_iter()
                    .map(|ip| SocketAddr::new(IpAddr::V4(ip), port))
                    .collect());
            }
        }
    }
    match system {
        Err(e) => Err(e),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no address for {host}"),
        )),
    }
}

/// A `ureq` resolver using [`resolve`]; ureq hands it `host:port`.
pub fn ureq_resolver(netloc: &str) -> io::Result<Vec<SocketAddr>> {
    let (host, port) = netloc
        .rsplit_once(':')
        .and_then(|(h, p)| Some((h.trim_matches(['[', ']']), p.parse().ok()?)))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no port"))?;
    resolve(host, port)
}

/// One A-record question to `server`, answered over UDP.
fn query_a(host: &str, server: &str, wait: Duration) -> io::Result<Vec<Ipv4Addr>> {
    let id = (uuid::Uuid::new_v4().as_u128() & 0xFFFF) as u16;
    let packet = question(id, host)?;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(wait))?;
    sock.send_to(&packet, server)?;
    let mut buf = [0u8; 1500];
    let n = sock.recv(&mut buf)?;
    parse_answer(id, &buf[..n])
}

fn question(id: u16, host: &str) -> io::Result<Vec<u8>> {
    let mut p = Vec::with_capacity(32 + host.len());
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // recursion desired, 1 question
    for label in host.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad host name"));
        }
        p.push(label.len() as u8);
        p.extend_from_slice(label.as_bytes());
    }
    p.push(0);
    p.extend_from_slice(&[0, 1, 0, 1]); // type A, class IN
    Ok(p)
}

/// Skip a (possibly compressed) name; returns the offset after it.
fn skip_name(d: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = *d.get(at)? as usize;
        if len == 0 {
            return Some(at + 1);
        }
        if len & 0xC0 == 0xC0 {
            return Some(at + 2);
        }
        at += 1 + len;
    }
}

fn parse_answer(id: u16, d: &[u8]) -> io::Result<Vec<Ipv4Addr>> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "malformed DNS answer");
    if d.len() < 12 || u16::from_be_bytes([d[0], d[1]]) != id || d[2] & 0x80 == 0 {
        return Err(bad());
    }
    if d[3] & 0x0F != 0 {
        return Ok(Vec::new()); // NXDOMAIN, SERVFAIL...
    }
    let questions = u16::from_be_bytes([d[4], d[5]]);
    let answers = u16::from_be_bytes([d[6], d[7]]);
    let mut at = 12;
    for _ in 0..questions {
        at = skip_name(d, at).ok_or_else(bad)? + 4;
    }
    let mut out = Vec::new();
    for _ in 0..answers {
        at = skip_name(d, at).ok_or_else(bad)?;
        let head = d.get(at..at + 10).ok_or_else(bad)?;
        let kind = u16::from_be_bytes([head[0], head[1]]);
        let len = u16::from_be_bytes([head[8], head[9]]) as usize;
        let data = d.get(at + 10..at + 10 + len).ok_or_else(bad)?;
        if kind == 1 && len == 4 {
            out.push(Ipv4Addr::new(data[0], data[1], data[2], data[3]));
        }
        at += 10 + len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_question() {
        let q = question(0x1234, "rps.example.com").unwrap();
        assert_eq!(&q[..4], &[0x12, 0x34, 0x01, 0x00]);
        assert_eq!(&q[12..16], &[3, b'r', b'p', b's']);
        assert!(q.ends_with(&[0, 0, 1, 0, 1]));
        assert!(question(1, "a..b").is_err());
    }

    #[test]
    fn reads_a_records_through_a_cname() {
        // Answer: www.example.com CNAME example.com, example.com A 192.0.2.7.
        let mut d = vec![0xAB, 0xCD, 0x81, 0x80, 0, 1, 0, 2, 0, 0, 0, 0];
        d.extend_from_slice(&question(0xABCD, "www.example.com").unwrap()[12..]);
        // CNAME, name as a pointer to the question (offset 12).
        d.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 2, 0xC0, 16]);
        // A record for the pointed-to name.
        d.extend_from_slice(&[0xC0, 16, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 192, 0, 2, 7]);
        assert_eq!(
            parse_answer(0xABCD, &d).unwrap(),
            vec![Ipv4Addr::new(192, 0, 2, 7)]
        );
        // A different id is not our answer; SERVFAIL gives nothing.
        assert!(parse_answer(1, &d).is_err());
        d[3] = 0x82;
        assert!(parse_answer(0xABCD, &d).unwrap().is_empty());
    }

    #[test]
    fn literal_addresses_need_no_lookup() {
        let a = resolve("192.0.2.9", 5070).unwrap();
        assert_eq!(a[0].to_string(), "192.0.2.9:5070");
        assert_eq!(ureq_resolver("192.0.2.9:443").unwrap()[0].port(), 443);
    }

    /// Needs the internet: `cargo test -p kagaz-core dns -- --ignored`.
    #[test]
    #[ignore]
    fn public_resolvers_answer() {
        for server in PUBLIC {
            let ips = query_a("example.com", server, Duration::from_secs(3)).unwrap();
            assert!(!ips.is_empty(), "{server} gave no address");
        }
    }
}
