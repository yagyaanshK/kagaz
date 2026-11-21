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
    let (_announced, ip) = announce(name, "Kagaz stand-in", port)?;
    let r = serve_losing(TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("port {port} is taken: {e}"))?, lose, stop, &mut say, Some(&format!("\"{name}\" is on the network at {ip} as a Google Cast TV, waiting for a sender (Ctrl+C ends).")));
    r
}

/// This computer listed on the network as a Google Cast screen while it
/// lives.
pub struct Announced(mdns_sd::ServiceDaemon);

impl Drop for Announced {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

/// Lists this computer as a Cast screen called `name` (`model` says what
/// it is), at `port`; with the address it is listed at.
pub fn announce(name: &str, model: &str, port: u16) -> Result<(Announced, IpAddr), String> {
    let ip = local_ip().ok_or("this computer has no address on the network")?;
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
    let id = hex(&super::super::miracast::random_bytes::<16>());
    let mut txt = vec![("id", id.clone()), ("fn", name.to_string()), ("md", model.to_string()), ("ve", "05".into()), ("ca", "4101".into()), ("st", "0".into()), ("rs", String::new())];
    // Kagaz on a phone lists this computer by what it is (nearby.rs): its
    // system and whether it is a laptop. Only in this announcement, which
    // goes out only while this computer is a screen.
    if let Some(os) = crate::nearby::this_os() {
        txt.push(("kz-os", os));
    }
    if let Some(form) = crate::nearby::this_form() {
        txt.push(("kz-form", form.to_string()));
    }
    let txt: Vec<(&str, &str)> = txt.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let info = mdns_sd::ServiceInfo::new("_googlecast._tcp.local.", &format!("Kagaz-{}", &id[..8]), "kagaz-screen.local.", ip, port, &txt[..])
        .map_err(|e| e.to_string())?;
    daemon.register(info).map_err(|e| e.to_string())?;
    Ok((Announced(daemon), ip))
}

/// Who asks to show their screen here.
#[derive(Debug, Clone)]
pub struct Who {
    pub address: IpAddr,
    /// The name the sender gave, or what it is ("Kagaz", a browser).
    pub name: String,
}

/// What a screen that takes senders does with them: the stand-in only
/// counts; the app shows the pictures and plays the sound.
pub trait Screen {
    /// A sender asks to start showing its screen: whether to take it.
    fn ask(&mut self, _who: &Who) -> bool {
        true
    }
    /// A whole picture (H.264, Annex B); `key`: whole by itself.
    fn picture(&mut self, _data: Vec<u8>, _key: bool) {}
    /// A sound packet, Opus or (`aac`) AAC, 48 kHz stereo.
    fn sound(&mut self, _packet: Vec<u8>, _aac: bool) {}
    /// Plain sound (Miracast's LPCM): 16-bit 48 kHz stereo, interleaved.
    fn pcm(&mut self, _samples: Vec<i16>) {}
    /// A line for the record.
    fn say(&mut self, line: String);
    /// The sender's session ended (or never started).
    fn ended(&mut self) {}
    /// Asked often: false ends this sender's session (this screen's Stop).
    fn keep(&mut self) -> bool {
        true
    }
}

/// A sender's sound on this computer's speakers: Opus at 48 kHz stereo,
/// through remote desktop's player (a small reserve that steers itself).
/// AAC (a Windows sender's) is not played yet; nor is anything on Windows,
/// whose Opus is not built in.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub struct Speaker {
    player: crate::remote::rdp::play::Player,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Default for Speaker {
    fn default() -> Self {
        Speaker { player: crate::remote::rdp::play::Player::new() }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Speaker {
    /// One packet; false when it could not be played here.
    pub fn play(&mut self, packet: &[u8], aac: bool) -> bool {
        use ironrdp_rdpsnd::pdu::{AudioFormat, WaveFormat};
        if aac {
            return false;
        }
        let opus = AudioFormat {
            format: WaveFormat::OPUS,
            n_channels: 2,
            n_samples_per_sec: 48_000,
            n_avg_bytes_per_sec: 20_000,
            n_block_align: 4,
            bits_per_sample: 16,
            data: None,
        };
        self.player.wave(&opus, packet) > 0
    }

    /// Plain sound: 16-bit, 48 kHz, stereo, interleaved (a Miracast
    /// sender's LPCM, its bytes already put in this machine's order).
    pub fn play_pcm(&mut self, samples: &[i16]) -> bool {
        use ironrdp_rdpsnd::pdu::{AudioFormat, WaveFormat};
        let pcm = AudioFormat {
            format: WaveFormat::PCM,
            n_channels: 2,
            n_samples_per_sec: 48_000,
            n_avg_bytes_per_sec: 192_000,
            n_block_align: 4,
            bits_per_sample: 16,
            data: None,
        };
        let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.player.wave(&pcm, &bytes) > 0
    }

    pub fn set_volume(&mut self, level: f32) {
        let v = (level.clamp(0.0, 1.0) * 65535.0) as u16;
        self.player.set_volume(v, v);
    }

    /// The sound stops at once.
    pub fn close(&mut self) {
        self.player.close();
    }
}

/// Only lines, through a closure (the stand-in).
struct Saying<'a>(&'a mut dyn FnMut(String));

impl Screen for Saying<'_> {
    fn say(&mut self, line: String) {
        (self.0)(line)
    }
}

/// Takes senders one at a time on `listener` until `stop`.
pub fn serve(listener: TcpListener, stop: &AtomicBool, say: &mut dyn FnMut(String), hello: Option<&str>) -> Result<(), String> {
    serve_losing(listener, None, stop, say, hello)
}

/// The same, losing every `lose`th packet of the stream (a weak link, for
/// tests of resending and of the rate).
pub fn serve_losing(listener: TcpListener, lose: Option<u64>, stop: &AtomicBool, say: &mut dyn FnMut(String), hello: Option<&str>) -> Result<(), String> {
    if let Some(h) = hello {
        say(h.to_string());
    }
    receive(listener, lose, stop, &mut Saying(say))
}

/// Takes senders one at a time on `listener` until `stop`, each handed to
/// `screen`.
pub fn receive(listener: TcpListener, lose: Option<u64>, stop: &AtomicBool, screen: &mut dyn Screen) -> Result<(), String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let config = tls_setup()?;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((tcp, from)) => {
                screen.say(format!("A sender connected from {}.", from.ip()));
                let r = session(tcp, from.ip(), config.clone(), lose, stop, screen);
                match r {
                    Ok(()) => screen.say("The sender left.".into()),
                    Err(e) => screen.say(format!("The session ended: {e}")),
                }
                screen.ended();
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
    /// The sound is AAC: 1024 samples a packet.
    aac: bool,
}

fn session(mut tcp: TcpStream, address: IpAddr, config: Arc<rustls::ServerConfig>, lose: Option<u64>, stop: &AtomicBool, screen: &mut dyn Screen) -> Result<(), String> {
    let mut who = Who { address, name: String::new() };
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
                // Kagaz's sender gives its device's name; others what
                // they are.
                ("urn:x-cast:com.google.cast.tp.connection", "CONNECT") if who.name.is_empty() => {
                    who.name = j["senderName"]
                        .as_str()
                        .or_else(|| j["userAgent"].as_str())
                        .unwrap_or("")
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(60)
                        .collect();
                }
                (super::channel::RECEIVER, "LAUNCH") if j["appId"] == super::APP && !app_running && !screen.ask(&who) => {
                    screen.say(format!("Declined {}.", if who.name.is_empty() { who.address.to_string() } else { who.name.clone() }));
                    let r = json!({ "type": "LAUNCH_ERROR", "requestId": request, "reason": "CANCELLED" });
                    link.send(&from, super::channel::RECEIVER, &r)?;
                    return Ok(());
                }
                (super::channel::RECEIVER, "LAUNCH") if j["appId"] == super::APP => {
                    app_running = true;
                    screen.say("The sender started screen mirroring.".into());
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
                    screen.say("The sender stopped screen mirroring.".into());
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
                    screen.say(format!(
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
        if !screen.keep() {
            screen.say("Stopped here.".into());
            return Ok(());
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
                                screen.sound(p.clone(), t.aac);
                                sound_packets += 1;
                                sound_samples += if t.aac { 1024 } else { super::super::miracast::external::opus_samples(&p) as u64 };
                            } else {
                                pictures += 1;
                                bytes += p.len() as u64;
                                let key = whole_by_itself(&p);
                                let sized = sps_size(&p);
                                screen.picture(p, key);
                                if let Some(s) = sized {
                                    whole += 1;
                                    if size != Some(s) {
                                        screen.say(format!("The pictures are {} x {} now.", s.0, s.1));
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
                screen.say(format!(
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
        if (sound && codec != "opus" && codec != "aac") || (!sound && codec != "h264") || out.iter().any(|t| t.sound == sound) {
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
        out.push(Taken { index: s["index"].as_u64().unwrap_or(0), receiver: Receiver::new(agreed), sound, aac: codec == "aac" });
    }
    out
}

/// Whether an Annex B picture is whole by itself (holds an IDR slice).
pub fn whole_by_itself(annex_b: &[u8]) -> bool {
    annex_b.windows(4).any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1f == 5)
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
