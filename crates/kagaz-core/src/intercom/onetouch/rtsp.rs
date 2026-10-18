//! Loopback RTSP adapter. Adds OneTouch's Account/Manual DESCRIBE headers,
//! rewrites relay URLs and relays UDP RTP/RTCP to the loopback video engine.
//! go2rtc handles RTSP negotiation, H.264 and browser remuxing.
use super::{md5_hex, rtp::H264Rtp};
use base64::Engine as _;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
    UdpSocket,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use url::Url;

#[derive(Clone)]
pub struct Target {
    pub(super) remote: String,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) password: String,
    pub(super) account: String,
    inband_h264: bool,
}
impl Target {
    pub fn new(relay: &str, mac: &str, password: &str, account: &str) -> io::Result<Self> {
        let url = Url::parse(&format!("rtsp://{relay}/{mac}")).map_err(|_| invalid())?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != format!("/{mac}")
            || mac.len() != 12
            || !mac.bytes().all(|c| c.is_ascii_hexdigit())
            || account.contains(['\r', '\n'])
        {
            return Err(invalid());
        }
        Ok(Self {
            remote: url.to_string(),
            host: url.host_str().ok_or_else(invalid)?.into(),
            port: url.port().unwrap_or(554),
            password: password.into(),
            account: account.into(),
            inband_h264: false,
        })
    }
    /// Smart Cloud relays can advertise stale H.264 parameter sets. Their RTP
    /// carries the active SPS/PPS; do not let the engine inject the stale sets
    /// at every keyframe. Legacy sources keep their advertised configuration.
    pub fn with_inband_h264(mut self) -> Self {
        self.inband_h264 = true;
        self
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid OneTouch RTSP message")
}

pub struct Bridge {
    pub url: String,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<TcpStream>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Bridge {
    pub fn start(target: Target) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let local = format!("rtsp://127.0.0.1:{}/{token}", listener.local_addr()?.port());
        let mut source = Url::parse(&local).map_err(|_| invalid())?;
        source.set_username("user").map_err(|_| invalid())?;
        source.set_password(Some("local")).map_err(|_| invalid())?;
        let stop = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let running = stop.clone();
        let connections = sockets.clone();
        let thread = std::thread::Builder::new()
            .name("onetouch-rtsp".into())
            .spawn(move || {
                // One video consumer at a time. Socket shutdown interrupts active forwarding.
                while !running.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((client, _)) => {
                            let _ = forward(client, &target, &local, &connections, &running);
                            connections.lock().unwrap().clear();
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(40))
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            // go2rtc separates source options with #, not URL query separators.
            url: format!("{}#transport=udp#backchannel=0", source),
            stop,
            sockets,
            thread: Some(thread),
        })
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for s in self.sockets.lock().unwrap().iter() {
            let _ = s.shutdown(Shutdown::Both);
        }
        // DNS/connect has a finite timeout; do not block the UI waiting for it.
        self.thread.take();
    }
}

pub(super) enum Frame {
    Binary(Vec<u8>),
    Message(String, Vec<u8>),
}
pub(super) fn frame(reader: &mut impl BufRead) -> io::Result<Frame> {
    let mut first = [0];
    reader.read_exact(&mut first)?;
    if first[0] == b'$' {
        let mut header = [0; 3];
        reader.read_exact(&mut header)?;
        let size = u16::from_be_bytes([header[1], header[2]]) as usize;
        let mut data = vec![0; size + 4];
        data[0] = b'$';
        data[1..4].copy_from_slice(&header);
        reader.read_exact(&mut data[4..])?;
        return Ok(Frame::Binary(data));
    }
    let mut bytes = vec![first[0]];
    // Bound every line as well as the entire header, including on malformed peers.
    loop {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if bytes.len() > 64 * 1024 {
            return Err(invalid());
        }
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = String::from_utf8(bytes).map_err(|_| invalid())?;
    let length = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.trim().parse::<usize>().map_err(|_| invalid()))
        .transpose()?
        .unwrap_or(0);
    if length > 1024 * 1024 {
        return Err(invalid());
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Frame::Message(header, body))
}
pub(super) fn parameter(header: &str, name: &str) -> Option<String> {
    header
        .trim_start_matches("Digest ")
        .split(',')
        .find_map(|part| {
            let (k, v) = part.trim().split_once('=')?;
            (k == name).then(|| v.trim().trim_matches('"').to_string())
        })
}
fn rewrite_request(header: &str, target: &Target, local: &str) -> io::Result<String> {
    let mut lines = header.lines();
    let first = lines.next().ok_or_else(invalid)?;
    let parts: Vec<_> = first.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(invalid());
    }
    let method = parts[0];
    let original = Url::parse(parts[1]).map_err(|_| invalid())?;
    let local_url = Url::parse(local).map_err(|_| invalid())?;
    let base_path = local_url.path();
    if original.host_str() != local_url.host_str()
        || original.port() != local_url.port()
        || !(original.path() == base_path || original.path().starts_with(&format!("{base_path}/")))
    {
        return Err(invalid());
    }
    let uri = format!("{}{}", target.remote, &original.path()[base_path.len()..]);
    let mut result = format!("{method} {uri} RTSP/1.0\r\n");
    for line in lines.filter(|s| !s.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(invalid());
        };
        if name.eq_ignore_ascii_case("authorization") && value.trim().starts_with("Digest ") {
            let realm = parameter(value.trim(), "realm").ok_or_else(invalid)?;
            let nonce = parameter(value.trim(), "nonce").ok_or_else(invalid)?;
            let ha1 = md5_hex(&format!("user:{realm}:{}", target.password));
            let ha2 = md5_hex(&format!("{method}:{uri}"));
            let qop = parameter(value.trim(), "qop");
            let (response, tail) = if let Some(qop) = qop {
                if qop != "auth" {
                    return Err(invalid());
                }
                let nc = parameter(value.trim(), "nc").ok_or_else(invalid)?;
                let cn = parameter(value.trim(), "cnonce").ok_or_else(invalid)?;
                (
                    md5_hex(&format!("{ha1}:{nonce}:{nc}:{cn}:auth:{ha2}")),
                    format!(", qop=auth, nc={nc}, cnonce=\"{cn}\""),
                )
            } else {
                (md5_hex(&format!("{ha1}:{nonce}:{ha2}")), String::new())
            };
            result.push_str(&format!("Authorization: Digest username=\"user\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\"{tail}\r\n"));
        } else if name.eq_ignore_ascii_case("authorization") && value.trim().starts_with("Basic ") {
            let auth = base64::engine::general_purpose::STANDARD
                .encode(format!("user:{}", target.password));
            result.push_str(&format!("Authorization: Basic {auth}\r\n"));
        } else if !name.eq_ignore_ascii_case("account") && !name.eq_ignore_ascii_case("manual") {
            result.push_str(line);
            result.push_str("\r\n");
        }
    }
    if method == "DESCRIBE" {
        result.push_str(&format!("Account: {}\r\nManual: 1\r\n", target.account));
    }
    result.push_str("\r\n");
    Ok(result)
}
fn inband_h264_sdp(body: &[u8]) -> Vec<u8> {
    let sdp = String::from_utf8_lossy(body);
    let payloads: Vec<_> = sdp
        .lines()
        .filter_map(|line| {
            let (payload, codec) = line.strip_prefix("a=rtpmap:")?.split_once(' ')?;
            codec
                .split('/')
                .next()?
                .eq_ignore_ascii_case("H264")
                .then_some(payload)
        })
        .collect();
    let lines: Vec<_> = sdp
        .lines()
        .map(|line| {
            if let Some((payload, params)) =
                line.strip_prefix("a=fmtp:").and_then(|v| v.split_once(' '))
            {
                if payloads.contains(&payload) {
                    let params = params
                        .split(';')
                        .filter(|param| {
                            let key = param.trim().split('=').next().unwrap_or("");
                            !key.eq_ignore_ascii_case("sprop-parameter-sets")
                                && !key.eq_ignore_ascii_case("profile-level-id")
                        })
                        .collect::<Vec<_>>()
                        .join(";");
                    return format!("a=fmtp:{payload} {params}");
                }
            }
            line.to_string()
        })
        .collect();
    let mut sdp = lines.join("\r\n");
    sdp.push_str("\r\n");
    sdp.into_bytes()
}
fn rewrite_response(header: &str, body: &[u8], remote: &str, local: &str) -> (String, Vec<u8>) {
    let body = if header.to_ascii_lowercase().contains("application/sdp") {
        String::from_utf8_lossy(body)
            .replace(remote, local)
            .into_bytes()
    } else {
        body.to_vec()
    };
    let mut out = String::new();
    for line in header.lines().filter(|s| !s.is_empty()) {
        if line
            .split_once(':')
            .is_some_and(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        {
            out.push_str(&format!("Content-Length: {}", body.len()));
        } else {
            out.push_str(&line.replace(remote, local));
        }
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    (out, body)
}
pub(super) fn header_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}
fn set_header(header: &str, name: &str, value: &str) -> String {
    let mut out = String::new();
    for line in header.lines().filter(|line| !line.is_empty()) {
        if !line
            .split_once(':')
            .is_some_and(|(key, _)| key.eq_ignore_ascii_case(name))
        {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str(&format!("{name}: {value}\r\n\r\n"));
    out
}
pub(super) fn ports(transport: &str, name: &str) -> io::Result<[u16; 2]> {
    let value = transport
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
        .ok_or_else(invalid)?;
    let (a, b) = value.split_once('-').ok_or_else(invalid)?;
    let pair = [
        a.parse().map_err(|_| invalid())?,
        b.parse().map_err(|_| invalid())?,
    ];
    if pair.contains(&0) || pair[0] == pair[1] {
        return Err(invalid());
    }
    Ok(pair)
}

// The relay answers SETUP with UDP even when TCP was requested. Let go2rtc
// negotiate UDP, but bridge its loopback endpoints to the authenticated peer.
struct UdpSetup {
    sequence: String,
    sockets: [UdpSocket; 2],
    client_ports: [u16; 2],
    ssrc: u32,
}
impl UdpSetup {
    fn new(header: &str, peer: IpAddr) -> io::Result<Self> {
        let client_ports = ports(
            header_value(header, "Transport").ok_or_else(invalid)?,
            "client_port",
        )?;
        let bind_ip = if peer.is_ipv4() {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        };
        let sockets = [
            UdpSocket::bind((bind_ip, 0))?,
            UdpSocket::bind((bind_ip, 0))?,
        ];
        let random = uuid::Uuid::new_v4();
        let ssrc = 100_000_000
            + u32::from_be_bytes(random.as_bytes()[..4].try_into().unwrap()) % 900_000_000;
        Ok(Self {
            sequence: header_value(header, "CSeq").ok_or_else(invalid)?.into(),
            sockets,
            client_ports,
            ssrc,
        })
    }
    fn local_ports(&self) -> io::Result<[u16; 2]> {
        Ok([
            self.sockets[0].local_addr()?.port(),
            self.sockets[1].local_addr()?.port(),
        ])
    }
    fn request(&self, header: &str) -> io::Result<String> {
        let ports = self.local_ports()?;
        let header = set_header(
            header,
            "Transport",
            &format!("RTP/AVP;unicast;client_port={}-{}", ports[0], ports[1]),
        );
        Ok(set_header(&header, "X-SSRC", &self.ssrc.to_string()))
    }
    fn start(
        self,
        header: &str,
        peer: IpAddr,
        stop: Arc<AtomicBool>,
        h264_payload: Option<u8>,
    ) -> io::Result<String> {
        if header_value(header, "X-Rtp-Confuse").is_some_and(|value| value != "0") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Encrypted OneTouch RTP is unsupported",
            ));
        }
        let server_ports = ports(
            header_value(header, "Transport").ok_or_else(invalid)?,
            "server_port",
        )?;
        let local_ports = self.local_ports()?;
        let rewritten = set_header(
            header,
            "Transport",
            &format!(
                "RTP/AVP;unicast;client_port={}-{};server_port={}-{}",
                self.client_ports[0], self.client_ports[1], local_ports[0], local_ports[1]
            ),
        );
        for (index, socket) in self.sockets.into_iter().enumerate() {
            let upstream = SocketAddr::new(peer, server_ports[index]);
            let loopback = if peer.is_ipv4() {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            } else {
                IpAddr::V6(Ipv6Addr::LOCALHOST)
            };
            let client = SocketAddr::new(loopback, self.client_ports[index]);
            let mut video = if index == 0 {
                h264_payload.map(H264Rtp::new)
            } else {
                None
            };
            socket.set_read_timeout(Some(Duration::from_millis(if video.is_some() {
                20
            } else {
                200
            })))?;
            // Open the NAT mapping from the advertised port, as the phone does.
            let mut probe = [0u8; 12];
            probe[0] = 0x80;
            probe[1] = 96;
            probe[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
            socket.send_to(&probe, upstream)?;
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("onetouch-rtp".into())
                .spawn(move || {
                    let mut buffer = [0u8; 65535];
                    while !stop.load(Ordering::Acquire) {
                        match socket.recv_from(&mut buffer) {
                            Ok((size, source)) => {
                                let destination = if source == upstream {
                                    if let Some(video) = video.as_mut() {
                                        let packets = video.push(&buffer[..size], Instant::now());
                                        if packets
                                            .iter()
                                            .any(|p| socket.send_to(p, client).is_err())
                                        {
                                            break;
                                        }
                                        continue;
                                    }
                                    client
                                } else if source == client {
                                    upstream
                                } else {
                                    continue;
                                };
                                // Ignore unrelated senders; never forward to a packet-supplied address.
                                if socket.send_to(&buffer[..size], destination).is_err() {
                                    break;
                                }
                            }
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                                ) =>
                            {
                                if let Some(video) = video.as_mut() {
                                    let packets = video.poll(Instant::now());
                                    if packets.iter().any(|p| socket.send_to(p, client).is_err()) {
                                        break;
                                    }
                                }
                            }
                            Err(_) => break,
                        }
                    }
                })?;
        }
        Ok(rewritten)
    }
}
struct StopDatagrams(Arc<AtomicBool>);
impl Drop for StopDatagrams {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn forward(
    client: TcpStream,
    target: &Target,
    local: &str,
    sockets: &Arc<Mutex<Vec<TcpStream>>>,
    stop: &AtomicBool,
) -> io::Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(30)))?;
    client.set_write_timeout(Some(Duration::from_secs(10)))?;
    sockets.lock().unwrap().push(client.try_clone()?);
    // Authenticate the random loopback path before connecting to the cloud.
    let mut client_reader = BufReader::new(client.try_clone()?);
    let Frame::Message(first, body) = frame(&mut client_reader)? else {
        return Err(invalid());
    };
    let first = rewrite_request(&first, target, local)?;
    let addresses = (target.host.as_str(), target.port).to_socket_addrs()?;
    let mut upstream = None;
    for address in addresses.take(4) {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Ok(s) = TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
            upstream = Some(s);
            break;
        }
    }
    let mut upstream = upstream.ok_or_else(|| {
        io::Error::new(io::ErrorKind::TimedOut, "OneTouch video relay unavailable")
    })?;
    upstream.set_read_timeout(Some(Duration::from_secs(30)))?;
    upstream.set_write_timeout(Some(Duration::from_secs(10)))?;
    sockets.lock().unwrap().push(upstream.try_clone()?);
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    upstream.write_all(first.as_bytes())?;
    upstream.write_all(&body)?;
    let mut response_reader = BufReader::new(upstream.try_clone()?);
    let mut receiver = client.try_clone()?;
    let remote = Arc::new(Mutex::new(target.remote.clone()));
    let response_remote = remote.clone();
    let pending: Arc<Mutex<Option<UdpSetup>>> = Arc::new(Mutex::new(None));
    let response_pending = pending.clone();
    let datagrams = Arc::new(AtomicBool::new(false));
    let _stop_datagrams = StopDatagrams(datagrams.clone());
    let peer = upstream.peer_addr()?.ip();
    let local_response = local.to_string();
    let inband_h264 = target.inband_h264;
    let writer_thread = std::thread::spawn(move || {
        let result = (|| -> io::Result<()> {
            let mut h264_payload = None;
            loop {
                match frame(&mut response_reader)? {
                    Frame::Binary(bytes) => receiver.write_all(&bytes)?,
                    Frame::Message(mut header, body) => {
                        // The cloud's Content-Base can use another authority and a
                        // session-specific path. Keep using the authenticated TCP
                        // connection; only map subsequent request URIs to that base.
                        if let Some(base) = header_value(&header, "Content-Base") {
                            let url = Url::parse(base).map_err(|_| invalid())?;
                            if url.scheme() != "rtsp"
                                || url.host_str().is_none()
                                || !url.username().is_empty()
                                || url.password().is_some()
                                || url.query().is_some()
                                || url.fragment().is_some()
                            {
                                return Err(invalid());
                            }
                            *response_remote.lock().unwrap() =
                                base.trim_end_matches('/').to_string();
                        }
                        let mut pending = response_pending.lock().unwrap();
                        if pending.as_ref().is_some_and(|setup| {
                            header_value(&header, "CSeq") == Some(setup.sequence.as_str())
                        }) {
                            let setup = pending.take().unwrap();
                            if header.starts_with("RTSP/1.0 200") {
                                header =
                                    setup.start(&header, peer, datagrams.clone(), h264_payload)?;
                                // Once media uses UDP, the control socket may be
                                // quiet until the negotiated RTSP keepalive. Let
                                // go2rtc own its liveness deadline; closing its
                                // socket or dropping Bridge still stops both sides.
                                receiver.set_read_timeout(None)?;
                                response_reader.get_ref().set_read_timeout(None)?;
                            }
                        }
                        drop(pending);
                        let body = if inband_h264
                            && header_value(&header, "Content-Type").is_some_and(|v| {
                                v.split(';')
                                    .next()
                                    .unwrap_or("")
                                    .trim()
                                    .eq_ignore_ascii_case("application/sdp")
                            }) {
                            h264_payload =
                                String::from_utf8_lossy(&body).lines().find_map(|line| {
                                    let (payload, codec) =
                                        line.strip_prefix("a=rtpmap:")?.split_once(' ')?;
                                    if codec.trim().eq_ignore_ascii_case("H264/90000") {
                                        payload.parse::<u8>().ok()
                                    } else {
                                        None
                                    }
                                });
                            inband_h264_sdp(&body)
                        } else {
                            body
                        };
                        let (h, b) = rewrite_response(
                            &header,
                            &body,
                            &response_remote.lock().unwrap(),
                            &local_response,
                        );
                        receiver.write_all(h.as_bytes())?;
                        receiver.write_all(&b)?;
                    }
                }
            }
        })();
        let _ = receiver.shutdown(Shutdown::Both);
        result
    });
    let result = (|| -> io::Result<()> {
        loop {
            match frame(&mut client_reader)? {
                Frame::Binary(bytes) => upstream.write_all(&bytes)?,
                Frame::Message(header, body) => {
                    let mut mapped = target.clone();
                    mapped.remote = remote.lock().unwrap().clone();
                    let mut request = rewrite_request(&header, &mapped, local)?;
                    if header.starts_with("SETUP ") {
                        let setup = UdpSetup::new(&request, peer)?;
                        request = setup.request(&request)?;
                        *pending.lock().unwrap() = Some(setup);
                    }
                    upstream.write_all(request.as_bytes())?;
                    upstream.write_all(&body)?;
                }
            }
        }
    })();
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = client.shutdown(Shutdown::Both);
    let _ = writer_thread.join();
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adds_cloud_headers_and_recomputes_digest_uri() {
        let t = Target::new("relay.example:554", "001122334455", "pass", "account1").unwrap();
        let h = "DESCRIBE rtsp://127.0.0.1:123/token RTSP/1.0\r\nCSeq: 2\r\nAuthorization: Digest username=\"user\", realm=\"r\", nonce=\"n\", uri=\"rtsp://127.0.0.1:123/token\", response=\"old\"\r\n\r\n";
        let rewritten = rewrite_request(h, &t, "rtsp://127.0.0.1:123/token").unwrap();
        assert!(rewritten.contains("Account: account1\r\nManual: 1"));
        assert!(rewritten.contains("DESCRIBE rtsp://relay.example:554/001122334455 RTSP/1.0"));
        assert!(!rewritten.contains("127.0.0.1"));
        assert!(!rewritten.contains("\"old\""));
        assert!(rewrite_request(h, &t, "rtsp://127.0.0.1:123/wrong").is_err());
    }
    #[test]
    fn bridge_forwards_real_socket_frames_and_stops_on_drop() {
        use std::io::Read;
        let relay = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target = Target::new(
            &relay.local_addr().unwrap().to_string(),
            "001122334455",
            "private-password",
            "resident",
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = relay.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let Frame::Message(request, _) = frame(&mut reader).unwrap() else {
                panic!("expected RTSP");
            };
            assert!(request.contains("Account: resident\r\nManual: 1"));
            let expected =
                base64::engine::general_purpose::STANDARD.encode("user:private-password");
            assert!(request.contains(&format!("Authorization: Basic {expected}")));
            assert!(request.contains("/001122334455 RTSP/1.0"));
            socket
                .write_all(
                    b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n$\x00\x00\x04rtp!",
                )
                .unwrap();
            let mut b = [0];
            assert_eq!(socket.read(&mut b).unwrap(), 0);
        });
        let bridge = Bridge::start(target).unwrap();
        assert!(!bridge.url.contains("private-password"));
        assert!(bridge.url.ends_with("#transport=udp#backchannel=0"));
        let mut url = Url::parse(bridge.url.split('#').next().unwrap()).unwrap();
        url.set_username("").unwrap();
        url.set_password(None).unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(
            client,
            "DESCRIBE {url} RTSP/1.0\r\nCSeq: 1\r\nAuthorization: Basic dXNlcjpsb2NhbA==\r\n\r\n"
        )
        .unwrap();
        let mut reader = BufReader::new(client);
        assert!(
            matches!(frame(&mut reader).unwrap(), Frame::Message(h, _) if h.starts_with("RTSP/1.0 200"))
        );
        assert!(matches!(frame(&mut reader).unwrap(), Frame::Binary(b) if b.ends_with(b"rtp!")));
        drop(bridge);
        server.join().unwrap();
    }
    #[test]
    fn udp_media_and_feedback_are_bridged_and_stopped() {
        let engine_rtp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let engine_rtcp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let relay_rtp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let relay_rtcp = UdpSocket::bind("127.0.0.1:0").unwrap();
        for s in [&engine_rtp, &engine_rtcp, &relay_rtp, &relay_rtcp] {
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        }
        let setup = UdpSetup::new(&format!(
            "SETUP rtsp://relay.example/device/trackID=0 RTSP/1.0\r\nCSeq: 3\r\nTransport: RTP/AVP;unicast;client_port={}-{}\r\n\r\n",
            engine_rtp.local_addr().unwrap().port(), engine_rtcp.local_addr().unwrap().port()
        ), IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        let mapped = setup
            .request("SETUP rtsp://relay.example/device/trackID=0 RTSP/1.0\r\nCSeq: 3\r\n\r\n")
            .unwrap();
        let proxy_ports = setup.local_ports().unwrap();
        assert_eq!(
            ports(header_value(&mapped, "Transport").unwrap(), "client_port").unwrap(),
            proxy_ports
        );
        assert!(header_value(&mapped, "X-SSRC").is_some());
        let stop = Arc::new(AtomicBool::new(false));
        let guard = StopDatagrams(stop.clone());
        let response = setup.start(&format!(
            "RTSP/1.0 200 OK\r\nCSeq: 3\r\nX-Rtp-Confuse: 0\r\nTransport: RTP/AVP;unicast;server_port={}-{}\r\n\r\n",
            relay_rtp.local_addr().unwrap().port(), relay_rtcp.local_addr().unwrap().port()
        ), IpAddr::V4(Ipv4Addr::LOCALHOST), stop, None).unwrap();
        assert_eq!(
            ports(header_value(&response, "Transport").unwrap(), "server_port").unwrap(),
            proxy_ports
        );
        let mut buffer = [0; 100];
        let (n, proxy_rtp) = relay_rtp.recv_from(&mut buffer).unwrap();
        assert_eq!(n, 12);
        assert_eq!(buffer[0], 0x80);
        let (_, proxy_rtcp) = relay_rtcp.recv_from(&mut buffer).unwrap();
        let unrelated = UdpSocket::bind("127.0.0.1:0").unwrap();
        unrelated.send_to(b"untrusted", proxy_rtp).unwrap();
        relay_rtp.send_to(b"video packet", proxy_rtp).unwrap();
        let (n, _) = engine_rtp.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], b"video packet");
        engine_rtcp.send_to(b"feedback", proxy_rtcp).unwrap();
        let (n, _) = relay_rtcp.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], b"feedback");
        drop(guard);
        std::thread::sleep(Duration::from_millis(300));
        engine_rtp
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        relay_rtp.send_to(b"stopped", proxy_rtp).unwrap();
        assert!(engine_rtp.recv_from(&mut buffer).is_err());
    }

    #[test]
    fn maps_session_specific_content_base_to_loopback() {
        let base = "rtsp://relay-address.example/temporary-session";
        let local = "rtsp://127.0.0.1:123/token";
        let (h, body) = rewrite_response(
            &format!("RTSP/1.0 200 OK\r\nContent-Base: {base}/\r\nContent-Type: application/sdp\r\nContent-Length: 0\r\n\r\n"),
            format!("a=control:{base}/trackID=0\r\n").as_bytes(), base, local,
        );
        assert_eq!(
            header_value(&h, "Content-Base"),
            Some("rtsp://127.0.0.1:123/token/")
        );
        assert!(String::from_utf8(body)
            .unwrap()
            .contains("rtsp://127.0.0.1:123/token/trackID=0"));
        let mut target = Target::new("relay.example", "001122334455", "test", "resident").unwrap();
        target.remote = base.into();
        let request = rewrite_request(
            &format!("SETUP {local}/trackID=0 RTSP/1.0\r\nCSeq: 3\r\n\r\n"),
            &target,
            local,
        )
        .unwrap();
        assert!(request.starts_with(&format!("SETUP {base}/trackID=0 RTSP/1.0")));
    }

    #[test]
    fn cloud_sdp_uses_inband_h264_without_changing_other_codecs() {
        let sdp = b"v=0\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 sprop-parameter-sets=stale;packetization-mode=1;profile-level-id=42E01F\r\na=rtpmap:97 H265/90000\r\na=fmtp:97 sprop-parameter-sets=keep;profile-level-id=keep\r\na=rtpmap:98 MPEG4-GENERIC/48000/2\r\na=fmtp:98 config=1190\r\n";
        let cleaned = inband_h264_sdp(sdp);
        let text = String::from_utf8(cleaned.clone()).unwrap();
        assert!(text.contains("a=fmtp:96 packetization-mode=1\r\n"));
        assert!(text.contains("a=fmtp:97 sprop-parameter-sets=keep;profile-level-id=keep\r\n"));
        assert!(text.contains("a=fmtp:98 config=1190\r\n"));
        assert!(!text.contains("stale"));
        let (header, body) = rewrite_response(
            "RTSP/1.0 200 OK\r\nContent-Type: application/sdp\r\nContent-Length: 999\r\n\r\n",
            &cleaned,
            "rtsp://remote",
            "rtsp://local",
        );
        assert_eq!(
            header_value(&header, "Content-Length")
                .unwrap()
                .parse::<usize>()
                .unwrap(),
            body.len()
        );
        let legacy = Target::new("relay.example", "001122334455", "test", "resident").unwrap();
        assert!(!legacy.inband_h264);
        assert!(legacy.with_inband_h264().inband_h264);
    }

    #[test]
    fn binary_and_sdp_framing() {
        let input = b"$\x00\x00\x04abcdRTSP/1.0 200 OK\r\nContent-Length: 3\r\n\r\nsdp";
        let mut reader = BufReader::new(&input[..]);
        assert!(matches!(frame(&mut reader).unwrap(), Frame::Binary(b) if b == &input[..8]));
        assert!(matches!(frame(&mut reader).unwrap(), Frame::Message(_, b) if b == b"sdp"));
        let (h, b) = rewrite_response(
            "RTSP/1.0 200 OK\r\nContent-Type: application/sdp\r\nContent-Length: 1\r\n\r\n",
            b"a=control:rtsp://cloud/device/track1",
            "rtsp://cloud/device",
            "rtsp://127.0.0.1:12/token",
        );
        assert!(h.contains(&format!("Content-Length: {}", b.len())));
        assert!(String::from_utf8(b).unwrap().contains("/token/track1"));
    }
}
