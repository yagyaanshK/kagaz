//! A stand-in Google Cast TV on this computer, for testing a sender (Kagaz
//! on a phone or a computer) without a real TV: it announces itself as a
//! Cast device, takes the secure connection on port 8009, starts "screen
//! mirroring" when asked, answers the offer, and puts the pictures and
//! the sound together as a TV would, answering with the TV's reports so
//! that the sender's resending and rate fitting run as they would. It
//! keeps no picture and plays no sound: only numbers (pictures a second,
//! the picture's size and each change of it, whole pictures, sound
//! packets, the rate) are reported.

use super::channel::{fields, record};
use super::stream::{Agreed, Receiver, CLOCK};
use serde_json::{json, Value};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const TRANSPORT: &str = "web-kagaz-1";
const ME: &str = "receiver-0";

/// Runs until `stop` on `port` (8009, as TVs use, unless testing),
/// saying what happens through `say`; `lose`: every that-many-th packet
/// of the stream is lost, as on a weak link.
pub fn run(name: &str, port: u16, lose: Option<u64>, stop: &AtomicBool, mut say: impl FnMut(String)) -> Result<(), String> {
    let ip = local_ip().ok_or("this computer has no address on the network")?;
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
    let id = hex(&super::super::miracast::random_bytes::<16>());
    let txt = [("id", id.as_str()), ("fn", name), ("md", "Kagaz stand-in"), ("ve", "05"), ("ca", "4101"), ("st", "0"), ("rs", "")];
    let info = mdns_sd::ServiceInfo::new("_googlecast._tcp.local.", &format!("Kagaz-stand-in-{}", &id[..8]), "kagaz-stand-in.local.", ip, port, &txt[..])
        .map_err(|e| e.to_string())?;
    daemon.register(info).map_err(|e| e.to_string())?;
    let r = serve_losing(TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("port {port} is taken: {e}"))?, lose, stop, &mut say, Some(&format!("\"{name}\" is on the network at {ip} as a Google Cast TV, waiting for a sender (Ctrl+C ends).")));
    let _ = daemon.shutdown();
    r
}

/// Takes senders one at a time on `listener` until `stop`.
pub fn serve(listener: TcpListener, stop: &AtomicBool, say: &mut dyn FnMut(String), hello: Option<&str>) -> Result<(), String> {
    serve_losing(listener, None, stop, say, hello)
}

/// The same, losing every `lose`th packet of the stream (a weak link, for
/// tests of resending and of the rate).
pub fn serve_losing(listener: TcpListener, lose: Option<u64>, stop: &AtomicBool, say: &mut dyn FnMut(String), hello: Option<&str>) -> Result<(), String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let config = tls_setup()?;
    if let Some(h) = hello {
        say(h.to_string());
    }
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((tcp, from)) => {
                say(format!("A sender connected from {}.", from.ip()));
                match session(tcp, config.clone(), lose, stop, say) {
                    Ok(()) => say("The sender left.".into()),
                    Err(e) => say(format!("The session ended: {e}")),
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
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

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<[u8; 16]> {
    let mut out = [0u8; 16];
    if s.len() != 32 {
        return None;
    }
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// A certificate made for the occasion, as a TV makes its own.
fn tls_setup() -> Result<Arc<rustls::ServerConfig>, String> {
    let made = rcgen::generate_simple_self_signed(vec!["kagaz-stand-in".to_string()]).map_err(|e| e.to_string())?;
    let cert = made.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(made.signing_key.serialize_der().into());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(config))
}

/// The TV's side of the secure connection: whole records in, records out.
struct Link {
    tcp: TcpStream,
    tls: rustls::ServerConnection,
    inbox: Vec<u8>,
}

impl Link {
    fn send(&mut self, to: &str, namespace: &str, json: &Value) -> Result<(), String> {
        self.tls.writer().write_all(&record(ME, to, namespace, json)).map_err(|e| e.to_string())?;
        while self.tls.wants_write() {
            match self.tls.write_tls(&mut self.tcp) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(1)),
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    /// (from, namespace, JSON) for each record come in.
    fn heard(&mut self) -> Result<Vec<(String, String, Value)>, String> {
        let mut closed = false;
        loop {
            match self.tls.read_tls(&mut self.tcp) {
                Ok(0) => {
                    closed = true;
                    break;
                }
                Ok(_) => {
                    self.tls.process_new_packets().map_err(|e| e.to_string())?;
                    let mut buf = [0u8; 16384];
                    loop {
                        match self.tls.reader().read(&mut buf) {
                            Ok(0) => {
                                closed = true;
                                break;
                            }
                            Ok(n) => self.inbox.extend(&buf[..n]),
                            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                            Err(e) => return Err(e.to_string()),
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::ConnectionReset => {
                    closed = true;
                    break;
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        let mut out = Vec::new();
        while self.inbox.len() >= 4 {
            let n = u32::from_be_bytes(self.inbox[..4].try_into().unwrap()) as usize;
            if self.inbox.len() < 4 + n {
                break;
            }
            let f = fields(&self.inbox[4..4 + n]);
            self.inbox.drain(..4 + n);
            if let Some([_, _, from, _, namespace, _, json]) = f {
                out.push((from, namespace, serde_json::from_str(&json).unwrap_or(Value::Null)));
            }
        }
        // What came before the close is still heard (the sender's Stop).
        if closed && out.is_empty() {
            return Err("the sender closed the connection".into());
        }
        Ok(out)
    }
}

/// One stream the sender offered and the stand-in took.
struct Taken {
    index: u64,
    receiver: Receiver,
    sound: bool,
}

fn session(mut tcp: TcpStream, config: Arc<rustls::ServerConfig>, lose: Option<u64>, stop: &AtomicBool, say: &mut dyn FnMut(String)) -> Result<(), String> {
    tcp.set_nonblocking(false).ok();
    tcp.set_nodelay(true).ok();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut tls = rustls::ServerConnection::new(config).map_err(|e| e.to_string())?;
    while tls.is_handshaking() {
        tls.complete_io(&mut tcp).map_err(|e| format!("the secure connection failed: {e}"))?;
    }
    tcp.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut link = Link { tcp, tls, inbox: Vec::new() };
    let session_id = hex(&super::super::miracast::random_bytes::<16>());
    let mut app_running = false;
    let status = |running: bool| {
        let apps = if running {
            json!([{ "appId": super::APP, "displayName": "Chrome Mirroring", "sessionId": session_id, "transportId": TRANSPORT, "namespaces": [{ "name": super::channel::STREAMING }] }])
        } else {
            json!([])
        };
        json!({ "type": "RECEIVER_STATUS", "requestId": 0, "status": { "applications": apps, "volume": { "level": 1.0, "muted": false } } })
    };
    let mut udp: Option<UdpSocket> = None;
    let mut taken: Vec<Taken> = Vec::new();
    let mut peer: Option<SocketAddr> = None;
    let mut last_report = Instant::now();
    let mut noted = Instant::now();
    let mut last_heard = Instant::now();
    // For the record each second.
    let (mut pictures, mut whole, mut bytes, mut sound_packets, mut sound_samples) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut size: Option<(u32, u32)> = None;
    let mut buf = vec![0u8; 2048];
    let mut came = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let heard = link.heard()?;
        if !heard.is_empty() {
            last_heard = Instant::now();
        }
        for (from, namespace, j) in heard {
            let kind = j["type"].as_str().unwrap_or("");
            let request = j["requestId"].as_i64().unwrap_or(0);
            match (namespace.as_str(), kind) {
                (HEARTBEAT, "PING") => link.send(&from, HEARTBEAT, &json!({ "type": "PONG" }))?,
                (super::channel::RECEIVER, "LAUNCH") if j["appId"] == super::APP => {
                    app_running = true;
                    say("The sender started screen mirroring.".into());
                    let mut s = status(true);
                    s["requestId"] = json!(request);
                    link.send(&from, super::channel::RECEIVER, &s)?;
                }
                (super::channel::RECEIVER, "LAUNCH") => {
                    let r = json!({ "type": "LAUNCH_ERROR", "requestId": request, "reason": "NOT_FOUND" });
                    link.send(&from, super::channel::RECEIVER, &r)?;
                }
                (super::channel::RECEIVER, "GET_STATUS") => {
                    let mut s = status(app_running);
                    s["requestId"] = json!(request);
                    link.send(&from, super::channel::RECEIVER, &s)?;
                }
                (super::channel::RECEIVER, "STOP") => {
                    say("The sender stopped screen mirroring.".into());
                    link.send(&from, super::channel::RECEIVER, &status(false))?;
                    return Ok(());
                }
                (super::channel::STREAMING, "OFFER") => {
                    let socket = UdpSocket::bind(("0.0.0.0", 0)).map_err(|e| e.to_string())?;
                    socket.set_nonblocking(true).map_err(|e| e.to_string())?;
                    let port = socket.local_addr().map_err(|e| e.to_string())?.port();
                    taken = take_offer(&j);
                    let answer = json!({
                        "type": "ANSWER", "seqNum": j["seqNum"], "result": "ok",
                        "answer": {
                            "udpPort": port,
                            "sendIndexes": taken.iter().map(|t| t.index).collect::<Vec<_>>(),
                            "ssrcs": taken.iter().map(|t| t.receiver_ssrc()).collect::<Vec<_>>(),
                            "display": { "dimensions": { "width": 3840, "height": 2160, "frameRate": "60" }, "scaling": "sender" },
                        }
                    });
                    say(format!(
                        "Offered {}; took {} (port {port}).",
                        j["offer"]["supportedStreams"].as_array().map(|s| s.len()).unwrap_or(0),
                        taken.iter().map(|t| if t.sound { "the sound" } else { "the pictures" }).collect::<Vec<_>>().join(" and ")
                    ));
                    link.send(&from, super::channel::STREAMING, &answer)?;
                    udp = Some(socket);
                }
                _ => {}
            }
        }
        if last_heard.elapsed() > Duration::from_secs(20) {
            return Err("the sender went quiet".into());
        }
        let Some(socket) = &udp else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        let now = Instant::now();
        let mut got = false;
        // A picture made whole is confirmed at once, as a Cast receiver
        // does; else every 33 ms.
        let mut completed = false;
        loop {
            match socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    got = true;
                    peer = Some(from);
                    came += 1;
                    // Reports (RTCP, types 200 to 206) always come through.
                    if lose.is_some_and(|l| came % l == 0) && n > 1 && !(200..=206).contains(&buf[1]) {
                        continue;
                    }
                    for t in taken.iter_mut() {
                        t.receiver.take(&buf[..n], now);
                        while let Some(p) = t.receiver.picture() {
                            completed = true;
                            if t.sound {
                                sound_packets += 1;
                                sound_samples += super::super::miracast::external::opus_samples(&p) as u64;
                            } else {
                                pictures += 1;
                                bytes += p.len() as u64;
                                if let Some(s) = sps_size(&p) {
                                    whole += 1;
                                    if size != Some(s) {
                                        say(format!("The pictures are {} x {} now.", s.0, s.1));
                                        size = Some(s);
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        if let Some(to) = peer {
            let since = now.duration_since(last_report);
            if since >= Duration::from_millis(33) || (completed && since >= Duration::from_millis(2)) {
                last_report = now;
                for t in &taken {
                    let _ = socket.send_to(&t.receiver.report(now), to);
                }
            }
        }
        if noted.elapsed() >= Duration::from_secs(1) {
            let secs = noted.elapsed().as_secs_f64();
            noted = Instant::now();
            if pictures > 0 || sound_packets > 0 {
                say(format!(
                    "{:.0} pictures a second, {:.1} Mbit/s, {whole} whole; sound: {sound_packets} packets, {:.2} s of it.",
                    pictures as f64 / secs,
                    bytes as f64 * 8.0 / secs / 1e6,
                    sound_samples as f64 / 48_000.0
                ));
            }
            (pictures, whole, bytes, sound_packets, sound_samples) = (0, 0, 0, 0, 0);
        }
        if !got {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(())
}

impl Taken {
    fn receiver_ssrc(&self) -> u32 {
        self.receiver.tv_ssrc()
    }
}

/// The streams taken from an offer: the first H.264 pictures and the first
/// Opus sound.
fn take_offer(j: &Value) -> Vec<Taken> {
    let mut out: Vec<Taken> = Vec::new();
    for s in j["offer"]["supportedStreams"].as_array().into_iter().flatten() {
        let sound = s["type"] == "audio_source";
        let codec = s["codecName"].as_str().unwrap_or("");
        if (sound && codec != "opus") || (!sound && codec != "h264") || out.iter().any(|t| t.sound == sound) {
            continue;
        }
        let (Some(key), Some(mask), Some(ssrc), Some(pt)) = (
            s["aesKey"].as_str().and_then(unhex),
            s["aesIvMask"].as_str().and_then(unhex),
            s["ssrc"].as_u64(),
            s["rtpPayloadType"].as_u64(),
        ) else {
            continue;
        };
        let agreed = Agreed {
            ssrc: ssrc as u32,
            tv_ssrc: ssrc as u32 + 1,
            key,
            iv_mask: mask,
            payload_type: pt as u8,
            clock: if sound { 48_000 } else { CLOCK },
            delay: Duration::from_millis(s["targetDelay"].as_u64().unwrap_or(400)),
        };
        out.push(Taken { index: s["index"].as_u64().unwrap_or(0), receiver: Receiver::new(agreed), sound });
    }
    out
}

/// The picture size a whole picture's sequence header gives (H.264).
pub fn sps_size(annex_b: &[u8]) -> Option<(u32, u32)> {
    // Find a NAL unit of type 7.
    let mut i = 0;
    let sps = loop {
        if i + 4 > annex_b.len() {
            return None;
        }
        if annex_b[i..i + 3] == [0, 0, 1] && annex_b[i + 3] & 0x1f == 7 {
            break &annex_b[i + 4..];
        }
        i += 1;
    };
    // Without the emulation-prevention bytes.
    let mut rbsp = Vec::with_capacity(sps.len().min(64));
    let mut zeros = 0;
    for &b in sps.iter().take(64) {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        rbsp.push(b);
    }
    let mut r = Bits { b: &rbsp, at: 0 };
    let profile = r.bits(8)?;
    r.bits(16)?; // constraints and level
    r.ue()?; // the header's number
    if matches!(profile, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        if r.ue()? == 3 {
            r.bits(1)?;
        }
        r.ue()?;
        r.ue()?;
        r.bits(1)?;
        if r.bits(1)? == 1 {
            return None; // scaling lists: not met here
        }
    }
    r.ue()?; // log2 of the largest frame number, less 4
    match r.ue()? {
        0 => {
            r.ue()?;
        }
        1 => {
            r.bits(1)?;
            r.se()?;
            r.se()?;
            for _ in 0..r.ue()? {
                r.se()?;
            }
        }
        _ => {}
    }
    r.ue()?; // reference pictures
    r.bits(1)?;
    let w_mbs = r.ue()? + 1;
    let h_units = r.ue()? + 1;
    let frames_only = r.bits(1)?;
    if frames_only == 0 {
        r.bits(1)?;
    }
    r.bits(1)?;
    let (mut w, mut h) = (w_mbs * 16, h_units * 16 * (2 - frames_only));
    if r.bits(1)? == 1 {
        let (left, right, top, bottom) = (r.ue()?, r.ue()?, r.ue()?, r.ue()?);
        w -= 2 * (left + right);
        h -= 2 * (top + bottom) * (2 - frames_only);
    }
    Some((w, h))
}

struct Bits<'a> {
    b: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.b.get(self.at / 8)?;
        let v = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(v as u32)
    }
    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = v << 1 | self.bit()?;
        }
        Some(v)
    }
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
    }
    fn se(&mut self) -> Option<i32> {
        let v = self.ue()?;
        Some(if v % 2 == 1 { v.div_ceil(2) as i32 } else { -((v / 2) as i32) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_size_from_kagazs_own_header() {
        // The header Kagaz writes for VA-API at 1920 x 1080 (cropped from
        // 1088), and at an upright phone's 912 x 1920.
        for (w, h) in [(1920u32, 1080u32), (912, 1920), (1280, 720)] {
            let s = crate::tv::miracast::h264::Stream { width: w as usize, height: h as usize, fps: 60, level_idc: 42, log2_max_frame_num: 8 };
            let sps = crate::tv::miracast::h264::sps(&s);
            let mut annex_b = vec![0, 0, 0, 1];
            annex_b.extend(&sps);
            assert_eq!(sps_size(&annex_b), Some((w, h)), "{w} x {h}");
        }
    }
}
