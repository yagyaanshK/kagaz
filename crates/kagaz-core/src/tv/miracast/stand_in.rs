//! A stand-in TV on this computer, for testing a sender (Kagaz on a
//! phone) without a real TV: it announces itself on the network as a
//! screen-mirroring TV does, takes the sender's "ready" on port 7250,
//! answers the session as the LG does, and decodes what arrives. It keeps
//! no picture: only numbers (pictures a second, sizes, whole pictures,
//! how many decode) are reported.

use super::{mice, rtsp, Link, Message};
use std::io::Read;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LG: &str = include_str!("../../../tests/fixtures/tv/lg-webos-2024-wfd-m3.txt");

/// Runs until `stop`, saying what happens through `say`.
pub fn run(name: &str, stop: &AtomicBool, mut say: impl FnMut(String)) -> Result<(), String> {
    let ip = local_ip().ok_or("this computer has no address on the network")?;
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
    let host = "kagaz-stand-in.local.";
    let info = mdns_sd::ServiceInfo::new("_display._tcp.local.", name, host, ip, mice::PORT, None)
        .map_err(|e| e.to_string())?;
    daemon.register(info).map_err(|e| e.to_string())?;
    let listener = TcpListener::bind(("0.0.0.0", mice::PORT)).map_err(|e| format!("port {} is taken: {e}", mice::PORT))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    say(format!("\"{name}\" is on the network at {ip}, waiting for a sender (Ctrl+C ends)."));
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut tcp, from)) => {
                tcp.set_nonblocking(false).ok();
                tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let mut buf = vec![0u8; 2048];
                let n = tcp.read(&mut buf).unwrap_or(0);
                match mice::read(&buf[..n]) {
                    Some((1, who, Some(port))) => {
                        say(format!("{who} ({}) is ready; connecting to its port {port}.", from.ip()));
                        if let Err(e) = session(SocketAddr::new(from.ip(), port), stop, &mut say) {
                            say(format!("The session failed: {e}"));
                        }
                    }
                    Some((2, who, _)) => say(format!("{who} says it stopped.")),
                    other => say(format!("Something else came on port {}: {other:?} ({n} bytes)", mice::PORT)),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let _ = daemon.shutdown();
    Ok(())
}

fn local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

/// Plays the TV's side of one session.
fn session(source: SocketAddr, stop: &AtomicBool, say: &mut impl FnMut(String)) -> Result<(), String> {
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
    let mut decoder = openh264::decoder::Decoder::new().map_err(|e| e.to_string())?;
    let mut stream = Stream::default();
    let mut last = Instant::now();
    let started = Instant::now();
    let mut ended = false;
    while !stop.load(Ordering::Relaxed) && !ended {
        match link.next(Duration::from_millis(50)) {
            Ok(Some(m)) => {
                if m.is_reply() {
                    if m.cseq() == setup_cseq {
                        let play = link.request("PLAY", "rtsp://localhost/wfd1.0/streamid=0");
                        link.send(&play)?;
                        say("Playing.".into());
                    }
                } else {
                    say(format!("  {} {}", m.method(), m.body.lines().next().unwrap_or("")));
                    let mut reply = Message::reply(m.cseq());
                    match m.method() {
                        "OPTIONS" => reply = reply.header("Public", "org.wfa.wfd1.0, GET_PARAMETER, SET_PARAMETER"),
                        "GET_PARAMETER" if !m.body.is_empty() => {
                            reply = reply.with_parameters(LG.replace("unicast 53000", &format!("unicast {port}")))
                        }
                        "SET_PARAMETER" if m.body.contains("wfd_video_formats") || m.body.contains("wfd2_video_formats") => {
                            let mode = m.body.lines().find(|l| l.contains("video_formats")).unwrap_or("");
                            say(format!("  the sender chose: {mode}"));
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
                say(format!("The sender closed the session ({e})."));
                break;
            }
        }
        // What arrived: decoded and counted.
        let got: Vec<Vec<u8>> = std::mem::take(&mut *packets.lock().unwrap());
        stream.take(&got, &mut decoder);
        if last.elapsed() >= Duration::from_secs(1) {
            say(stream.second(started.elapsed()));
            last = Instant::now();
        }
    }
    done.store(true, Ordering::Relaxed);
    let _ = receiver.join();
    say(format!(
        "Ended after {:.0} s: {} packets, {} pictures decoded, {} whole pictures, {} lost packets{}.",
        started.elapsed().as_secs_f64(),
        stream.packets_total,
        stream.decoded_total,
        stream.keys_total,
        stream.lost,
        stream.size.map(|(w, h)| format!(", {w}x{h}")).unwrap_or_default()
    ));
    let _ = rtsp::MODES; // the modes the LG's list names (for reading the log)
    Ok(())
}

/// Counts for the report.
#[derive(Default)]
struct Stream {
    pes: Vec<u8>,
    seq: Option<u16>,
    lost: u64,
    packets: u64,
    packets_total: u64,
    bytes: u64,
    decoded: u64,
    decoded_total: u64,
    keys_total: u64,
    errors: u64,
    size: Option<(usize, usize)>,
}

impl Stream {
    fn take(&mut self, rtp: &[Vec<u8>], decoder: &mut openh264::decoder::Decoder) {
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
                if pid != 0x1011 {
                    continue;
                }
                let mut at = 4;
                if t[3] & 0x20 != 0 {
                    at += 1 + t[4] as usize;
                }
                if t[1] & 0x40 != 0 && at + 9 <= t.len() {
                    at += 9 + t[at + 8] as usize;
                }
                if at < t.len() {
                    self.pes.extend_from_slice(&t[at..]);
                }
            }
        }
        // Decode the whole units gathered (the last may be incomplete).
        let es = std::mem::take(&mut self.pes);
        let units: Vec<&[u8]> = openh264::nal_units(&es).collect();
        let keep_from = units.last().map(|u| u.as_ptr() as usize - es.as_ptr() as usize).unwrap_or(es.len());
        for u in &units[..units.len().saturating_sub(1)] {
            let kind = u.iter().skip_while(|&&b| b == 0).nth(1).map(|b| b & 0x1f).unwrap_or(0);
            if kind == 5 {
                self.keys_total += 1;
            }
            match decoder.decode(u) {
                Ok(Some(y)) => {
                    use openh264::formats::YUVSource;
                    self.decoded += 1;
                    self.decoded_total += 1;
                    self.size = Some(y.dimensions());
                }
                Ok(None) => {}
                Err(_) => self.errors += 1,
            }
        }
        self.pes = es[keep_from..].to_vec();
    }

    fn second(&mut self, at: Duration) -> String {
        let line = format!(
            "  {:3} s: {} packets ({} kbit/s), {} pictures decoded, {} undecodable units, {} packets lost so far{}",
            at.as_secs(),
            self.packets,
            self.bytes * 8 / 1000,
            self.decoded,
            self.errors,
            self.lost,
            self.size.map(|(w, h)| format!(", {w}x{h}")).unwrap_or_default()
        );
        (self.packets, self.bytes, self.decoded, self.errors) = (0, 0, 0, 0);
        line
    }
}
