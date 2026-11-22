//! Reaching a device: every path it has, tried at once, each checked for
//! the thing it should speak (a connection that opens is not enough: the
//! address may now belong to something else). Also: which ways in a host
//! offers, and a look over the local network for a device whose address
//! changed.
//!
//! Nothing here signs in. What a device shows before sign-in is enough to
//! know it: a Remote Desktop computer presents its certificate, a VNC
//! server its protocol version, an SSH server its banner.

use super::store::{Device, Identity, PathKind};
use super::{Speaks, Way};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What a host showed of itself on a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub speaks: Speaks,
    /// The server's own first words ("RFB 003.008", "SSH-2.0-OpenSSH_9.6").
    pub banner: String,
    pub identity: Identity,
}

/// Why a path did not lead to the device.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Trouble {
    #[error("the name is not known on this network")]
    NoAddress,
    #[error("this device has no gateway now (it is not on a hotspot)")]
    NoGateway,
    #[error("no answer")]
    NoAnswer,
    #[error("the device is there, but nothing listens on that port")]
    Refused,
    #[error("something else answers there")]
    SomethingElse,
    #[error("another device answers there: {0}")]
    AnotherDevice(String),
    /// From afar (remote/far.rs), in its own words.
    #[error("{0}")]
    Far(String),
}

/// A path that answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Which of the device's paths (its place in `Device::paths`).
    pub path: usize,
    pub way: Way,
    pub addr: SocketAddr,
    /// How long the connection took to open.
    pub millis: u32,
    pub seen: Seen,
}

/// How one path did, for one way in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathReport {
    pub path: usize,
    pub way: Way,
    pub outcome: Result<Answer, Trouble>,
}

/// Addresses for a host the person's own network knows. Only the system is
/// asked: a private name is not something to send to a public resolver.
fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, Trouble> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    match (host, port).to_socket_addrs() {
        Ok(found) => {
            let mut all: Vec<SocketAddr> = found.collect();
            // IPv4 first: home networks answer on it more surely.
            all.sort_by_key(|a| a.is_ipv6());
            if all.is_empty() {
                Err(Trouble::NoAddress)
            } else {
                Ok(all)
            }
        }
        Err(_) => Err(Trouble::NoAddress),
    }
}

/// The gateways this device has now, one per network it is on (Linux; the
/// other systems come with their viewers).
pub fn gateways() -> Vec<IpAddr> {
    let mut out = Vec::new();
    if let Ok(text) = std::fs::read_to_string("/proc/net/route") {
        out.extend(parse_routes(&text));
    }
    out
}

/// Default routes' gateways from Linux's `/proc/net/route`.
fn parse_routes(text: &str) -> Vec<IpAddr> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 || f[1] != "00000000" {
            continue;
        }
        if let Ok(raw) = u32::from_str_radix(f[2], 16) {
            let ip = IpAddr::V4(Ipv4Addr::from(raw.swap_bytes()));
            if raw != 0 && !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

/// Open a connection to `addr` and see what speaks there.
pub fn look(addr: SocketAddr, speaks: Speaks, timeout: Duration) -> Result<(u32, Seen), Trouble> {
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| match e.kind() {
        std::io::ErrorKind::ConnectionRefused => Trouble::Refused,
        _ => Trouble::NoAnswer,
    })?;
    let millis = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let _ = stream.set_nodelay(true);
    let seen = match speaks {
        Speaks::Rdp => see_rdp(&mut stream, addr),
        Speaks::Vnc => see_banner(&mut stream, "RFB ", Speaks::Vnc),
        Speaks::Ssh => see_banner(&mut stream, "SSH-", Speaks::Ssh),
    }?;
    Ok((millis, seen))
}

/// VNC and SSH servers speak first: one line that starts with `start`.
fn see_banner(stream: &mut TcpStream, start: &str, speaks: Speaks) -> Result<Seen, Trouble> {
    let mut buf = [0u8; 64];
    let mut got = 0;
    while got < buf.len() {
        match stream.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
        if buf[..got].contains(&b'\n') {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf[..got]);
    let line = text.lines().next().unwrap_or("").trim();
    if got == 0 {
        return Err(Trouble::NoAnswer);
    }
    if !line.starts_with(start) {
        return Err(Trouble::SomethingElse);
    }
    Ok(Seen {
        speaks,
        banner: line.to_string(),
        identity: Identity::default(),
    })
}

/// Security protocols in an RDP negotiation ([MS-RDPBCGR] 2.2.1.1.1).
const PROTOCOL_SSL: u32 = 1;
const PROTOCOL_HYBRID: u32 = 2;

/// An RDP server answers a connection request with a connection confirm;
/// when it agrees to TLS, its certificate says which computer it is.
fn see_rdp(stream: &mut TcpStream, addr: SocketAddr) -> Result<Seen, Trouble> {
    // TPKT, X.224 Connection Request, then the negotiation request asking
    // for TLS or TLS with network-level sign-in, as every modern client.
    let mut request = vec![0x03, 0x00, 0x00, 0x13, 0x0e, 0xe0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00];
    request.extend_from_slice(&(PROTOCOL_SSL | PROTOCOL_HYBRID).to_le_bytes());
    stream.write_all(&request).map_err(|_| Trouble::NoAnswer)?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).map_err(|_| Trouble::NoAnswer)?;
    let len = u16::from_be_bytes([head[2], head[3]]) as usize;
    if head[0] != 0x03 || !(7..=512).contains(&len) {
        return Err(Trouble::SomethingElse);
    }
    let mut body = vec![0u8; len - 4];
    stream.read_exact(&mut body).map_err(|_| Trouble::SomethingElse)?;
    // X.224 Connection Confirm.
    if body.len() < 7 || body[1] & 0xf0 != 0xd0 {
        return Err(Trouble::SomethingElse);
    }
    let mut seen = Seen {
        speaks: Speaks::Rdp,
        banner: "Remote Desktop".to_string(),
        identity: Identity::default(),
    };
    // A negotiation response (type 2) names the protocol it chose.
    let chose_tls = body.len() >= 15
        && body[7] == 0x02
        && u32::from_le_bytes([body[11], body[12], body[13], body[14]]) & (PROTOCOL_SSL | PROTOCOL_HYBRID) != 0;
    if chose_tls {
        if let Some(der) = certificate(stream, addr) {
            seen.identity = identity_of(&der);
        }
    }
    Ok(seen)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A TLS setup that takes whatever certificate is shown, its signature
/// still checked. Remote Desktop servers sign their own certificates, so
/// trust comes from remembering the certificate, not from an authority.
pub(crate) fn any_certificate_tls() -> Option<Arc<rustls::ClientConfig>> {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};
    #[derive(Debug)]
    struct Shown(Arc<rustls::crypto::CryptoProvider>);
    impl ServerCertVerifier for Shown {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
        }
        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .ok()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Shown(provider)))
        .with_no_client_auth();
    Some(Arc::new(config))
}

/// Who a certificate says the device is: its name and its fingerprint.
pub(crate) fn identity_of(der: &[u8]) -> Identity {
    Identity {
        name: common_name(der),
        certificate: Some(hex(&Sha256::digest(der))),
    }
}

/// Shake hands over TLS just far enough to be shown the certificate. Any
/// certificate is taken: this is how the device introduces itself, and
/// what Kagaz remembers and compares is the certificate itself.
fn certificate(stream: &mut TcpStream, addr: SocketAddr) -> Option<Vec<u8>> {
    use rustls::pki_types::ServerName;
    let config = any_certificate_tls()?;
    let name = ServerName::IpAddress(addr.ip().into());
    let mut conn = rustls::ClientConnection::new(config, name).ok()?;
    while conn.is_handshaking() {
        if conn.peer_certificates().is_some() {
            break;
        }
        conn.complete_io(stream).ok()?;
    }
    conn.peer_certificates()?.first().map(|c| c.as_ref().to_vec())
}

/// The last common name in a certificate (the subject's; the issuer's
/// comes first and is the same in a self-signed one). A plain scan for the
/// attribute, which saves an X.509 parser for one string.
fn common_name(der: &[u8]) -> Option<String> {
    const CN: [u8; 5] = [0x06, 0x03, 0x55, 0x04, 0x03];
    let mut found = None;
    let mut at = 0;
    while let Some(i) = der[at..].windows(CN.len()).position(|w| w == CN) {
        let value = at + i + CN.len();
        at = value;
        let (Some(&tag), Some(&len)) = (der.get(value), der.get(value + 1)) else {
            break;
        };
        // A name is short: one length byte.
        if len >= 0x80 {
            continue;
        }
        let len = len as usize;
        let Some(bytes) = der.get(value + 2..value + 2 + len) else {
            continue;
        };
        let text = match tag {
            // UTF8String, PrintableString, IA5String, T61String.
            0x0c | 0x13 | 0x16 | 0x14 => String::from_utf8_lossy(bytes).to_string(),
            // BMPString: two bytes a character.
            0x1e => String::from_utf16_lossy(&bytes.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect::<Vec<_>>()),
            _ => continue,
        };
        if !text.is_empty() {
            found = Some(text);
        }
    }
    found
}

/// One path, one way in: resolve, connect, and see that it is the device.
fn try_path(device: &Device, path: usize, way: Way, timeout: Duration) -> Result<Answer, Trouble> {
    let p = &device.paths[path];
    let addrs = match p.kind {
        PathKind::Gateway => {
            let all: Vec<SocketAddr> = gateways().into_iter().map(|ip| SocketAddr::new(ip, way.port)).collect();
            if all.is_empty() {
                return Err(Trouble::NoGateway);
            }
            all
        }
        PathKind::Far => return Err(Trouble::NoAddress),
        _ => resolve(&p.host, way.port)?,
    };
    let mut last = Trouble::NoAnswer;
    for addr in addrs {
        match look(addr, way.speaks, timeout) {
            Ok((millis, seen)) => {
                // Not the device remembered there: the router gave its
                // address to something else.
                if !device.identity.is_empty() && !seen.identity.is_empty() && !device.identity.matches(&seen.identity) {
                    last = Trouble::AnotherDevice(seen.identity.name.clone().unwrap_or_else(|| addr.ip().to_string()));
                    continue;
                }
                return Ok(Answer {
                    path,
                    way,
                    addr,
                    millis,
                    seen,
                });
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Start every path of `device` for each of `ways` at once; reports arrive
/// as each finishes.
fn start(device: &Device, ways: &[Way], timeout: Duration) -> (mpsc::Receiver<PathReport>, usize) {
    let (tx, rx) = mpsc::channel();
    let device = Arc::new(device.clone());
    let mut count = 0;
    // From afar is tried on its own, when none of these answers.
    for path in (0..device.paths.len()).filter(|p| device.paths[*p].kind != PathKind::Far) {
        for way in ways {
            let (tx, device, way) = (tx.clone(), device.clone(), *way);
            count += 1;
            std::thread::spawn(move || {
                let outcome = try_path(&device, path, way, timeout);
                let _ = tx.send(PathReport { path, way, outcome });
            });
        }
    }
    (rx, count)
}

/// How every path of the device does now, for every way in it has.
pub fn check(device: &Device, timeout: Duration) -> Vec<PathReport> {
    let (rx, count) = start(device, &device.ways, timeout);
    let mut reports: Vec<PathReport> = rx.iter().take(count).collect();
    reports.sort_by_key(|r| (r.path, r.way.port));
    reports
}

/// Reach the device for `speaks`: every path at once, the first to answer
/// wins and the rest are left to finish by themselves. When none answers,
/// every path's trouble comes back.
pub fn reach(device: &Device, speaks: Speaks, timeout: Duration) -> Result<Answer, Vec<PathReport>> {
    let Some(way) = device.way(speaks) else {
        return Err(Vec::new());
    };
    let (rx, count) = start(device, &[way], timeout);
    let mut failed = Vec::new();
    for report in rx.iter().take(count) {
        match report.outcome {
            Ok(answer) => return Ok(answer),
            Err(_) => failed.push(report),
        }
    }
    failed.sort_by_key(|r| r.path);
    Err(failed)
}

/// Which ways in a host offers, on the usual ports, asked all at once.
pub fn detect(host: &str, timeout: Duration) -> Vec<(Way, SocketAddr, Seen)> {
    let (tx, rx) = mpsc::channel();
    for speaks in Speaks::ALL {
        let (tx, host) = (tx.clone(), host.to_string());
        std::thread::spawn(move || {
            let way = Way::new(speaks);
            let found = resolve(&host, way.port)
                .ok()
                .and_then(|addrs| addrs.into_iter().find_map(|a| look(a, speaks, timeout).ok().map(|(_, seen)| (way, a, seen))));
            let _ = tx.send(found);
        });
    }
    drop(tx);
    let mut out: Vec<(Way, SocketAddr, Seen)> = rx.iter().flatten().collect();
    out.sort_by_key(|(w, _, _)| w.port != 3389);
    out
}

/// A device seen on the local network that shows its screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub ip: IpAddr,
    pub way: Way,
    pub seen: Seen,
}

/// The local IPv4 networks this device is on, as (address, prefix length),
/// small enough to look over (a home network; not a /16).
fn local_networks() -> Vec<(Ipv4Addr, u8)> {
    let mut out = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = iface.addr {
                let prefix = u32::from(v4.netmask).count_ones() as u8;
                if (22..=30).contains(&prefix) && !out.iter().any(|(ip, _)| *ip == v4.ip) {
                    out.push((v4.ip, prefix));
                }
            }
        }
    }
    out
}

/// Is one of the device's local addresses on a network this device is on?
pub fn on_same_network(device: &Device) -> bool {
    let nets = local_networks();
    device.paths.iter().any(|p| match (p.kind, p.host.parse::<Ipv4Addr>()) {
        (PathKind::Address, Ok(ip)) => nets.iter().any(|(mine, prefix)| {
            let mask = u32::MAX << (32 - *prefix as u32);
            u32::from(ip) & mask == u32::from(*mine) & mask
        }),
        // A name says nothing of where it is; being on any network counts.
        (PathKind::Name, _) => !nets.is_empty(),
        _ => false,
    })
}

/// Every other address of the network `ip`/`prefix`.
fn hosts(ip: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    let mask = u32::MAX << (32 - prefix as u32);
    let net = u32::from(ip) & mask;
    (1..(!mask))
        .map(|i| Ipv4Addr::from(net | i))
        .filter(|h| *h != ip)
        .collect()
}

/// Look over the local networks for devices that show their screen
/// (Remote Desktop, VNC): the way a saved device is found again after the
/// router gave it a new address, and what "Found nearby" lists. Only this
/// device's own networks, one connection attempt per address and port.
pub fn search(timeout: Duration) -> Vec<Found> {
    let mut targets: Vec<(Ipv4Addr, Speaks)> = Vec::new();
    for (ip, prefix) in local_networks() {
        for host in hosts(ip, prefix) {
            for speaks in [Speaks::Rdp, Speaks::Vnc] {
                targets.push((host, speaks));
            }
        }
    }
    // A few dozen at a time: an empty address costs the whole timeout.
    let targets = Arc::new(std::sync::Mutex::new(targets));
    let (tx, rx) = mpsc::channel();
    let workers = 96;
    for _ in 0..workers {
        let (targets, tx) = (targets.clone(), tx.clone());
        std::thread::spawn(move || loop {
            let next = targets.lock().ok().and_then(|mut t| t.pop());
            let Some((host, speaks)) = next else { break };
            let way = Way::new(speaks);
            let addr = SocketAddr::new(IpAddr::V4(host), way.port);
            // The quick knock first; the longer look only where a port is open.
            if TcpStream::connect_timeout(&addr, timeout).is_err() {
                continue;
            }
            if let Ok((_, seen)) = look(addr, speaks, timeout.max(Duration::from_millis(1500))) {
                let _ = tx.send(Found {
                    ip: IpAddr::V4(host),
                    way,
                    seen,
                });
            }
        });
    }
    drop(tx);
    let mut found: Vec<Found> = rx.iter().collect();
    found.sort_by_key(|f| (f.ip, f.way.port));
    found
}

#[cfg(test)]
mod tests {
    use super::super::store::PathTo;
    use super::*;
    use std::net::TcpListener;

    /// A server on a free local port that says `greeting` to whoever
    /// connects (after reading `expect` bytes first, when asked).
    fn server(greeting: &'static [u8], expect: usize) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = vec![0u8; expect];
                if expect > 0 && stream.read_exact(&mut buf).is_err() {
                    continue;
                }
                let _ = stream.write_all(greeting);
            }
        });
        port
    }

    const T: Duration = Duration::from_millis(800);

    fn local(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn a_vnc_server_is_known_by_its_first_line() {
        let port = server(b"RFB 003.008\n", 0);
        let (_, seen) = look(local(port), Speaks::Vnc, T).unwrap();
        assert_eq!(seen.banner, "RFB 003.008");
        // The same port is not SSH.
        assert_eq!(look(local(port), Speaks::Ssh, T), Err(Trouble::SomethingElse));
    }

    #[test]
    fn an_ssh_server_is_known_by_its_banner() {
        let port = server(b"SSH-2.0-OpenSSH_9.6\r\n", 0);
        let (_, seen) = look(local(port), Speaks::Ssh, T).unwrap();
        assert_eq!(seen.banner, "SSH-2.0-OpenSSH_9.6");
    }

    #[test]
    fn a_remote_desktop_server_is_known_by_its_connection_confirm() {
        // Connection Confirm with a negotiation failure (no TLS offered
        // here): still Remote Desktop, with nothing to identify it by.
        let confirm: &'static [u8] = &[
            0x03, 0x00, 0x00, 0x13, 0x0e, 0xd0, 0x00, 0x00, 0x12, 0x34, 0x00, 0x03, 0x00, 0x08, 0x00, 0x02, 0x00, 0x00, 0x00,
        ];
        let port = server(confirm, 19);
        let (_, seen) = look(local(port), Speaks::Rdp, T).unwrap();
        assert_eq!(seen.speaks, Speaks::Rdp);
        assert!(seen.identity.is_empty());
        // A web server on that port is something else.
        let web = server(b"HTTP/1.1 400 Bad Request\r\n\r\n", 19);
        assert_eq!(look(local(web), Speaks::Rdp, T), Err(Trouble::SomethingElse));
    }

    #[test]
    fn a_closed_port_is_refused() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert_eq!(look(local(port), Speaks::Vnc, T), Err(Trouble::Refused));
    }

    #[test]
    fn the_first_path_that_answers_is_used() {
        let port = server(b"RFB 003.008\n", 0);
        let device = Device {
            id: "d".into(),
            name: "Test".into(),
            ways: vec![Way { speaks: Speaks::Vnc, port }],
            // 192.0.2.0/24 is for documentation: nothing answers there.
            paths: vec![PathTo::new("192.0.2.1"), PathTo::new("no-such-host.invalid"), PathTo::new("127.0.0.1")],
            ..Default::default()
        };
        let answer = reach(&device, Speaks::Vnc, T).unwrap();
        assert_eq!(answer.path, 2);
        let reports = check(&device, T);
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[1].outcome, Err(Trouble::NoAddress));
        assert!(reports[2].outcome.is_ok());
        // No way in for Remote Desktop on this device at all.
        assert_eq!(reach(&device, Speaks::Rdp, T), Err(Vec::new()));
    }

    #[test]
    fn default_gateways_are_read_from_the_route_table() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     wlan0\t00000000\t010200C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                     wlan0\t000200C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n";
        assert_eq!(parse_routes(table), vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))]);
    }

    #[test]
    fn a_network_lists_its_other_addresses() {
        let all = hosts(Ipv4Addr::new(192, 0, 2, 20), 24);
        assert_eq!(all.len(), 253);
        assert!(!all.contains(&Ipv4Addr::new(192, 0, 2, 20)));
        assert!(!all.contains(&Ipv4Addr::new(192, 0, 2, 0)));
        assert!(!all.contains(&Ipv4Addr::new(192, 0, 2, 255)));
        assert_eq!(hosts(Ipv4Addr::new(192, 0, 2, 1), 30).len(), 1);
    }

    #[test]
    fn the_subject_name_is_read_from_a_certificate() {
        // Issuer CN "Root", subject CN "STUDY-LAPTOP", as the attribute
        // bytes appear inside a certificate.
        let mut der = vec![0x30, 0x10, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x04];
        der.extend_from_slice(b"Root");
        der.extend_from_slice(&[0x31, 0x13, 0x06, 0x03, 0x55, 0x04, 0x03, 0x13, 0x0c]);
        der.extend_from_slice(b"STUDY-LAPTOP");
        assert_eq!(common_name(&der).as_deref(), Some("STUDY-LAPTOP"));
        assert_eq!(common_name(&[0x30, 0x00]), None);
    }
}
