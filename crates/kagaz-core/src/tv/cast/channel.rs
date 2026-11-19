//! The Cast channel to a TV: TLS to port 8009, each message its length and
//! a small record (who from, who to, a namespace, and JSON as text). The
//! TV shows its own certificate, made for itself; a sender checks it only
//! to refuse TVs Google did not certify, which Kagaz has no need to do.
//!
//! Nothing here waits: the session asks for what has come between
//! pictures.

use serde_json::{json, Value};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

pub const PORT: u16 = 8009;
const CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
pub const RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
/// Cast Streaming's own messages (the offer and the answer).
pub const STREAMING: &str = "urn:x-cast:com.google.cast.webrtc";
/// The TV itself, as against an app running on it.
pub const TV: &str = "receiver-0";
const ME: &str = "sender-kagaz";
/// A message longer than this is not one.
const LONGEST: usize = 1 << 20;

/// What the TV, or an app on it, said.
#[derive(Debug, Clone)]
pub struct Said {
    pub from: String,
    pub namespace: String,
    pub json: Value,
}

impl Said {
    pub fn kind(&self) -> &str {
        self.json["type"].as_str().unwrap_or("")
    }
}

pub struct Channel {
    tcp: TcpStream,
    tls: rustls::ClientConnection,
    inbox: Vec<u8>,
    last_ping: Instant,
    last_heard: Instant,
}

fn varint(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        out.push(if n == 0 { b } else { b | 0x80 });
        if n == 0 {
            return;
        }
    }
}

fn text(field: u8, s: &str, out: &mut Vec<u8>) {
    out.push(field << 3 | 2);
    varint(s.len(), out);
    out.extend(s.as_bytes());
}

/// One message as it travels: the length, then the record.
pub fn record(from: &str, to: &str, namespace: &str, json: &Value) -> Vec<u8> {
    let mut body = vec![0x08, 0x00]; // protocol version 2.1.0
    text(2, from, &mut body);
    text(3, to, &mut body);
    text(4, namespace, &mut body);
    body.extend([0x28, 0x00]); // the payload is text
    text(6, &json.to_string(), &mut body);
    let mut out = (body.len() as u32).to_be_bytes().to_vec();
    out.extend(body);
    out
}

/// A record's text fields by number: 2 from, 3 to, 4 namespace, 6 JSON.
pub fn fields(mut b: &[u8]) -> Option<[String; 7]> {
    fn number(b: &mut &[u8]) -> Option<usize> {
        let (mut n, mut shift) = (0usize, 0);
        loop {
            let (&byte, rest) = b.split_first()?;
            *b = rest;
            n |= ((byte & 0x7f) as usize) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                return Some(n);
            }
            if shift > 35 {
                return None;
            }
        }
    }
    let mut out: [String; 7] = Default::default();
    while !b.is_empty() {
        let key = number(&mut b)?;
        match key & 7 {
            0 => {
                number(&mut b)?;
            }
            2 => {
                let n = number(&mut b)?;
                if b.len() < n {
                    return None;
                }
                if key >> 3 < 7 {
                    out[key >> 3] = String::from_utf8_lossy(&b[..n]).into_owned();
                }
                b = &b[n..];
            }
            _ => return None,
        }
    }
    Some(out)
}

/// This device's name, for a screen that asks who wants to show here
/// (another Kagaz): the computer's name, or "a phone".
pub fn device_name() -> String {
    #[cfg(windows)]
    let host = std::env::var("COMPUTERNAME").unwrap_or_default();
    #[cfg(unix)]
    let host = {
        let mut buf = [0u8; 256];
        // SAFETY: a buffer of the length given; the name ends in a nul.
        let r = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        if r == 0 {
            let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..end]).into_owned()
        } else {
            String::new()
        }
    };
    #[cfg(not(any(windows, unix)))]
    let host = String::new();
    let host = host.trim().trim_end_matches(".local").to_string();
    if cfg!(any(target_os = "android", target_os = "ios")) || host.is_empty() || host == "localhost" {
        "a phone".into()
    } else {
        host
    }
}

/// TLS that takes the TV's own certificate as shown. Cast devices make
/// their certificate themselves, some in the old version 1 form that
/// certificate libraries refuse to read, so neither it nor the signature
/// made with it is checked: the connection is private against listeners,
/// and which device answers is settled by the address the owner chose.
fn tls_setup() -> Option<std::sync::Arc<rustls::ClientConfig>> {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};
    use std::sync::Arc;
    #[derive(Debug)]
    struct AsShown(Arc<rustls::crypto::CryptoProvider>);
    impl ServerCertVerifier for AsShown {
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
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
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
        .with_custom_certificate_verifier(Arc::new(AsShown(provider)))
        .with_no_client_auth();
    Some(Arc::new(config))
}

impl Channel {
    /// Connects and greets the TV.
    pub fn open(tv: IpAddr, wait: Duration) -> Result<Channel, String> {
        Channel::open_at(SocketAddr::new(tv, PORT), wait)
    }

    /// The same at another port (a stand-in TV in a test).
    pub fn open_at(at: SocketAddr, wait: Duration) -> Result<Channel, String> {
        let tv = at.ip();
        let mut tcp = TcpStream::connect_timeout(&at, wait)
            .map_err(|e| format!("the TV does not answer Google Cast (port {}): {e}", at.port()))?;
        tcp.set_nodelay(true).ok();
        tcp.set_read_timeout(Some(wait)).ok();
        tcp.set_write_timeout(Some(wait)).ok();
        let config = tls_setup().ok_or("no TLS on this device")?;
        let name = rustls::pki_types::ServerName::IpAddress(tv.into());
        let mut tls = rustls::ClientConnection::new(config, name).map_err(|e| format!("Cast's secure connection: {e}"))?;
        while tls.is_handshaking() {
            tls.complete_io(&mut tcp).map_err(|e| format!("the TV refused Cast's secure connection: {e}"))?;
        }
        tcp.set_nonblocking(true).map_err(|e| e.to_string())?;
        let now = Instant::now();
        let mut c = Channel { tcp, tls, inbox: Vec::new(), last_ping: now, last_heard: now };
        c.greet(TV)?;
        Ok(c)
    }

    /// Opens a conversation with the TV, or with an app on it.
    pub fn greet(&mut self, to: &str) -> Result<(), String> {
        let name = device_name();
        self.send(to, CONNECTION, &json!({ "type": "CONNECT", "userAgent": "Kagaz", "senderName": name, "connType": 0, "origin": {} }))
    }

    pub fn send(&mut self, to: &str, namespace: &str, json: &Value) -> Result<(), String> {
        let gone = |e: std::io::Error| format!("the Cast connection to the TV broke: {e}");
        self.tls.writer().write_all(&record(ME, to, namespace, json)).map_err(gone)?;
        let give_up = Instant::now() + Duration::from_secs(5);
        while self.tls.wants_write() {
            match self.tls.write_tls(&mut self.tcp) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < give_up => std::thread::sleep(Duration::from_millis(1)),
                Err(e) => return Err(gone(e)),
            }
        }
        Ok(())
    }

    /// What has come since last asked. Answers the TV's "are you there",
    /// and asks the same every five seconds (the TV drops a silent sender).
    pub fn heard(&mut self) -> Result<Vec<Said>, String> {
        loop {
            match self.tls.read_tls(&mut self.tcp) {
                Ok(0) => return Err("the TV closed the Cast connection".into()),
                Ok(_) => {
                    self.tls.process_new_packets().map_err(|e| format!("the Cast connection to the TV broke: {e}"))?;
                    let mut buf = [0u8; 16384];
                    loop {
                        match self.tls.reader().read(&mut buf) {
                            Ok(0) => return Err("the TV closed the Cast connection".into()),
                            Ok(n) => self.inbox.extend(&buf[..n]),
                            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                            Err(e) => return Err(format!("the Cast connection to the TV broke: {e}")),
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(format!("the Cast connection to the TV broke: {e}")),
            }
        }
        let mut out = Vec::new();
        while self.inbox.len() >= 4 {
            let n = u32::from_be_bytes(self.inbox[..4].try_into().unwrap()) as usize;
            if n > LONGEST {
                return Err("the TV sent something that is not Cast".into());
            }
            if self.inbox.len() < 4 + n {
                break;
            }
            let f = fields(&self.inbox[4..4 + n]);
            self.inbox.drain(..4 + n);
            let Some([_, _, from, _, namespace, _, json]) = f else { continue };
            self.last_heard = Instant::now();
            let json: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
            if namespace == HEARTBEAT {
                if json["type"] == "PING" {
                    self.send(&from, HEARTBEAT, &json!({ "type": "PONG" }))?;
                }
                continue;
            }
            out.push(Said { from, namespace, json });
        }
        if self.last_ping.elapsed() >= Duration::from_secs(5) {
            self.last_ping = Instant::now();
            self.send(TV, HEARTBEAT, &json!({ "type": "PING" }))?;
        }
        if self.last_heard.elapsed() >= Duration::from_secs(20) {
            return Err("the TV stopped answering over Cast".into());
        }
        Ok(out)
    }

    /// Waits up to `wait` for a message `want` picks, handing back what it
    /// made of it. `stop` ends the wait early.
    pub fn until<T>(
        &mut self,
        wait: Duration,
        stop: &std::sync::atomic::AtomicBool,
        mut want: impl FnMut(&Said) -> Option<Result<T, String>>,
    ) -> Result<Option<T>, String> {
        let end = Instant::now() + wait;
        while Instant::now() < end && !stop.load(std::sync::atomic::Ordering::Relaxed) {
            for said in self.heard()? {
                if let Some(r) = want(&said) {
                    return r.map(Some);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_round_trips() {
        let long = "x".repeat(300);
        let r = record("sender-kagaz", TV, RECEIVER, &json!({ "type": "LAUNCH", "appId": long }));
        assert_eq!(u32::from_be_bytes(r[..4].try_into().unwrap()) as usize, r.len() - 4);
        // Version 0, then the sender's name as field 2.
        assert_eq!(&r[4..8], [0x08, 0x00, 0x12, 12]);
        let f = fields(&r[4..]).unwrap();
        assert_eq!((f[2].as_str(), f[3].as_str(), f[4].as_str()), ("sender-kagaz", TV, RECEIVER));
        let json: Value = serde_json::from_str(&f[6]).unwrap();
        assert_eq!(json["type"], "LAUNCH");
        assert_eq!(json["appId"].as_str().unwrap().len(), 300);
        // Cut short: not a record.
        assert!(fields(&r[4..r.len() - 5]).is_none());
    }
}
