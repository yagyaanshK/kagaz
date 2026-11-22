//! Reaching a paired computer from another network (docs/remote-connection.md
//! step 7, the owner's plan of 2026-11-23): no server of anyone's in
//! between, and the computer never open to the internet at large.
//!
//! The computer that shares itself keeps a door on a UDP port (the number
//! of its Remote Desktop port). The door answers only a knock made with a
//! paired device's secret: anything else that arrives there is dropped
//! without a word, so to anyone scanning it looks closed. A knock carries
//! the time and a number used once (one recorded and sent again is
//! refused), sealed with a key made from the secret; the device is told
//! apart by a tag that changes every day, so nobody can follow it. Only
//! the address that knocked is let through, and only while it keeps
//! talking. Behind the door an encrypted connection (QUIC, with the
//! certificate pinned at pairing) carries the device's Remote Desktop
//! connection to the computer's own port, where it signs in as it does on
//! the home network: the secret, the PIN, the person asked.
//!
//! The device learns where the computer can be reached on every
//! connection (`ask`), near or far, and keeps it for the next time. The
//! computer also posts its addresses in BitTorrent's address book (the
//! DHT, millions of ordinary computers, nobody's server), one record for
//! each paired device, signed with a key made from that device's secret
//! and the day and sealed with another: only that device finds it or can
//! read it, and nobody can tell it from any other record or follow it
//! from one day to the next.
//!
//! Both programs build this same file.

use super::link;
use hmac::{Hmac, KeyInit, Mac};
use quinn::rustls;
use quinn::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, Runtime, UdpPoller};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, IoSliceMut, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What a question for the computer's addresses starts with, on its
/// Remote Desktop port (as a pairing starts with `link::MAGIC`).
pub const ASK: &[u8; 4] = b"KGZA";
/// What the encrypted connection carries.
pub const ALPN: &[u8] = b"kagaz-far/1";
/// The certificate's name (it is pinned, so the name says nothing).
const NAME: &str = "kagaz-share";
/// A knock: K, its kind, the day's tag, the time, the number used once,
/// the seal.
const KNOCK: usize = 58;
/// How far apart the two clocks may be, in seconds.
const LEEWAY: u64 = 120;
/// How long an address let through stays let through without a word.
const KEPT: Duration = Duration::from_secs(60);
pub const DAY: u64 = 24 * 60 * 60;

fn mac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("any key length");
    for p in parts {
        m.update(&(p.len() as u64).to_be_bytes());
        m.update(p);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&m.finalize().into_bytes());
    out
}

fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Seconds since the epoch.
pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn fresh() -> [u8; 16] {
    let mut n = [0u8; 16];
    getrandom::fill(&mut n).expect("the system's random numbers");
    n
}

/// An address as one family sees it (IPv4 inside IPv6 back to IPv4).
fn plain(a: SocketAddr) -> SocketAddr {
    SocketAddr::new(a.ip().to_canonical(), a.port())
}

/// An address for an IPv6 socket that takes IPv4 too.
fn for_v6(a: SocketAddr) -> SocketAddr {
    match a.ip() {
        IpAddr::V4(v4) => SocketAddr::new(IpAddr::V6(v4.to_ipv6_mapped()), a.port()),
        IpAddr::V6(_) => a,
    }
}

/// One paired device's keys, made from the hash of its secret (what the
/// computer keeps; the device hashes its own).
#[derive(Clone, Debug)]
pub struct Pair {
    /// Who it is, on the side that holds it.
    pub id: String,
    door: [u8; 32],
}

impl Pair {
    pub fn new(id: &str, secret_hash: &str) -> Pair {
        Pair { id: id.into(), door: mac(secret_hash.as_bytes(), &[b"kagaz far door"]) }
    }

    /// A key for one purpose and one day.
    fn key(&self, label: &str, day: u64) -> [u8; 32] {
        mac(&self.door, &[label.as_bytes(), &day.to_be_bytes()])
    }

    /// What tells this device's knocks apart on a day (and nothing on the
    /// next).
    fn tag(&self, day: u64) -> [u8; 8] {
        let k = self.key("tag", day);
        let mut t = [0u8; 8];
        t.copy_from_slice(&k[..8]);
        t
    }
}

/// A knock, or the computer's answer to one.
fn knock(pair: &Pair, answer: bool, nonce: [u8; 16], at: u64) -> [u8; KNOCK] {
    let mut k = [0u8; KNOCK];
    k[0] = b'K';
    k[1] = if answer { 2 } else { 1 };
    k[2..10].copy_from_slice(&pair.tag(at / DAY));
    k[10..18].copy_from_slice(&at.to_be_bytes());
    k[18..34].copy_from_slice(&nonce);
    let m = mac(&pair.door, &[b"knock", &k[..34]]);
    k[34..].copy_from_slice(&m[..24]);
    k
}

/// Whose knock this is, whether it answers one, and its number used once;
/// none when it is not a good knock from a paired device now.
fn read_knock<'a>(pairs: &'a [Pair], packet: &[u8], at: u64) -> Option<(&'a Pair, bool, [u8; 16])> {
    if packet.len() != KNOCK || packet[0] != b'K' || !(1..=2).contains(&packet[1]) {
        return None;
    }
    let when = u64::from_be_bytes(packet[10..18].try_into().ok()?);
    if when.abs_diff(at) > LEEWAY {
        return None;
    }
    let pair = pairs.iter().find(|p| p.tag(when / DAY)[..] == packet[2..10])?;
    let m = mac(&pair.door, &[b"knock", &packet[..34]]);
    if !same_bytes(&m[..24], &packet[34..]) {
        return None;
    }
    Some((pair, packet[1] == 2, packet[18..34].try_into().ok()?))
}

/// Which end of the door this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The computer: answers knocks.
    Host,
    /// The device: knocks, and takes answers.
    Viewer,
}

#[derive(Debug, Default)]
struct Kept {
    pairs: Vec<Pair>,
    /// Who knocked (or answered), from where, last heard when.
    admitted: HashMap<SocketAddr, (String, Instant)>,
    /// Numbers used once, while their knock could still be good.
    nonces: HashMap<[u8; 16], Instant>,
    /// The computer answered from these (the device's side).
    answered: Vec<SocketAddr>,
}

/// A UDP socket that lets through to the encrypted connection only what
/// comes from an address that knocked with a paired device's secret, and
/// says nothing to anything else.
pub struct Door {
    inner: Arc<dyn AsyncUdpSocket>,
    role: Role,
    v6: bool,
    kept: Mutex<Kept>,
    /// Told when an answer comes (the device's side).
    pub heard: tokio::sync::Notify,
}

impl std::fmt::Debug for Door {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Door").field("role", &self.role).field("v6", &self.v6).finish()
    }
}

/// A UDP socket on `port` (0: any) for IPv6 and IPv4 both where it can.
fn udp(port: u16) -> io::Result<(std::net::UdpSocket, bool)> {
    use socket2::{Domain, Protocol, Socket, Type};
    if let Ok(s) = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)) {
        let both = s.set_only_v6(false).is_ok();
        if both && s.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)).into()).is_ok() {
            return Ok((s.into(), true));
        }
    }
    Ok((std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?, false))
}

impl Door {
    /// The door on `port` (0: any), inside a tokio runtime.
    pub fn open(role: Role, port: u16) -> io::Result<Arc<Door>> {
        let (socket, v6) = udp(port)?;
        let inner = quinn::TokioRuntime.wrap_udp_socket(socket)?;
        Ok(Arc::new(Door { inner, role, v6, kept: Mutex::new(Kept::default()), heard: tokio::sync::Notify::new() }))
    }

    pub fn port(&self) -> u16 {
        self.inner.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// The paired devices whose knocks are answered.
    pub fn set_pairs(&self, pairs: Vec<Pair>) {
        self.kept.lock().unwrap().pairs = pairs;
    }

    fn send_raw(&self, to: SocketAddr, bytes: &[u8]) -> io::Result<()> {
        let destination = if self.v6 { for_v6(to) } else { plain(to) };
        if !self.v6 && destination.is_ipv6() {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "no IPv6 here"));
        }
        self.inner.try_send(&Transmit { destination, ecn: None, contents: bytes, segment_size: None, src_ip: None })
    }

    /// Knock at `to` (the device's side).
    pub fn knock(&self, pair: &Pair, to: SocketAddr) -> io::Result<()> {
        self.send_raw(to, &knock(pair, false, fresh(), unix_now()))
    }

    /// Where the computer answered from since last asked.
    pub fn answers(&self) -> Vec<SocketAddr> {
        std::mem::take(&mut self.kept.lock().unwrap().answered)
    }

    /// One datagram from `from`: for the encrypted connection (true), or a
    /// knock dealt with here, or nothing anyone asked for (false, and no
    /// word back).
    fn sort(&self, from: SocketAddr, data: &[u8]) -> bool {
        let from = plain(from);
        let now = Instant::now();
        let mut k = self.kept.lock().unwrap();
        if data.len() == KNOCK {
            if let Some((pair, answer, nonce)) = read_knock(&k.pairs, data, unix_now()) {
                let pair = pair.clone();
                k.nonces.retain(|_, t| now.duration_since(*t) < Duration::from_secs(2 * LEEWAY + 10));
                if k.nonces.contains_key(&nonce) {
                    return false;
                }
                k.nonces.insert(nonce, now);
                k.admitted.retain(|_, (_, t)| now.duration_since(*t) < KEPT);
                match (self.role, answer) {
                    (Role::Host, false) => {
                        k.admitted.insert(from, (pair.id.clone(), now));
                        drop(k);
                        let _ = self.send_raw(from, &knock(&pair, true, fresh(), unix_now()));
                    }
                    (Role::Viewer, true) => {
                        k.admitted.insert(from, (pair.id.clone(), now));
                        k.answered.push(from);
                        drop(k);
                        self.heard.notify_one();
                    }
                    _ => {}
                }
                return false;
            }
        }
        match k.admitted.get_mut(&from) {
            Some((_, seen)) if now.duration_since(*seen) < KEPT => {
                *seen = now;
                true
            }
            _ => false,
        }
    }
}

impl AsyncUdpSocket for Door {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        self.inner.try_send(transmit)
    }

    fn poll_recv(&self, cx: &mut Context, bufs: &mut [IoSliceMut<'_>], meta: &mut [RecvMeta]) -> Poll<io::Result<usize>> {
        loop {
            let n = match self.inner.poll_recv(cx, bufs, meta) {
                Poll::Ready(Ok(n)) => n,
                other => return other,
            };
            let mut kept = 0;
            for i in 0..n {
                let m = meta[i];
                if !self.sort(m.addr, &bufs[i][..m.len]) {
                    continue;
                }
                if kept != i {
                    let (to, from) = bufs.split_at_mut(i);
                    to[kept][..m.len].copy_from_slice(&from[0][..m.len]);
                    meta[kept] = m;
                }
                kept += 1;
            }
            if kept > 0 {
                return Poll::Ready(Ok(kept));
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.inner.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.inner.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

/// Latency first: a word every few seconds keeps the routers' doors open,
/// and a connection gone quiet for half a minute is over.
fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(5)));
    t.max_idle_timeout(Some(Duration::from_secs(30).try_into().expect("a short time")));
    Arc::new(t)
}

fn says(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The computer's end: the encrypted connection behind the door, with the
/// computer's own certificate (the one pinned at pairing).
pub fn host_endpoint(
    door: Arc<Door>,
    der: Vec<u8>,
    key: Vec<u8>,
    provider: Arc<rustls::crypto::CryptoProvider>,
) -> Result<quinn::Endpoint, String> {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(says)?
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(der)], PrivateKeyDer::Pkcs8(key.into()))
        .map_err(says)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(says)?;
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    server.transport_config(transport());
    quinn::Endpoint::new_with_abstract_socket(quinn::EndpointConfig::default(), Some(server), door, Arc::new(quinn::TokioRuntime))
        .map_err(says)
}

/// Each stream a device opens goes on to the computer's Remote Desktop
/// port, at `rdp`, where it is signed in as any other.
pub async fn serve(endpoint: quinn::Endpoint, rdp: SocketAddr) {
    while let Some(incoming) = endpoint.accept().await {
        tokio::spawn(async move {
            let Ok(conn) = incoming.await else { return };
            while let Ok((send, recv)) = conn.accept_bi().await {
                tokio::spawn(async move {
                    if let Ok(tcp) = tokio::net::TcpStream::connect(rdp).await {
                        let _ = tcp.set_nodelay(true);
                        pipe(tcp, send, recv).await;
                    }
                });
            }
        });
    }
}

async fn pipe(tcp: tokio::net::TcpStream, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
    use tokio::io::AsyncWriteExt;
    let (mut r, mut w) = tcp.into_split();
    let up = async {
        let _ = tokio::io::copy(&mut r, &mut send).await;
        let _ = send.finish();
    };
    let down = async {
        let _ = tokio::io::copy(&mut recv, &mut w).await;
        let _ = w.shutdown().await;
    };
    tokio::join!(up, down);
}

/// Takes only the certificate pinned at pairing.
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if link::same(&link::hex(&Sha256::digest(end_entity.as_ref())), &self.fingerprint.to_ascii_lowercase()) {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("not the paired computer".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn client_config(fingerprint: &str, provider: Arc<rustls::crypto::CryptoProvider>) -> Result<quinn::ClientConfig, String> {
    let verifier = Arc::new(Pinned { fingerprint: fingerprint.into(), provider: provider.clone() });
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(says)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(says)?;
    let mut c = quinn::ClientConfig::new(Arc::new(crypto));
    c.transport_config(transport());
    Ok(c)
}

/// The computer, reached from afar: Remote Desktop goes to `local` on this
/// device, and on through the encrypted connection to the computer. It
/// stays while a connection through it is open, and a little after.
#[derive(Debug, Clone)]
pub struct Reached {
    pub local: SocketAddr,
    /// Where the computer answered from.
    pub via: SocketAddr,
}

/// Where to look for the computer besides the addresses already known.
#[derive(Debug, Clone, Default)]
pub struct Looking {
    /// In BitTorrent's address book, for what the computer posted there.
    pub book: bool,
    /// Its nodes to start from (none: the usual ones; a test's own).
    pub bootstrap: Option<Vec<String>>,
}

impl Looking {
    fn dht(&self) -> Result<mainline::Dht, String> {
        let mut b = mainline::Dht::builder();
        if let Some(nodes) = &self.bootstrap {
            b.bootstrap(nodes);
        }
        b.build().map_err(says)
    }
}

/// Knock at each of `known`, and at what `looking` finds, until the
/// computer answers, and open the encrypted connection to it, its
/// certificate the one pinned (`fingerprint`).
pub fn reach(
    pair: Pair,
    fingerprint: String,
    known: Vec<SocketAddr>,
    looking: Looking,
    provider: Arc<rustls::crypto::CryptoProvider>,
) -> Result<Reached, String> {
    if known.is_empty() && !looking.book {
        return Err("no address from afar is known for it yet".into());
    }
    let (told, heard) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("far".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => {
                    let _ = told.send(Err(says(e)));
                    return;
                }
            };
            runtime.block_on(tunnel(pair, fingerprint, known, looking, provider, told));
        })
        .map_err(says)?;
    heard.recv_timeout(Duration::from_secs(90)).map_err(|_| "the computer did not answer from afar".to_string())?
}

/// What the knocking learns as it goes: more places to knock, and whether
/// the looking is over.
#[derive(Default)]
struct Search {
    at: Vec<SocketAddr>,
    /// When the last look ended (none: still looking).
    looked: Option<Instant>,
    why: Option<String>,
}

async fn tunnel(
    pair: Pair,
    fingerprint: String,
    known: Vec<SocketAddr>,
    looking: Looking,
    provider: Arc<rustls::crypto::CryptoProvider>,
    told: std::sync::mpsc::Sender<Result<Reached, String>>,
) {
    let fail = |why: String| {
        let _ = told.send(Err(why));
    };
    let door = match Door::open(Role::Viewer, 0) {
        Ok(d) => d,
        Err(e) => return fail(says(e)),
    };
    door.set_pairs(vec![pair.clone()]);
    // The connection's end reads the door from now on (answers too).
    let config = match client_config(&fingerprint, provider) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let endpoint = match quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        None,
        door.clone(),
        Arc::new(quinn::TokioRuntime),
    ) {
        Ok(e) => e,
        Err(e) => return fail(says(e)),
    };
    let began = Instant::now();
    let search = Arc::new(Mutex::new(Search { at: known, looked: None, why: None }));
    if looking.book {
        let (search, pair) = (search.clone(), pair.clone());
        std::thread::spawn(move || {
            let found = looking.dht().map(|dht| read(&dht, &pair, BOOK));
            let mut s = search.lock().unwrap();
            match found {
                Ok(Some(r)) => {
                    for a in r.addresses {
                        if !s.at.contains(&a) {
                            s.at.push(a);
                        }
                    }
                }
                Ok(None) => s.why = Some("it has posted no address in the address book".into()),
                Err(e) => s.why = Some(format!("the address book could not be reached ({e})")),
            }
            s.looked = Some(Instant::now());
        });
    } else {
        search.lock().unwrap().looked = Some(began);
    }
    // Knocks, a few a second, until an answer, or a few seconds after the
    // looking is over.
    let via = loop {
        let (at, over) = {
            let s = search.lock().unwrap();
            let over = s.looked.is_some_and(|t| t.elapsed() > Duration::from_secs(4)) || began.elapsed() > Duration::from_secs(40);
            (s.at.clone(), over)
        };
        for a in &at {
            let _ = door.knock(&pair, *a);
        }
        if let Some(a) = door.answers().into_iter().next() {
            break a;
        }
        if over {
            let why = search.lock().unwrap().why.clone();
            return fail(match why {
                Some(w) if at.is_empty() => w,
                _ => "the computer did not answer from afar (switched off, asleep, or From anywhere is off there)".into(),
            });
        }
        let _ = tokio::time::timeout(Duration::from_millis(300), door.heard.notified()).await;
        if let Some(a) = door.answers().into_iter().next() {
            break a;
        }
    };
    let conn = match endpoint.connect_with(config, via, NAME) {
        Ok(c) => match tokio::time::timeout(Duration::from_secs(10), c).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return fail(format!("the encrypted connection did not open ({e})")),
            Err(_) => return fail("the encrypted connection did not open in time".into()),
        },
        Err(e) => return fail(says(e)),
    };
    let listener = match tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
        Ok(l) => l,
        Err(e) => return fail(says(e)),
    };
    let local = match listener.local_addr() {
        Ok(a) => a,
        Err(e) => return fail(says(e)),
    };
    let _ = told.send(Ok(Reached { local, via }));
    // Each connection made here goes through as a stream of its own; the
    // tunnel ends half a minute after the last one, or with the computer.
    let open = Arc::new(AtomicUsize::new(0));
    let mut quiet = Instant::now();
    loop {
        tokio::select! {
            got = listener.accept() => {
                let Ok((tcp, _)) = got else { continue };
                let _ = tcp.set_nodelay(true);
                let Ok((send, recv)) = conn.open_bi().await else { break };
                let open = open.clone();
                open.fetch_add(1, Ordering::AcqRel);
                tokio::spawn(async move {
                    pipe(tcp, send, recv).await;
                    open.fetch_sub(1, Ordering::AcqRel);
                });
            }
            _ = conn.closed() => break,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                if open.load(Ordering::Acquire) > 0 {
                    quiet = Instant::now();
                } else if quiet.elapsed() > Duration::from_secs(30) {
                    break;
                }
            }
        }
    }
    conn.close(0u32.into(), b"done");
    endpoint.wait_idle().await;
}

// ---- BitTorrent's address book ----
//
// A record is a "mutable item" (BEP 44) of the DHT: signed with a key
// made from the pair's secret and the day, so it is found only by those
// who can make the key and looks like any other record to anyone else;
// what it says is sealed with another key from the secret. A computer
// posts one record for each paired device.

/// The computer's addresses, for a paired device.
pub const BOOK: &str = "book";

/// What a record says.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// When it was posted (seconds since the epoch).
    pub at: u64,
    pub addresses: Vec<SocketAddr>,
}

/// A record older than this is not believed.
const RECORD_LASTS: u64 = 2 * DAY;

fn record_keys(pair: &Pair, label: &str, day: u64) -> (mainline::SigningKey, [u8; 32]) {
    let signer = mainline::SigningKey::from_bytes(&pair.key(&format!("{label} signer"), day));
    (signer, pair.key(&format!("{label} seal"), day))
}

fn seal(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    use chacha20poly1305::aead::{Aead, KeyInit as _};
    let cipher = chacha20poly1305::ChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).expect("the system's random numbers");
    let mut out = nonce.to_vec();
    out.extend(cipher.encrypt(&nonce.into(), plain).expect("sealing never fails"));
    out
}

fn unseal(key: &[u8; 32], sealed: &[u8]) -> Option<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, KeyInit as _};
    if sealed.len() < 12 {
        return None;
    }
    let cipher = chacha20poly1305::ChaCha20Poly1305::new(key.into());
    let nonce: [u8; 12] = sealed[..12].try_into().ok()?;
    cipher.decrypt(&nonce.into(), &sealed[12..]).ok()
}

/// Post `addresses` for `pair` under `label`, for today.
#[allow(deprecated)]
pub fn post(dht: &mainline::Dht, pair: &Pair, label: &str, addresses: &[SocketAddr]) -> Result<(), String> {
    let at = unix_now();
    let (signer, key) = record_keys(pair, label, at / DAY);
    let said = serde_json::json!({ "at": at, "a": addresses.iter().map(|a| a.to_string()).collect::<Vec<_>>() });
    let value = seal(&key, said.to_string().as_bytes());
    let item = mainline::MutableItem::new(signer, &value, at as i64, None);
    dht.put_mutable(item, None).map(|_| ()).map_err(says)
}

/// The newest record for `pair` under `label` (today's, else yesterday's).
#[allow(deprecated)]
pub fn read(dht: &mainline::Dht, pair: &Pair, label: &str) -> Option<Record> {
    let now = unix_now();
    let mut best: Option<Record> = None;
    for day in [now / DAY, now / DAY - 1] {
        let (signer, key) = record_keys(pair, label, day);
        let Some(item) = dht.get_mutable_most_recent(signer.verifying_key().as_bytes(), None) else { continue };
        let Some(plain) = unseal(&key, item.value()) else { continue };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&plain) else { continue };
        let at = v["at"].as_u64().unwrap_or(0);
        if now.saturating_sub(at) > RECORD_LASTS || best.as_ref().is_some_and(|b| b.at >= at) {
            continue;
        }
        let addresses = v["a"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()?.parse().ok()).collect()).unwrap_or_default();
        best = Some(Record { at, addresses });
    }
    best
}

/// What a computer says of being reached from afar.
#[derive(Debug, Clone, PartialEq)]
pub struct Told {
    /// From anywhere is on there.
    pub on: bool,
    /// Where it can be knocked at now (it may know none yet).
    pub addresses: Vec<SocketAddr>,
}

/// Ask the computer at `addr` (its Remote Desktop port, near or through
/// the door) where it can be reached from afar. Only a paired device is
/// told, and the answer is proved by the computer.
pub fn ask(addr: SocketAddr, pair: &Pair, wait: Duration) -> io::Result<Told> {
    let mut s = TcpStream::connect_timeout(&addr, wait)?;
    s.set_read_timeout(Some(wait))?;
    s.set_write_timeout(Some(wait))?;
    s.write_all(ASK)?;
    let nonce = fresh();
    let at = unix_now();
    let proof = mac(&pair.door, &[b"ask", &nonce, &at.to_be_bytes()]);
    link::write_frame(
        &mut s,
        &serde_json::json!({ "tag": link::hex(&pair.tag(at / DAY)), "at": at, "nonce": link::hex(&nonce), "proof": link::hex(&proof) }),
    )?;
    let reply = link::read_frame(&mut s)?;
    let list: Vec<String> = reply["addresses"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let on = reply["on"].as_bool().unwrap_or(false);
    let told = mac(&pair.door, &[b"told", &nonce, &[on as u8], list.join(",").as_bytes()]);
    if !link::same(reply["proof"].as_str().unwrap_or(""), &link::hex(&told)) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not the paired computer"));
    }
    Ok(Told { on, addresses: list.iter().filter_map(|a| a.parse().ok()).collect() })
}

/// The computer's side of `ask`, after its mark: what is asked by a paired
/// device is answered (whether From anywhere is on, and `addresses`);
/// anything else, with nothing.
pub fn answer_ask(s: &mut (impl Read + Write), pairs: &[Pair], on: bool, addresses: &[SocketAddr]) -> io::Result<()> {
    let q = link::read_frame(s)?;
    let bad = || io::Error::new(io::ErrorKind::PermissionDenied, "not a paired device");
    let at = q["at"].as_u64().ok_or_else(bad)?;
    if at.abs_diff(unix_now()) > LEEWAY {
        return Err(bad());
    }
    let tag = link::unhex(q["tag"].as_str().unwrap_or("")).ok_or_else(bad)?;
    let nonce = link::unhex(q["nonce"].as_str().unwrap_or("")).ok_or_else(bad)?;
    let pair = pairs.iter().find(|p| p.tag(at / DAY)[..] == tag[..]).ok_or_else(bad)?;
    let proof = mac(&pair.door, &[b"ask", &nonce, &at.to_be_bytes()]);
    if !link::same(q["proof"].as_str().unwrap_or(""), &link::hex(&proof)) {
        return Err(bad());
    }
    let list: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
    let told = mac(&pair.door, &[b"told", &nonce, &[on as u8], list.join(",").as_bytes()]);
    link::write_frame(s, &serde_json::json!({ "on": on, "addresses": list, "proof": link::hex(&told) }))
}

/// Is this address one the internet can reach (not a home network's,
/// nor one an internet provider shares among homes)?
pub fn public(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || (o[0] == 100 && (o[1] & 0xC0) == 64)
                || o[0] == 0
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => (v6.segments()[0] & 0xE000) == 0x2000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_knock_is_taken_once_from_its_device_and_only_now() {
        let a = Pair::new("a", "1111");
        let b = Pair::new("b", "2222");
        let now = unix_now();
        let k = knock(&a, false, [7; 16], now);
        let pairs = [b.clone(), a.clone()];
        let (who, answer, nonce) = read_knock(&pairs, &k, now).unwrap();
        assert_eq!((who.id.as_str(), answer, nonce), ("a", false, [7; 16]));
        assert!(read_knock(&[b], &k, now).is_none(), "another device's secret");
        assert!(read_knock(&pairs, &k, now + 600).is_none(), "too old");
        let mut bent = k;
        bent[20] ^= 1;
        assert!(read_knock(&pairs, &bent, now).is_none(), "changed on the way");
        // Another day, another tag: nobody can follow a device.
        assert_ne!(a.tag(now / DAY), a.tag(now / DAY + 1));
    }

    #[test]
    fn addresses_the_internet_reaches() {
        assert!(public("203.0.113.9".parse().unwrap()));
        assert!(!public("192.168.1.5".parse().unwrap()));
        assert!(!public("100.72.1.5".parse().unwrap()), "shared by an internet provider");
        assert!(public("2001:db8::5".parse().unwrap()));
        assert!(!public("fe80::1".parse().unwrap()));
        assert!(!public("fd7a:115c:a1e0::1".parse().unwrap()));
    }

    /// A computer with its door open, its Remote Desktop port echoing
    /// what it is sent; the device's keys, and the certificate to pin.
    fn computer() -> (tokio::runtime::Runtime, SocketAddr, Pair, String) {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cert = rcgen::generate_simple_self_signed(vec![NAME.into()]).unwrap();
        let der = cert.cert.der().to_vec();
        let key = cert.signing_key.serialize_der();
        let fingerprint = link::hex(&Sha256::digest(&der));
        let pair = Pair::new("dev", &link::hash("secret"));
        // The computer's Remote Desktop port: echoes what it is sent.
        let rdp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let rdp_at = rdp.local_addr().unwrap();
        std::thread::spawn(move || {
            for s in rdp.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut s = s;
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = s.read(&mut buf) {
                        if n == 0 || s.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let host_pair = Pair::new("dev", &link::hash("secret"));
        let p = provider.clone();
        let port = runtime.block_on(async move {
            let door = Door::open(Role::Host, 0).unwrap();
            door.set_pairs(vec![host_pair]);
            let port = door.port();
            let ep = host_endpoint(door, der, key, p).unwrap();
            tokio::spawn(serve(ep, rdp_at));
            port
        });
        (runtime, (Ipv4Addr::LOCALHOST, port).into(), pair, fingerprint)
    }

    fn echoes(r: &Reached) {
        let mut s = TcpStream::connect(r.local).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(b"hello computer").unwrap();
        let mut got = [0u8; 14];
        s.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"hello computer");
    }

    #[test]
    fn the_door_lets_only_a_paired_device_through_to_the_computer() {
        let (runtime, door_at, pair, fingerprint) = computer();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        // A stranger hears nothing back.
        let stranger = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        stranger.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        stranger.send_to(&knock(&Pair::new("x", "other"), false, fresh(), unix_now()), door_at).unwrap();
        stranger.send_to(b"\xc0\x00\x00\x00\x01hello", door_at).unwrap();
        let mut buf = [0u8; 2048];
        assert!(stranger.recv_from(&mut buf).is_err(), "the door says nothing to a stranger");
        // A wrong certificate is refused.
        let wrong = reach(pair.clone(), "00".repeat(32), vec![door_at], Looking::default(), provider.clone());
        assert!(wrong.is_err());
        // The paired device gets through, to the computer's own port.
        let r = reach(pair, fingerprint, vec![door_at], Looking::default(), provider).unwrap();
        echoes(&r);
        drop(runtime);
    }

    #[test]
    #[allow(deprecated)]
    fn the_address_book_leads_only_a_paired_device_to_the_computer() {
        let testnet = mainline::Testnet::builder(10).bind_address(Ipv4Addr::LOCALHOST).build().unwrap();
        let (runtime, door_at, pair, fingerprint) = computer();
        // The computer posts where it is, for its paired device.
        let dht = mainline::Dht::builder().bootstrap(&testnet.bootstrap).bind_address(Ipv4Addr::LOCALHOST).build().unwrap();
        post(&dht, &pair, BOOK, &[door_at]).unwrap();
        // Another device's keys find nothing there; the pair's, the door.
        let other = mainline::Dht::builder().bootstrap(&testnet.bootstrap).bind_address(Ipv4Addr::LOCALHOST).build().unwrap();
        assert!(read(&other, &Pair::new("x", "another secret"), BOOK).is_none());
        assert_eq!(read(&other, &pair, BOOK).unwrap().addresses, vec![door_at]);
        // Knowing no address, the device finds it there and gets through.
        let looking = Looking { book: true, bootstrap: Some(testnet.bootstrap.clone()) };
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let r = reach(pair, fingerprint, Vec::new(), looking, provider).unwrap();
        assert_eq!(r.via, door_at);
        echoes(&r);
        drop(runtime);
    }

    #[test]
    fn only_a_paired_device_is_told_the_addresses() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let at = listener.local_addr().unwrap();
        let pairs = vec![Pair::new("dev", "hash")];
        let told: Vec<SocketAddr> = vec!["203.0.113.9:3390".parse().unwrap()];
        let t = told.clone();
        std::thread::spawn(move || {
            for s in listener.incoming().flatten().take(2) {
                let mut s = s;
                let mut mark = [0u8; 4];
                s.read_exact(&mut mark).unwrap();
                assert_eq!(&mark, ASK);
                let _ = answer_ask(&mut s, &pairs, true, &t);
            }
        });
        assert_eq!(ask(at, &Pair::new("dev", "hash"), Duration::from_secs(3)).unwrap(), Told { on: true, addresses: told });
        assert!(ask(at, &Pair::new("dev", "other"), Duration::from_secs(3)).is_err());
    }
}
