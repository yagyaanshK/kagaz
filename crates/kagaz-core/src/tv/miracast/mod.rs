//! Miracast (Wi-Fi Display) from this computer to a TV, over the ordinary
//! network (MS-MICE) or over a Wi-Fi Direct link made first.
//!
//! Kagaz is the source: it runs the RTSP server the TV connects to, agrees
//! one picture size, and sends H.264 in an MPEG transport stream over RTP.
//! No HDCP: a TV that demands it ends the session, and Kagaz says so.

pub mod mice;
pub mod rtsp;
pub mod ts;
pub mod video;

use rtsp::{Message, Mode, SinkAbilities};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The RTSP port Wi-Fi Display uses, and the one Kagaz announces in its
/// Wi-Fi Direct information.
pub const RTSP_PORT: u16 = 7236;

/// How the TV is reached.
#[derive(Debug, Clone, Copy)]
pub enum Route {
    /// Over the ordinary network: Kagaz asks the TV at this address to
    /// connect back (MS-MICE).
    Network(IpAddr),
    /// Over a Wi-Fi Direct link already made: the TV connects on its own.
    Direct,
}

/// Where the pictures come from: the test pattern, or this computer's
/// screen.
pub trait Pictures {
    /// The picture due at `due` since the start, as RGBA at this size.
    fn picture(&mut self, due: Duration, width: usize, height: usize) -> &[u8];
}

impl Pictures for video::Pattern {
    fn picture(&mut self, due: Duration, width: usize, height: usize) -> &[u8] {
        if self.width != width || self.height != height {
            *self = video::Pattern::new(width, height);
        }
        self.draw(due.as_millis() as u64)
    }
}

#[cfg(target_os = "linux")]
impl Pictures for super::capture::Screen {
    fn picture(&mut self, _due: Duration, width: usize, height: usize) -> &[u8] {
        super::capture::Screen::picture(self, width, height)
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// The name the TV shows for this computer.
    pub name: String,
    /// The tallest picture to send: 1080 or 720.
    pub max_height: u32,
    /// Stop after this long; `None` runs until `stop` is set.
    pub length: Option<Duration>,
    /// How long to wait for the TV to connect.
    pub wait: Duration,
}

/// What happened, as it happens.
#[derive(Debug, Clone)]
pub enum Event {
    /// Waiting for the TV to connect.
    Waiting,
    /// The TV connected from this address.
    Connected(IpAddr),
    /// What the TV says it can take.
    Abilities(SinkAbilities),
    /// The TV is playing the stream.
    Playing(Mode),
    /// Once a second while playing.
    Progress { seconds: u64, frames: u64, kilobits_per_second: u64 },
    /// The TV asked for a fresh key frame.
    KeyFrameAsked,
}

/// How a session went.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub abilities: Option<SinkAbilities>,
    pub mode: Option<Mode>,
    pub frames: u64,
    pub bytes: u64,
    /// Who ended it: "you", "the TV" or "time".
    pub ended_by: String,
}

struct Link {
    tcp: TcpStream,
    buf: Vec<u8>,
    cseq: u32,
}

impl Link {
    fn send(&mut self, m: &Message) -> Result<(), String> {
        self.tcp.write_all(&m.to_bytes()).map_err(|e| format!("the TV's connection broke: {e}"))
    }

    fn request(&mut self, method: &str, url: &str) -> Message {
        self.cseq += 1;
        Message::request(method, url, self.cseq)
    }

    /// Reads what has arrived, waiting at most `wait`. `Ok(None)` when
    /// nothing whole has come yet.
    fn next(&mut self, wait: Duration) -> Result<Option<Message>, String> {
        if let Some(m) = rtsp::take_message(&mut self.buf) {
            return Ok(Some(m));
        }
        self.tcp.set_read_timeout(Some(wait.max(Duration::from_millis(1)))).ok();
        let mut chunk = [0u8; 16 * 1024];
        match self.tcp.read(&mut chunk) {
            Ok(0) => Err("the TV closed the connection".into()),
            Ok(n) => {
                self.buf.extend_from_slice(&chunk[..n]);
                Ok(rtsp::take_message(&mut self.buf))
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(format!("the TV's connection broke: {e}")),
        }
    }
}

/// What the session's requests from the TV changed.
#[derive(Default)]
struct Asked {
    rtp_port: Option<u16>,
    playing: bool,
    teardown: bool,
    key_frame: bool,
}

const PUBLIC: &str = "org.wfa.wfd1.0, SETUP, TEARDOWN, PLAY, PAUSE, GET_PARAMETER, SET_PARAMETER";

/// Answers one request from the TV.
fn answer(link: &mut Link, m: &Message, session: &str, udp_port: u16, asked: &mut Asked) -> Result<(), String> {
    let mut reply = Message::reply(m.cseq());
    match m.method() {
        "OPTIONS" => reply = reply.header("Public", PUBLIC),
        "SETUP" => {
            let transport = m.get("Transport").unwrap_or("");
            if let Some(port) = transport
                .split(';')
                .find_map(|p| p.trim().strip_prefix("client_port="))
                .and_then(|p| p.split('-').next())
                .and_then(|p| p.parse().ok())
            {
                asked.rtp_port = Some(port);
            }
            let client = asked.rtp_port.unwrap_or(0);
            reply = reply.header("Session", format!("{session};timeout=30")).header(
                "Transport",
                format!("RTP/AVP/UDP;unicast;client_port={client};server_port={udp_port}"),
            );
        }
        "PLAY" => {
            asked.playing = true;
            reply = reply.header("Session", session).header("Range", "npt=now-");
        }
        "PAUSE" => {
            asked.playing = false;
            reply = reply.header("Session", session);
        }
        "TEARDOWN" => {
            asked.teardown = true;
            reply = reply.header("Session", session);
        }
        "SET_PARAMETER" => {
            if m.body.contains("wfd_idr_request") {
                asked.key_frame = true;
            }
        }
        // GET_PARAMETER with no body is the TV's keep-alive.
        _ => {}
    }
    link.send(&reply)
}

/// The address this computer uses towards `peer`.
fn local_address_towards(peer: IpAddr) -> Option<IpAddr> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    s.connect((peer, 9)).ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(16) {
        let id = uuid::Uuid::new_v4();
        chunk.copy_from_slice(&id.as_bytes()[..chunk.len()]);
    }
    out
}

/// Mirrors to the TV until `stop` is set, the TV ends it, or the length
/// runs out.
///
/// `ready` runs once this computer listens for the TV and before waiting
/// for it: for Wi-Fi Direct it makes the link (the TV connects as soon as
/// the link is up). What it returns is kept until the session ends.
pub fn mirror<R>(
    route: Route,
    opts: &Options,
    pictures: &mut dyn Pictures,
    stop: &AtomicBool,
    ready: impl FnOnce() -> Result<R, String>,
    mut event: impl FnMut(Event),
) -> Result<Report, String> {
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, RTSP_PORT))
        .or_else(|_| match route {
            // Over the network the port is announced, so any will do.
            Route::Network(_) => TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)),
            Route::Direct => Err(std::io::Error::new(ErrorKind::AddrInUse, "")),
        })
        .map_err(|_| format!("port {RTSP_PORT} on this computer is in use by another program"))?;
    let rtsp_port = listener.local_addr().map(|a| a.port()).unwrap_or(RTSP_PORT);
    let source_id: [u8; 16] = random_bytes();
    let _kept = ready()?;

    let mut mice = None;
    if let Route::Network(tv) = route {
        let mut s = TcpStream::connect_timeout(&SocketAddr::new(tv, mice::PORT), Duration::from_secs(5))
            .map_err(|e| format!("the TV does not take screen mirroring over the network (port {}): {e}", mice::PORT))?;
        s.write_all(&mice::source_ready(&opts.name, rtsp_port, &source_id))
            .map_err(|e| format!("the TV dropped the connection: {e}"))?;
        mice = Some(s);
    }
    let finish_mice = |mice: &mut Option<TcpStream>| {
        if let Some(mut s) = mice.take() {
            let _ = s.write_all(&mice::stop_projection(&opts.name, &source_id));
        }
    };

    event(Event::Waiting);
    listener.set_nonblocking(true).ok();
    let deadline = Instant::now() + opts.wait;
    let (tcp, peer) = loop {
        match listener.accept() {
            Ok(x) => break x,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if stop.load(Ordering::Relaxed) || Instant::now() > deadline {
                    finish_mice(&mut mice);
                    return Err("the TV did not connect. Is its screen sharing on, and did it ask to allow this computer?".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                finish_mice(&mut mice);
                return Err(format!("waiting for the TV failed: {e}"));
            }
        }
    };
    tcp.set_nonblocking(false).ok();
    tcp.set_nodelay(true).ok();
    let tv_ip = peer.ip();
    event(Event::Connected(tv_ip));
    let here = tcp.local_addr().map(|a| a.ip()).ok().or_else(|| local_address_towards(tv_ip));
    let presentation = format!(
        "rtsp://{}/wfd1.0/streamid=0",
        here.map(|a| a.to_string()).unwrap_or_else(|| "localhost".into())
    );

    let mut link = Link { tcp, buf: Vec::new(), cseq: 0 };
    let session = format!("{:08}", u32::from_be_bytes(random_bytes()) % 100_000_000);
    let udp = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| format!("no UDP port: {e}"))?;
    let udp_port = udp.local_addr().map(|a| a.port()).unwrap_or(0);
    let mut asked = Asked::default();
    let mut report = Report::default();

    let result = (|| -> Result<(), String> {
        // M1: our OPTIONS. The TV replies, then sends its own (M2).
        let m1 = link.request("OPTIONS", "*").header("Require", "org.wfa.wfd1.0");
        let m1_seq = m1.cseq().to_string();
        link.send(&m1)?;
        let mut waiting_for = Some(m1_seq);
        let mut step = 1;
        let mut mode = rtsp::MODES[3];
        let handshake_end = Instant::now() + Duration::from_secs(15);
        while !asked.playing {
            if Instant::now() > handshake_end || stop.load(Ordering::Relaxed) {
                return Err(format!("the TV stopped answering during the setup (step {step})"));
            }
            let Some(m) = link.next(Duration::from_millis(200))? else { continue };
            if !m.is_reply() {
                answer(&mut link, &m, &session, udp_port, &mut asked)?;
                if asked.teardown {
                    return Err("the TV ended the session during the setup".into());
                }
                continue;
            }
            if waiting_for.as_deref() != Some(m.cseq()) {
                continue;
            }
            if m.status() != 200 {
                return Err(format!(
                    "the TV refused step {step} of the setup ({})",
                    m.first.trim_start_matches("RTSP/1.0 ")
                ));
            }
            match step {
                1 => {
                    // M3: what can you take?
                    let mut body = rtsp::ASKED.join("\r\n");
                    body.push_str("\r\n");
                    let m3 = link.request("GET_PARAMETER", "rtsp://localhost/wfd1.0").with_parameters(body);
                    waiting_for = Some(m3.cseq().to_string());
                    link.send(&m3)?;
                    step = 3;
                }
                3 => {
                    let a = rtsp::sink_abilities(&m.body);
                    if a.profiles.is_empty() {
                        return Err("the TV named no H.264 picture it can show".into());
                    }
                    mode = rtsp::choose_mode(&a, opts.max_height);
                    let port = if a.rtp_port != 0 { a.rtp_port } else { 1028 };
                    asked.rtp_port = Some(port);
                    event(Event::Abilities(a.clone()));
                    report.abilities = Some(a);
                    report.mode = Some(mode);
                    // M4: this is the stream.
                    let m4 = link
                        .request("SET_PARAMETER", "rtsp://localhost/wfd1.0")
                        .with_parameters(rtsp::stream_parameters(mode, port, &presentation));
                    waiting_for = Some(m4.cseq().to_string());
                    link.send(&m4)?;
                    step = 4;
                }
                4 => {
                    // M5: start. The TV answers with SETUP (M6) and PLAY (M7).
                    let m5 = link
                        .request("SET_PARAMETER", "rtsp://localhost/wfd1.0")
                        .with_parameters("wfd_trigger_method: SETUP\r\n".into());
                    waiting_for = Some(m5.cseq().to_string());
                    link.send(&m5)?;
                    step = 5;
                }
                _ => waiting_for = None,
            }
        }

        let target = SocketAddr::new(tv_ip, asked.rtp_port.unwrap_or(1028));
        event(Event::Playing(mode));
        let bitrate = match (mode.height > 720, mode.fps > 30) {
            (true, true) => 16_000_000,
            (true, false) => 10_000_000,
            (false, true) => 8_000_000,
            (false, false) => 6_000_000,
        };
        let (w, h) = (mode.width as usize, mode.height as usize);
        let mut encoder = video::Video::new(w, h, mode.fps, bitrate)?;
        let mut muxer = ts::Muxer::new();
        let mut rtp = ts::Rtp::new(u32::from_be_bytes(random_bytes()));
        let start = Instant::now();
        let frame_time = Duration::from_secs_f64(1.0 / mode.fps as f64);
        let mut next_frame = start;
        let mut last_keepalive = start;
        let mut last_progress = start;
        let mut bytes_at_progress = 0u64;
        loop {
            if stop.load(Ordering::Relaxed) {
                report.ended_by = "you".into();
                break;
            }
            if opts.length.is_some_and(|l| start.elapsed() >= l) {
                report.ended_by = "time".into();
                break;
            }
            // Requests from the TV between pictures.
            let wait = next_frame.saturating_duration_since(Instant::now());
            if let Some(m) = link.next(wait)? {
                if !m.is_reply() {
                    answer(&mut link, &m, &session, udp_port, &mut asked)?;
                    if asked.teardown {
                        report.ended_by = "the TV".into();
                        return Ok(());
                    }
                    if std::mem::take(&mut asked.key_frame) {
                        encoder.key_next();
                        event(Event::KeyFrameAsked);
                    }
                }
                continue;
            }
            if Instant::now() < next_frame {
                continue;
            }
            // Each picture is stamped with the time it was due, not the
            // moment it happened to be drawn: the TV gets exactly even steps.
            let due = next_frame - start;
            next_frame += frame_time;
            if Instant::now() > next_frame + frame_time * 4 {
                // Fell behind (a slow machine): drop the backlog.
                next_frame = Instant::now() + frame_time;
            }
            let rgba = pictures.picture(due, w, h);
            let picture = encoder.encode(rgba)?;
            if picture.data.is_empty() {
                continue;
            }
            let pts90 = (due.as_micros() as u64 * 9 / 100) + ts::PTS_AHEAD;
            // The clock reference is the real time it leaves.
            let pcr27 = start.elapsed().as_micros() as u64 * 27;
            let packets = muxer.frame(&picture.data, picture.key, pcr27, pts90);
            if asked.playing {
                for p in rtp.packets(&packets, pts90) {
                    report.bytes += p.len() as u64;
                    // A full Wi-Fi queue drops a packet; the next key frame mends it.
                    let _ = udp.send_to(&p, target);
                }
            }
            report.frames += 1;
            if last_keepalive.elapsed() >= Duration::from_secs(20) {
                let keep = link.request("GET_PARAMETER", "rtsp://localhost/wfd1.0").header("Session", session.clone());
                link.send(&keep)?;
                last_keepalive = Instant::now();
            }
            if last_progress.elapsed() >= Duration::from_secs(1) {
                let secs = last_progress.elapsed().as_secs_f64();
                event(Event::Progress {
                    seconds: start.elapsed().as_secs(),
                    frames: report.frames,
                    kilobits_per_second: ((report.bytes - bytes_at_progress) as f64 * 8.0 / 1000.0 / secs) as u64,
                });
                bytes_at_progress = report.bytes;
                last_progress = Instant::now();
            }
        }
        // Ask the TV to end the session (M5 with TEARDOWN), and answer its
        // TEARDOWN.
        let end = link
            .request("SET_PARAMETER", "rtsp://localhost/wfd1.0")
            .with_parameters("wfd_trigger_method: TEARDOWN\r\n".into());
        link.send(&end)?;
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until && !asked.teardown {
            match link.next(Duration::from_millis(100)) {
                Ok(Some(m)) if !m.is_reply() => answer(&mut link, &m, &session, udp_port, &mut asked)?,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        Ok(())
    })();
    finish_mice(&mut mice);
    result.map(|_| report)
}

#[cfg(test)]
mod tests_support {
    pub use super::tests::*;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const LG: &str = include_str!("../../../tests/fixtures/tv/lg-webos-2024-wfd-m3.txt");

    /// Sessions listen on port 7236, so the tests that run one take turns.
    pub static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    /// A stand-in TV acting as the LG did: answers the setup, asks for SETUP
    /// and PLAY, takes the stream over UDP and ends when asked.
    pub fn stand_in_tv(received: Arc<Mutex<Vec<Vec<u8>>>>) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
            let port = udp.local_addr().unwrap().port();
            udp.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            let r2 = received.clone();
            std::thread::spawn(move || {
                let start = Instant::now();
                let mut buf = [0u8; 2048];
                while start.elapsed() < Duration::from_secs(6) {
                    if let Ok(n) = udp.recv(&mut buf) {
                        r2.lock().unwrap().push(buf[..n].to_vec());
                    }
                }
            });
            std::thread::sleep(Duration::from_millis(50));
            let tcp = TcpStream::connect(("127.0.0.1", RTSP_PORT)).unwrap();
            let mut link = Link { tcp, buf: Vec::new(), cseq: 100 };
            let mut seen = Vec::new();
            let mut setup_cseq = String::new();
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(8) {
                let m = match link.next(Duration::from_millis(100)) {
                    Ok(Some(m)) => m,
                    Ok(None) => continue,
                    Err(_) => break,
                };
                seen.push(if m.is_reply() { format!("reply {}", m.status()) } else { m.method().to_string() });
                if m.is_reply() {
                    if m.cseq() == setup_cseq {
                        // Our SETUP was answered: play.
                        let play = link.request("PLAY", "rtsp://127.0.0.1/wfd1.0/streamid=0");
                        link.send(&play).unwrap();
                    }
                    continue;
                }
                let mut reply = Message::reply(m.cseq());
                match m.method() {
                    "OPTIONS" => reply = reply.header("Public", "org.wfa.wfd1.0, GET_PARAMETER, SET_PARAMETER"),
                    "GET_PARAMETER" if !m.body.is_empty() => {
                        reply = reply.with_parameters(LG.replace("unicast 53000", &format!("unicast {port}")))
                    }
                    _ => {}
                }
                link.send(&reply).unwrap();
                if m.method() == "OPTIONS" {
                    let m2 = link.request("OPTIONS", "*").header("Require", "org.wfa.wfd1.0");
                    link.send(&m2).unwrap();
                }
                if m.body.contains("wfd_trigger_method: SETUP") {
                    let setup = link
                        .request("SETUP", "rtsp://127.0.0.1/wfd1.0/streamid=0")
                        .header("Transport", format!("RTP/AVP/UDP;unicast;client_port={port}"));
                    setup_cseq = setup.cseq().to_string();
                    link.send(&setup).unwrap();
                }
                if m.body.contains("wfd_trigger_method: TEARDOWN") {
                    let td = link.request("TEARDOWN", "rtsp://127.0.0.1/wfd1.0/streamid=0");
                    link.send(&td).unwrap();
                }
            }
            seen
        })
    }

    /// The H.264 the stream carries, from RTP, transport stream and PES.
    pub fn elementary_stream(rtp: &[Vec<u8>]) -> Vec<u8> {
        let mut pes = Vec::new();
        for p in rtp {
            assert_eq!(p[1], 33);
            for t in p[12..].chunks(188) {
                assert_eq!(t[0], 0x47);
                let pid = (((t[1] & 0x1f) as u16) << 8) | t[2] as u16;
                if pid != 0x1011 {
                    continue;
                }
                let mut at = 4;
                if t[3] & 0x20 != 0 {
                    at += 1 + t[4] as usize;
                }
                if t[1] & 0x40 != 0 {
                    // PES start: skip its header.
                    at += 9 + t[at + 8] as usize;
                }
                pes.extend_from_slice(&t[at..]);
            }
        }
        pes
    }

    /// A session of `length` with the stand-in TV: the report, and the H.264
    /// that arrived.
    pub fn session_with_stand_in(length: Duration) -> (Report, Vec<u8>) {
        let mut pattern = video::Pattern::new(16, 16);
        session_with_stand_in_from(&mut pattern, length)
    }

    /// The same with pictures from anywhere.
    pub fn session_with_stand_in_from(pictures: &mut dyn Pictures, length: Duration) -> (Report, Vec<u8>) {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let received = Arc::new(Mutex::new(Vec::new()));
        let tv = std::cell::Cell::new(None);
        let opts = Options {
            name: "Kagaz test".into(),
            max_height: 720,
            length: Some(length),
            wait: Duration::from_secs(5),
        };
        let stop = AtomicBool::new(false);
        let report = mirror(
            Route::Direct,
            &opts,
            pictures,
            &stop,
            || {
                tv.set(Some(stand_in_tv(received.clone())));
                Ok(())
            },
            |_| {},
        )
        .unwrap();
        tv.take().unwrap().join().unwrap();
        let es = elementary_stream(&received.lock().unwrap());
        (report, es)
    }

    #[test]
    fn a_whole_session_with_a_stand_in_tv() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let received = Arc::new(Mutex::new(Vec::new()));
        let tv = std::cell::Cell::new(None);
        let opts = Options {
            name: "Kagaz test".into(),
            max_height: 720,
            length: Some(Duration::from_millis(1500)),
            wait: Duration::from_secs(5),
        };
        let stop = AtomicBool::new(false);
        let mut events = Vec::new();
        let mut pattern = video::Pattern::new(16, 16);
        let report = mirror(
            Route::Direct,
            &opts,
            &mut pattern,
            &stop,
            || {
                tv.set(Some(stand_in_tv(received.clone())));
                Ok(())
            },
            |e| events.push(e),
        )
        .unwrap();
        let seen = tv.take().unwrap().join().unwrap();
        assert_eq!(report.ended_by, "time");
        assert_eq!(report.mode.unwrap().height, 720);
        assert!(report.frames >= 40, "{} frames", report.frames);
        assert!(events.iter().any(|e| matches!(e, Event::Playing(_))));
        // The TV saw the setup in order, and the end.
        let requests: Vec<&str> = seen.iter().map(String::as_str).filter(|s| !s.starts_with("reply")).collect();
        assert_eq!(requests, ["OPTIONS", "GET_PARAMETER", "SET_PARAMETER", "SET_PARAMETER", "SET_PARAMETER"]);

        // What arrived decodes into pictures of the agreed size.
        let es = elementary_stream(&received.lock().unwrap());
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut pictures = 0;
        for nal in openh264::nal_units(&es) {
            if let Ok(Some(yuv)) = decoder.decode(nal) {
                use openh264::formats::YUVSource;
                assert_eq!(yuv.dimensions(), (1280, 720));
                pictures += 1;
            }
        }
        assert!(pictures >= 30, "{pictures} pictures decoded");
    }
}

#[cfg(test)]
mod motion {
    use super::tests_support::*;
    use super::*;

    /// The test pattern's square moves the same distance in every picture
    /// that arrives (4.96 pixels at 720p60, so 4 to 6 after rounding), and
    /// no picture goes missing: what reaches the TV is steady.
    #[test]
    fn the_square_moves_steadily() {
        let (report, es) = session_with_stand_in(Duration::from_millis(3000));
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut xs = Vec::new();
        for nal in openh264::nal_units(&es) {
            if let Ok(Some(yuv)) = decoder.decode(nal) {
                use openh264::formats::YUVSource;
                let (w, h) = yuv.dimensions();
                let row = (h * 2 / 3) / 2;
                let y = &yuv.y()[row * yuv.strides().0..][..w];
                // White is 235 in video's limited range; the bars are below 200.
                xs.push(y.iter().position(|&v| v > 225).expect("the square is on the row"));
            }
        }
        assert!(xs.len() as u64 + 2 >= report.frames, "{} of {} pictures decoded", xs.len(), report.frames);
        for p in xs.windows(2) {
            let step = p[1] as i64 - p[0] as i64;
            assert!((3..=7).contains(&step), "a step of {step} pixels: {xs:?}");
        }
    }
}
