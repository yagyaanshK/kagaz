//! Live video and sound from a OneTouch door station without the helper
//! program: Kagaz speaks RTSP to the cloud relay itself, with the same
//! account headers and digest login the bridge adds for the engine, takes
//! the RTP the relay sends over UDP, and hands whole pictures and sound to
//! the window's own player.

use super::md5_hex;
use super::rtp::H264Rtp;
use super::rtsp::{frame, header_value, parameter, ports, Frame, Target};
use crate::intercom::g711::Law;
use crate::intercom::h264::{parse_rtp, Depacketizer};
use crate::intercom::sdp;
use crate::media::Media;
use std::io::{self, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// A running live view; dropping it ends the session.
pub struct Live {
    stop: Arc<AtomicBool>,
    sound: Arc<AtomicBool>,
}

impl Live {
    /// Start in the background. `sink` gets every picture and sound, then
    /// `Media::Ended`; it returns false to stop.
    pub fn start(target: Target, mut sink: Box<dyn FnMut(Media) -> bool + Send>) -> Live {
        let stop = Arc::new(AtomicBool::new(false));
        let sound = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let wanted = sound.clone();
        let _ = std::thread::Builder::new()
            .name("onetouch-live".into())
            .spawn(move || {
                let why = match run(&target, &flag, &wanted, &mut *sink) {
                    Ok(()) => String::new(),
                    Err(e) => e.to_string(),
                };
                log(&format!(
                    "ended: {}",
                    if why.is_empty() { "closed" } else { &why }
                ));
                if !flag.load(Ordering::Acquire) {
                    sink(Media::Ended(why));
                }
            });
        Live { stop, sound }
    }

    /// Ask for the door's sound, or let it go. The relay's monitoring
    /// offers pictures only; the official app opens the sound with its own
    /// request (`AK_OPER` with `Audio-Intercom: 1`), which rings nothing,
    /// but only for smart locks: door stations answer it with 486 (busy),
    /// with or without the session header (measured 2026-10-18), so their
    /// sound comes only in a call.
    pub fn set_sound(&self, on: bool) {
        self.sound.store(on, Ordering::Release);
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// A diary of the session in the cache, for a view without a picture.
pub fn log(line: &str) {
    let path = crate::paths::cache_dir().join("onetouch-live.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "{now} {line}");
    }
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

struct Rtsp {
    tcp: TcpStream,
    reader: BufReader<TcpStream>,
    cseq: u32,
    challenge: Option<String>,
    session: Option<String>,
    /// The session's timeout in seconds, as the relay says.
    timeout: u64,
    target: Target,
}

impl Rtsp {
    /// One request and its answer; a login challenge is answered once.
    fn request(&mut self, method: &str, uri: &str, extra: &[String]) -> io::Result<(String, Vec<u8>)> {
        let mut tried_auth = false;
        loop {
            self.cseq += 1;
            let mut head = format!(
                "{method} {uri} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: Kagaz\r\n",
                self.cseq
            );
            if let Some(ch) = &self.challenge {
                head.push_str(&format!(
                    "Authorization: {}\r\n",
                    authorization(ch, method, uri, &self.target.password)?
                ));
            }
            if let Some(s) = &self.session {
                head.push_str(&format!("Session: {s}\r\n"));
            }
            for e in extra {
                head.push_str(e);
                head.push_str("\r\n");
            }
            if method == "DESCRIBE" {
                head.push_str(&format!("Account: {}\r\nManual: 1\r\n", self.target.account));
            }
            head.push_str("\r\n");
            self.tcp.write_all(head.as_bytes())?;
            let (header, body) = loop {
                match frame(&mut self.reader)? {
                    Frame::Message(h, b) => break (h, b),
                    Frame::Binary(_) => continue,
                }
            };
            let status = header
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|c| c.parse::<u16>().ok())
                .unwrap_or(0);
            log(&format!("{method} -> {status}"));
            if status == 401 && !tried_auth {
                tried_auth = true;
                let ch = header_value(&header, "WWW-Authenticate")
                    .ok_or_else(|| invalid("the relay sent no login challenge"))?;
                if !ch.starts_with("Digest ") {
                    return Err(invalid("the relay wants a login Kagaz does not do"));
                }
                self.challenge = Some(ch.to_string());
                continue;
            }
            if status != 200 {
                return Err(io::Error::other(format!(
                    "the relay answered {method} with {status}"
                )));
            }
            if self.session.is_none() {
                if let Some(s) = header_value(&header, "Session") {
                    let mut parts = s.split(';');
                    self.session = parts.next().map(|id| id.trim().to_string());
                    if let Some(t) = parts
                        .find_map(|p| p.trim().strip_prefix("timeout="))
                        .and_then(|t| t.trim().parse::<u64>().ok())
                    {
                        self.timeout = t;
                    }
                }
            }
            return Ok((header, body));
        }
    }
}

/// The digest login the relay expects (user "user", the device's password).
fn authorization(challenge: &str, method: &str, uri: &str, password: &str) -> io::Result<String> {
    let realm = parameter(challenge, "realm").ok_or_else(|| invalid("bad login challenge"))?;
    let nonce = parameter(challenge, "nonce").ok_or_else(|| invalid("bad login challenge"))?;
    let ha1 = md5_hex(&format!("user:{realm}:{password}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let (response, tail) = match parameter(challenge, "qop") {
        Some(qop) if qop.split(',').any(|q| q.trim() == "auth") => {
            let cnonce = uuid::Uuid::new_v4().simple().to_string();
            let nc = "00000001";
            (
                md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}")),
                format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""),
            )
        }
        Some(_) => return Err(invalid("the relay wants a login Kagaz does not do")),
        None => (md5_hex(&format!("{ha1}:{nonce}:{ha2}")), String::new()),
    };
    Ok(format!(
        "Digest username=\"user\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\"{tail}"
    ))
}

/// The `a=control:` lines of an SDP: the session's, then each media's.
struct Controls {
    session: Option<String>,
    media: Vec<(String, Option<String>)>,
}

fn controls(sdp: &str) -> Controls {
    let mut c = Controls {
        session: None,
        media: Vec::new(),
    };
    for line in sdp.lines().map(str::trim) {
        if let Some(m) = line.strip_prefix("m=") {
            c.media
                .push((m.split_whitespace().next().unwrap_or("").to_string(), None));
        } else if let Some(v) = line.strip_prefix("a=control:") {
            match c.media.last_mut() {
                Some(m) => m.1 = Some(v.trim().to_string()),
                None => c.session = Some(v.trim().to_string()),
            }
        }
    }
    c
}

/// A control URL against the session's base.
fn resolve(base: &str, control: Option<&str>) -> String {
    match control {
        None | Some("*") => base.to_string(),
        Some(c) if c.starts_with("rtsp://") => c.to_string(),
        Some(c) => format!("{}/{}", base.trim_end_matches('/'), c.trim_start_matches('/')),
    }
}

/// The H.264 payload type of a video stream and its out-of-band parameter sets.
fn h264_of(s: &sdp::Stream) -> Option<(u8, Option<String>)> {
    let pt = s.rtpmap.iter().find_map(|(pt, enc)| {
        enc.split('/')
            .next()
            .is_some_and(|n| n.eq_ignore_ascii_case("H264"))
            .then_some(*pt)
    })?;
    let sprop = s.fmtp.iter().find(|(p, _)| *p == pt).and_then(|(_, f)| {
        f.split(';').find_map(|param| {
            param
                .trim()
                .strip_prefix("sprop-parameter-sets=")
                .map(str::to_string)
        })
    });
    Some((pt, sprop))
}

/// The G.711 payload type of an audio stream, and which law.
fn g711_of(s: &sdp::Stream) -> Option<(u8, Law)> {
    s.payloads.iter().copied().find_map(|pt| {
        let name = s
            .rtpmap
            .iter()
            .find(|(p, _)| *p == pt)
            .map(|(_, e)| e.split('/').next().unwrap_or("").to_ascii_uppercase());
        match name.as_deref() {
            Some("PCMU") => Some((pt, Law::Mu)),
            Some("PCMA") => Some((pt, Law::A)),
            None => Law::from_payload_type(pt).map(|law| (pt, law)),
            _ => None,
        }
    })
}

/// One media track over UDP: our two sockets and where the relay sends from.
struct Track {
    sockets: [UdpSocket; 2],
}

fn setup(rtsp: &mut Rtsp, control: &str, peer: IpAddr) -> io::Result<Track> {
    let sockets = udp_pair(peer)?;
    let local = [
        sockets[0].local_addr()?.port(),
        sockets[1].local_addr()?.port(),
    ];
    let random = uuid::Uuid::new_v4();
    let ssrc = 100_000_000
        + u32::from_be_bytes(random.as_bytes()[..4].try_into().unwrap()) % 900_000_000;
    let (head, _) = rtsp.request(
        "SETUP",
        control,
        &[
            format!(
                "Transport: RTP/AVP;unicast;client_port={}-{}",
                local[0], local[1]
            ),
            format!("X-SSRC: {ssrc}"),
        ],
    )?;
    if header_value(&head, "X-Rtp-Confuse").is_some_and(|v| v != "0") {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this device's video is scrambled, which Kagaz cannot play",
        ));
    }
    let transport = header_value(&head, "Transport").ok_or_else(|| invalid("no transport"))?;
    let server = ports(transport, "server_port")?;
    // Open the NAT mapping from our ports, as the phone does.
    probe(&sockets, peer, server, 96, ssrc);
    Ok(Track { sockets })
}

/// Open or close the door's sound on a monitoring session, as the
/// official app does: its own `AK_OPER` request on the monitor address,
/// with our audio ports and no session header. The answer's Transport
/// says where the relay takes our sound (`server_port`).
fn intercom(rtsp: &mut Rtsp, uri: &str, local: [u16; 2], on: bool) -> io::Result<String> {
    let session = rtsp.session.take();
    let answer = rtsp.request(
        "AK_OPER",
        uri,
        &[
            format!(
                "Transport: RTP/AVP;unicast;client_port={}-{}",
                local[0], local[1]
            ),
            format!("Audio-Intercom:{}", u8::from(on)),
        ],
    );
    rtsp.session = session;
    let (head, _) = answer?;
    for line in head.lines().skip(1) {
        let name = line.split(':').next().unwrap_or("").trim();
        if !matches!(name, "CSeq" | "Date" | "Server" | "Session" | "") {
            log(&format!("sound answer: {line}"));
        }
    }
    Ok(header_value(&head, "Transport").unwrap_or("").to_string())
}

/// The sound's sockets, read in the background: G.711 by its payload type.
fn sound_reader(
    sockets: [UdpSocket; 2],
    only: Option<(u8, Law)>,
    tx: mpsc::Sender<Media>,
    flag: Arc<AtomicBool>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("onetouch-live-audio".into())
        .spawn(move || {
            let _keep = &sockets[1];
            let socket = &sockets[0];
            let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
            let mut buffer = [0u8; 4096];
            let mut first = true;
            while !flag.load(Ordering::Acquire) {
                match socket.recv_from(&mut buffer) {
                    Ok((n, _)) => {
                        let Some((_, ts, _, payload)) = parse_rtp(&buffer[..n]) else {
                            continue;
                        };
                        let pt = buffer[1] & 0x7F;
                        let law = match only {
                            Some((want, law)) if want == pt => law,
                            Some(_) => continue,
                            None => match Law::from_payload_type(pt) {
                                Some(law) => law,
                                None => continue,
                            },
                        };
                        if payload.is_empty() {
                            continue;
                        }
                        if first {
                            first = false;
                            log(&format!("first sound, pt {pt}"));
                        }
                        if tx
                            .send(Media::Audio {
                                pcm: law.decode(payload),
                                timestamp: ts,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(e) if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                    Err(_) => break,
                }
            }
        })?;
    Ok(())
}

fn udp_pair(peer: IpAddr) -> io::Result<[UdpSocket; 2]> {
    let bind_ip = if peer.is_ipv4() {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    };
    Ok([UdpSocket::bind((bind_ip, 0))?, UdpSocket::bind((bind_ip, 0))?])
}

fn probe(sockets: &[UdpSocket; 2], peer: IpAddr, server: [u16; 2], pt: u8, ssrc: u32) {
    for (i, socket) in sockets.iter().enumerate() {
        let mut packet = [0u8; 12];
        packet[0] = 0x80;
        packet[1] = pt;
        packet[8..12].copy_from_slice(&ssrc.to_be_bytes());
        let _ = socket.send_to(&packet, SocketAddr::new(peer, server[i]));
    }
}

fn run(
    target: &Target,
    stop: &AtomicBool,
    sound: &AtomicBool,
    sink: &mut dyn FnMut(Media) -> bool,
) -> io::Result<()> {
    log(&format!("start {}:{}", target.host, target.port));
    let addresses = crate::dns::resolve(&target.host, target.port)?;
    let mut tcp = None;
    for address in addresses.iter().take(4) {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Ok(s) = TcpStream::connect_timeout(address, Duration::from_secs(5)) {
            tcp = Some(s);
            break;
        }
    }
    let tcp = tcp.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "the OneTouch video relay did not answer",
        )
    })?;
    tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(10)))?;
    let peer = tcp.peer_addr()?.ip();
    let reader = BufReader::new(tcp.try_clone()?);
    let mut rtsp = Rtsp {
        tcp,
        reader,
        cseq: 0,
        challenge: None,
        session: None,
        timeout: 60,
        target: target.clone(),
    };
    let (head, body) = rtsp.request(
        "DESCRIBE",
        &target.remote,
        &["Accept: application/sdp".to_string()],
    )?;
    let text = String::from_utf8_lossy(&body).to_string();
    let base = header_value(&head, "Content-Base")
        .map(|b| b.trim_end_matches('/').to_string())
        .unwrap_or_else(|| target.remote.clone());
    // The diary keeps what the relay offers (media lines only: no addresses).
    for line in text.lines().map(str::trim) {
        if line.starts_with("m=") || line.starts_with("a=rtpmap") || line.starts_with("a=control") {
            log(&format!("offer {line}"));
        }
    }
    let desc = sdp::parse(&text);
    let ctl = controls(&text);
    let control_of = |kind: &str| {
        ctl.media
            .iter()
            .find(|(k, _)| k == kind)
            .and_then(|(_, c)| c.as_deref())
    };
    let video = desc
        .video
        .as_ref()
        .ok_or_else(|| invalid("the relay offers no video for this device"))?;
    let (video_pt, sprop) =
        h264_of(video).ok_or_else(|| invalid("the relay's video is not H.264"))?;
    let video_track = setup(&mut rtsp, &resolve(&base, control_of("video")), peer)?;
    let audio = desc.audio.as_ref().and_then(g711_of);
    let audio_track = match audio {
        Some(_) => Some(setup(&mut rtsp, &resolve(&base, control_of("audio")), peer)?),
        None => None,
    };
    let play_uri = resolve(&base, ctl.session.as_deref());
    rtsp.request("PLAY", &play_uri, &["Range: npt=0.000-".to_string()])?;
    log(&format!(
        "playing: video pt {video_pt}, sprop {}, sound {}",
        sprop.is_some(),
        audio.map(|(pt, law)| format!("pt {pt} {law:?}")).unwrap_or("none".into())
    ));

    let (tx, rx) = mpsc::channel::<Media>();
    let readers_stop = Arc::new(AtomicBool::new(false));
    {
        let tx = tx.clone();
        let flag = readers_stop.clone();
        let Track { sockets } = video_track;
        std::thread::Builder::new()
            .name("onetouch-live-video".into())
            .spawn(move || {
                let _keep = &sockets[1]; // the RTCP mapping stays open
                let socket = &sockets[0];
                let _ = socket.set_read_timeout(Some(Duration::from_millis(20)));
                let mut reorder = H264Rtp::new(video_pt);
                let mut pictures = Depacketizer::new();
                if let Some(sp) = &sprop {
                    pictures.set_parameter_sets(sp);
                }
                let mut buffer = [0u8; 65535];
                let mut first = true;
                while !flag.load(Ordering::Acquire) {
                    let packets = match socket.recv_from(&mut buffer) {
                        Ok((n, _)) => reorder.push(&buffer[..n], Instant::now()),
                        Err(e) if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                        {
                            reorder.poll(Instant::now())
                        }
                        Err(_) => break,
                    };
                    for p in packets {
                        if p.len() < 12 || p[1] & 0x7F != video_pt {
                            continue;
                        }
                        if let Some(unit) = pictures.push(&p) {
                            if first {
                                first = false;
                                log("first picture");
                            }
                            if tx
                                .send(Media::Video {
                                    data: unit.data,
                                    key: unit.key,
                                    timestamp: unit.timestamp,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
                let s = &pictures.stats;
                log(&format!(
                    "video: {} packets, {} pictures ({} key), {} damaged, {} waiting",
                    s.packets, s.units, s.keyframes, s.damaged, s.waiting
                ));
            })?;
    }
    if let (Some(only), Some(Track { sockets })) = (audio, audio_track) {
        sound_reader(sockets, Some(only), tx.clone(), readers_stop.clone())?;
    }
    // Without sound in the offer, sound comes on request (see `intercom`):
    // its ports are ready, and their reader runs, from the start.
    let mut door_sound = None;
    if audio.is_none() {
        let sockets = udp_pair(peer)?;
        let local = [sockets[0].local_addr()?.port(), sockets[1].local_addr()?.port()];
        let probes = [sockets[0].try_clone()?, sockets[1].try_clone()?];
        sound_reader(sockets, None, tx.clone(), readers_stop.clone())?;
        door_sound = Some((probes, local));
    }
    let mut sound_on = false;
    let mut sound_server: Option<[u16; 2]> = None;
    drop(tx);

    let keepalive = Duration::from_secs((rtsp.timeout / 2).clamp(10, 25));
    let mut last = Instant::now();
    let result = loop {
        if stop.load(Ordering::Acquire) {
            break Ok(());
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(m) => {
                if !sink(m) {
                    break Ok(());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(io::Error::other("the relay stopped sending"));
            }
        }
        let wanted = sound.load(Ordering::Acquire);
        if wanted != sound_on {
            sound_on = wanted;
            if let Some((sockets, local)) = &door_sound {
                match intercom(&mut rtsp, &target.remote, *local, wanted) {
                    Ok(transport) => {
                        sound_server = ports(&transport, "server_port").ok();
                        if let (true, Some(server)) = (wanted, sound_server) {
                            probe(sockets, peer, server, 0, 0);
                        }
                    }
                    Err(e) => log(&format!("sound request failed: {e}")),
                }
            }
        }
        if last.elapsed() >= keepalive {
            last = Instant::now();
            if let Err(e) = rtsp.request("OPTIONS", &play_uri, &[]) {
                break Err(e);
            }
            if let (true, Some((sockets, _)), Some(server)) = (sound_on, &door_sound, sound_server) {
                probe(sockets, peer, server, 0, 0);
            }
        }
    };
    readers_stop.store(true, Ordering::Release);
    let _ = rtsp.tcp.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = rtsp.tcp.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = rtsp.request("TEARDOWN", &play_uri, &[]);
    let _ = rtsp.tcp.shutdown(Shutdown::Both);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_controls_and_resolves_them() {
        let sdp = "v=0\r\na=control:*\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z0LgH6o=,aM48gA==\r\na=control:trackID=0\r\nm=audio 0 RTP/AVP 8\r\na=control:rtsp://relay.example.com/x/trackID=1\r\n";
        let c = controls(sdp);
        assert_eq!(c.session.as_deref(), Some("*"));
        assert_eq!(c.media[0].1.as_deref(), Some("trackID=0"));
        let base = "rtsp://relay.example.com/x";
        assert_eq!(resolve(base, c.session.as_deref()), base);
        assert_eq!(resolve(base, c.media[0].1.as_deref()), "rtsp://relay.example.com/x/trackID=0");
        assert_eq!(resolve(base, c.media[1].1.as_deref()), "rtsp://relay.example.com/x/trackID=1");
        let d = sdp::parse(sdp);
        let (pt, sprop) = h264_of(d.video.as_ref().unwrap()).unwrap();
        assert_eq!(pt, 96);
        assert_eq!(sprop.as_deref(), Some("Z0LgH6o=,aM48gA=="));
        assert_eq!(g711_of(d.audio.as_ref().unwrap()), Some((8, Law::A)));
    }

    #[test]
    fn answers_a_digest_challenge() {
        let a = authorization(
            "Digest realm=\"relay\", nonce=\"abc\"",
            "DESCRIBE",
            "rtsp://relay.example.com/001122334455",
            "secret",
        )
        .unwrap();
        assert!(a.starts_with("Digest username=\"user\", realm=\"relay\", nonce=\"abc\""));
        assert!(a.contains("response=\""));
        assert!(!a.contains("qop"));
        let b = authorization("Digest realm=\"r\", nonce=\"n\", qop=\"auth\"", "PLAY", "rtsp://x/y", "p").unwrap();
        assert!(b.contains("qop=auth, nc=00000001, cnonce="));
    }
}
