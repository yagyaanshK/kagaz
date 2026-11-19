//! The screen's side of Miracast over the network (MS-MICE), for a stand-in
//! TV on this computer (testing a sender without a real TV) and for this
//! computer as a screen (Windows' own Cast): it announces itself as a
//! screen-mirroring TV does, takes a sender's "ready" on port 7250, asks
//! whether to take it, answers the session as the LG does (with plain
//! LPCM sound, which every sender has), and hands each whole picture and
//! the sound to a [`Screen`]. The stand-in decodes the pictures and
//! reports numbers only (pictures a second, sizes, whole pictures, how
//! many decode); no picture is shown or kept there.

use super::{mice, rtsp, Link, Message};
use crate::tv::cast::stand_in::{Screen, Who};
use std::io::Read;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LG: &str = include_str!("../../../tests/fixtures/tv/lg-webos-2024-wfd-m3.txt");
/// The pictures' and the sound's streams in a Miracast transport stream.
const VIDEO_PID: u16 = 0x1011;
const AUDIO_PID: u16 = 0x1100;

/// The stand-in: announces itself as `name` and reports until `stop`.
pub fn run(name: &str, stop: &AtomicBool, mut say: impl FnMut(String)) -> Result<(), String> {
    let (_announced, ip) = announce(name)?;
    let listener = TcpListener::bind(("0.0.0.0", mice::PORT)).map_err(|e| format!("port {} is taken: {e}", mice::PORT))?;
    say(format!("\"{name}\" is on the network at {ip}, waiting for a sender (Ctrl+C ends)."));
    let mut screen = Counting { say: &mut say, decoder: openh264::decoder::Decoder::new().map_err(|e| e.to_string())?, decoded: 0, errors: 0, size: None, last: Instant::now() };
    receive(listener, stop, &mut screen)
}

/// This computer listed on the network as a screen-mirroring TV while it
/// lives.
pub struct Announced(mdns_sd::ServiceDaemon);

impl Drop for Announced {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

/// Lists this computer as a Miracast screen called `name`.
pub fn announce(name: &str) -> Result<(Announced, IpAddr), String> {
    let ip = local_ip().ok_or("this computer has no address on the network")?;
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
    let info = mdns_sd::ServiceInfo::new("_display._tcp.local.", name, "kagaz-screen.local.", ip, mice::PORT, None)
        .map_err(|e| e.to_string())?;
    daemon.register(info).map_err(|e| e.to_string())?;
    Ok((Announced(daemon), ip))
}

/// Takes senders one at a time on `listener` (port 7250) until `stop`.
pub fn receive(listener: TcpListener, stop: &AtomicBool, screen: &mut dyn Screen) -> Result<(), String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut tcp, from)) => {
                tcp.set_nonblocking(false).ok();
                tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let mut buf = vec![0u8; 2048];
                let n = tcp.read(&mut buf).unwrap_or(0);
                match mice::read(&buf[..n]) {
                    Some((1, who, Some(port))) => {
                        let w = Who { address: from.ip(), name: who.chars().filter(|c| !c.is_control()).take(60).collect() };
                        if !screen.ask(&w) {
                            screen.say(format!("Declined {}.", w.name));
                            screen.ended();
                            continue;
                        }
                        screen.say(format!("{} ({}) is ready; connecting to its port {port}.", w.name, from.ip()));
                        if let Err(e) = session(SocketAddr::new(from.ip(), port), stop, screen) {
                            screen.say(format!("The session failed: {e}"));
                        }
                        screen.ended();
                    }
                    Some((2, who, _)) => screen.say(format!("{who} says it stopped.")),
                    other => screen.say(format!("Something else came on port {}: {other:?} ({n} bytes)", mice::PORT)),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

/// What the screen answers for its abilities: the LG's (the pictures it
/// takes), with plain 48 kHz sound only, at `port`.
fn abilities(port: u16) -> String {
    LG.lines()
        .map(|l| {
            if l.starts_with("wfd_audio_codecs:") {
                "wfd_audio_codecs: LPCM 00000002 00".to_string()
            } else {
                l.replace("unicast 53000", &format!("unicast {port}"))
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// Plays the screen's side of one session.
fn session(source: SocketAddr, stop: &AtomicBool, screen: &mut dyn Screen) -> Result<(), String> {
    let udp = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    let port = udp.local_addr().map_err(|e| e.to_string())?.port();
    udp.set_read_timeout(Some(Duration::from_millis(100))).ok();
    let packets: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    let (p, d) = (packets.clone(), done.clone());
    let receiver = std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        while !d.load(Ordering::Relaxed) {
            if let Ok(n) = udp.recv(&mut buf) {
                p.lock().unwrap().push(buf[..n].to_vec());
            }
        }
    });
    let tcp = TcpStream::connect_timeout(&source, Duration::from_secs(5)).map_err(|e| format!("could not connect: {e}"))?;
    let mut link = Link { tcp, buf: Vec::new(), cseq: 100 };
    let mut setup_cseq = String::new();
    let mut stream = Stream::default();
    let started = Instant::now();
    let mut last = Instant::now();
    let mut ended = false;
    while !stop.load(Ordering::Relaxed) && !ended {
        if !screen.keep() {
            let td = link.request("TEARDOWN", "rtsp://localhost/wfd1.0/streamid=0");
            let _ = link.send(&td);
            screen.say("Stopped here.".into());
            break;
        }
        match link.next(Duration::from_millis(20)) {
            Ok(Some(m)) => {
                if m.is_reply() {
                    if m.cseq() == setup_cseq {
                        let play = link.request("PLAY", "rtsp://localhost/wfd1.0/streamid=0");
                        link.send(&play)?;
                        screen.say("Playing.".into());
                    }
                } else {
                    screen.say(format!("  {} {}", m.method(), m.body.lines().next().unwrap_or("")));
                    let mut reply = Message::reply(m.cseq());
                    match m.method() {
                        "OPTIONS" => reply = reply.header("Public", "org.wfa.wfd1.0, GET_PARAMETER, SET_PARAMETER"),
                        "GET_PARAMETER" if !m.body.is_empty() => reply = reply.with_parameters(abilities(port)),
                        "SET_PARAMETER" if m.body.contains("wfd_video_formats") || m.body.contains("wfd2_video_formats") => {
                            let mode = m.body.lines().find(|l| l.contains("video_formats")).unwrap_or("");
                            screen.say(format!("  the sender chose: {mode}"));
                        }
                        _ => {}
                    }
                    link.send(&reply)?;
                    if m.method() == "OPTIONS" {
                        let m2 = link.request("OPTIONS", "*").header("Require", "org.wfa.wfd1.0");
                        link.send(&m2)?;
                    }
                    if m.body.contains("wfd_trigger_method: SETUP") {
                        let setup = link
                            .request("SETUP", "rtsp://localhost/wfd1.0/streamid=0")
                            .header("Transport", format!("RTP/AVP/UDP;unicast;client_port={port}"));
                        setup_cseq = setup.cseq().to_string();
                        link.send(&setup)?;
                    }
                    if m.body.contains("wfd_trigger_method: TEARDOWN") {
                        let td = link.request("TEARDOWN", "rtsp://localhost/wfd1.0/streamid=0");
                        link.send(&td)?;
                        ended = true;
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                screen.say(format!("The sender closed the session ({e})."));
                break;
            }
        }
        let got: Vec<Vec<u8>> = std::mem::take(&mut *packets.lock().unwrap());
        stream.take(&got, screen);
        if last.elapsed() >= Duration::from_secs(1) {
            let line = stream.second(started.elapsed());
            screen.say(line);
            last = Instant::now();
        }
    }
    done.store(true, Ordering::Relaxed);
    let _ = receiver.join();
    screen.say(format!(
        "Ended after {:.0} s: {} packets, {} pictures, {} whole pictures, {} lost packets.",
        started.elapsed().as_secs_f64(),
        stream.packets_total,
        stream.pictures_total,
        stream.keys_total,
        stream.lost,
    ));
    let _ = rtsp::MODES; // the modes the LG's list names (for reading the log)
    Ok(())
}

/// The transport stream put back together: whole pictures (a PES packet
/// each, as Miracast senders send them) and the sound.
#[derive(Default)]
struct Stream {
    picture: Vec<u8>,
    sound: Vec<u8>,
    seq: Option<u16>,
    lost: u64,
    packets: u64,
    packets_total: u64,
    bytes: u64,
    pictures: u64,
    pictures_total: u64,
    keys_total: u64,
}

impl Stream {
    fn take(&mut self, rtp: &[Vec<u8>], screen: &mut dyn Screen) {
        for p in rtp {
            if p.len() < 12 || p[1] & 0x7f != 33 {
                continue;
            }
            let seq = u16::from_be_bytes([p[2], p[3]]);
            if let Some(prev) = self.seq {
                let gap = seq.wrapping_sub(prev).wrapping_sub(1);
                if gap < 1000 {
                    self.lost += gap as u64;
                }
            }
            self.seq = Some(seq);
            self.packets += 1;
            self.packets_total += 1;
            self.bytes += p.len() as u64;
            for t in p[12..].chunks(188) {
                if t.len() < 188 || t[0] != 0x47 {
                    continue;
                }
                let pid = (((t[1] & 0x1f) as u16) << 8) | t[2] as u16;
                if pid != VIDEO_PID && pid != AUDIO_PID {
                    continue;
                }
                let start = t[1] & 0x40 != 0;
                let mut at = 4;
                if t[3] & 0x20 != 0 {
                    at += 1 + t[4] as usize;
                }
                // A new PES packet: the one before is whole.
                if start {
                    if pid == VIDEO_PID {
                        self.picture_done(screen);
                    } else {
                        self.sound_done(screen);
                    }
                    if at + 9 <= t.len() {
                        at += 9 + t[at + 8] as usize;
                    }
                }
                if at < t.len() {
                    if pid == VIDEO_PID {
                        self.picture.extend_from_slice(&t[at..]);
                    } else {
                        self.sound.extend_from_slice(&t[at..]);
                    }
                }
            }
        }
    }

    fn picture_done(&mut self, screen: &mut dyn Screen) {
        if self.picture.is_empty() {
            return;
        }
        let p = std::mem::take(&mut self.picture);
        let key = crate::tv::cast::stand_in::whole_by_itself(&p);
        if key {
            self.keys_total += 1;
        }
        self.pictures += 1;
        self.pictures_total += 1;
        screen.picture(p, key);
    }

    /// LPCM in a Miracast stream: a 4-byte header, then 16-bit samples,
    /// most significant byte first.
    fn sound_done(&mut self, screen: &mut dyn Screen) {
        if self.sound.len() <= 4 {
            self.sound.clear();
            return;
        }
        let s = std::mem::take(&mut self.sound);
        let samples: Vec<i16> = s[4..].chunks_exact(2).map(|b| i16::from_be_bytes([b[0], b[1]])).collect();
        screen.pcm(samples);
    }

    fn second(&mut self, at: Duration) -> String {
        let line = format!(
            "{} pictures a second ({} kbit/s, {} packets); {} packets lost so far; {} s in.",
            self.pictures,
            self.bytes * 8 / 1000,
            self.packets,
            self.lost,
            at.as_secs(),
        );
        (self.packets, self.bytes, self.pictures) = (0, 0, 0);
        line
    }
}

/// The stand-in's screen: decodes each picture (to count what decodes and
/// learn its size) and says what happens.
struct Counting<'a, F: FnMut(String)> {
    say: &'a mut F,
    decoder: openh264::decoder::Decoder,
    decoded: u64,
    errors: u64,
    size: Option<(usize, usize)>,
    last: Instant,
}

impl<F: FnMut(String)> Screen for Counting<'_, F> {
    fn picture(&mut self, data: Vec<u8>, _key: bool) {
        for u in openh264::nal_units(&data) {
            match self.decoder.decode(u) {
                Ok(Some(y)) => {
                    use openh264::formats::YUVSource;
                    self.decoded += 1;
                    self.size = Some(y.dimensions());
                }
                Ok(None) => {}
                Err(_) => self.errors += 1,
            }
        }
        if self.last.elapsed() >= Duration::from_secs(1) {
            self.last = Instant::now();
            let (d, e) = (self.decoded, self.errors);
            (self.say)(format!("  decoded {d} pictures, {e} undecodable units so far{}", self.size.map(|(w, h)| format!(", {w}x{h}")).unwrap_or_default()));
        }
    }

    fn say(&mut self, line: String) {
        (self.say)(line)
    }
}
