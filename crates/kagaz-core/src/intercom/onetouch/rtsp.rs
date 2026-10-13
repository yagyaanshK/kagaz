//! Loopback RTSP adapter. Adds OneTouch's Account/Manual DESCRIBE headers,
//! rewrites the relay authority, and forwards interleaved RTP without decoding.
//! go2rtc handles RTSP negotiation, H.264 and browser remuxing.
use super::md5_hex;
use base64::Engine as _;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use url::Url;

#[derive(Clone)]
pub struct Target {
    remote: String,
    host: String,
    port: u16,
    password: String,
    account: String,
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
        })
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
            url: format!("{}#transport=tcp&backchannel=0", source),
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

enum Frame {
    Binary(Vec<u8>),
    Message(String, Vec<u8>),
}
fn frame(reader: &mut impl BufRead) -> io::Result<Frame> {
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
fn parameter(header: &str, name: &str) -> Option<String> {
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
    let remote = target.remote.clone();
    let local_response = local.to_string();
    let writer_thread = std::thread::spawn(move || {
        let result = (|| -> io::Result<()> {
            loop {
                match frame(&mut response_reader)? {
                    Frame::Binary(bytes) => receiver.write_all(&bytes)?,
                    Frame::Message(header, body) => {
                        let (h, b) = rewrite_response(&header, &body, &remote, &local_response);
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
                    upstream.write_all(rewrite_request(&header, target, local)?.as_bytes())?;
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
