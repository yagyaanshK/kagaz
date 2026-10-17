//! A phone line on a SIP server over TCP, as the OneTouch cloud gives its
//! app: stay registered, ring on an incoming call, answer or turn it down,
//! call a device, hang up, press the door key. Voice is G.711 both ways;
//! video (H.264) is received only.
//!
//! One thread owns the signalling: it reads what a reader thread hands it
//! from the TCP connection, takes commands from the app, renews the
//! registration and keeps the connection alive, and reconnects when it
//! drops. Each call adds two receiver threads (voice, video) that hand
//! decoded media to the app; the app's microphone audio goes out through
//! [`Phone::send_audio`].

use super::g711::Law;
use super::h264::{AccessUnit, Depacketizer};
use super::sdp::{self, Agreed};
use super::sip::{self, digest_authorization, token, Message};
use super::SipAccount;
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What happened on the line, for the app to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Registered (`up`) or not, with a reason when not.
    Line { up: bool, detail: String },
    /// Someone is calling us.
    Incoming {
        id: String,
        number: String,
        display: String,
        video: bool,
    },
    /// We are calling `number`.
    Outgoing { id: String, number: String },
    /// The other side is ringing.
    Ringing { id: String },
    /// Talking.
    Connected { id: String, video: bool },
    /// The call is over.
    Ended { id: String, reason: String },
}

/// Media for the app: decoded voice (8 kHz mono) or a whole picture.
pub enum MediaOut {
    Audio(Vec<i16>),
    Video(AccessUnit),
}

enum Command {
    Answer,
    Decline,
    Hangup,
    Call(String),
    Dtmf(char),
    Stop,
}

type EventSink = Arc<dyn Fn(Event) + Send + Sync>;
type MediaSink = Arc<dyn Fn(MediaOut) + Send + Sync>;

/// The line. Dropping it signs out and stops.
pub struct Phone {
    tx: Sender<Command>,
    audio: Arc<Mutex<Option<AudioOut>>>,
}

impl Phone {
    /// Start the line on its own thread.
    pub fn start(
        line: SipAccount,
        on_event: impl Fn(Event) + Send + Sync + 'static,
        on_media: impl Fn(MediaOut) + Send + Sync + 'static,
    ) -> Phone {
        let (tx, rx) = mpsc::channel();
        let audio = Arc::new(Mutex::new(None));
        let shared = audio.clone();
        let events: EventSink = Arc::new(on_event);
        let media: MediaSink = Arc::new(on_media);
        std::thread::Builder::new()
            .name("kagaz-phone".into())
            .spawn(move || run(line, rx, events, media, shared))
            .expect("phone thread");
        Phone { tx, audio }
    }

    pub fn answer(&self) {
        let _ = self.tx.send(Command::Answer);
    }
    pub fn decline(&self) {
        let _ = self.tx.send(Command::Decline);
    }
    pub fn hangup(&self) {
        let _ = self.tx.send(Command::Hangup);
    }
    /// Call a number on the same server (a device's `sip`).
    pub fn call(&self, number: &str) {
        let _ = self.tx.send(Command::Call(number.to_string()));
    }
    /// Press a key during a call (`#` opens most doors).
    pub fn dtmf(&self, key: char) {
        let _ = self.tx.send(Command::Dtmf(key));
    }

    /// Microphone audio, 8 kHz mono, any length; sent in 20 ms packets.
    pub fn send_audio(&self, pcm: &[i16]) {
        if let Some(out) = self.audio.lock().unwrap().as_mut() {
            out.push(pcm);
        }
    }
}

impl Drop for Phone {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
    }
}

// ---------------------------------------------------------------- media

const FRAME: usize = 160; // 20 ms at 8 kHz

/// The voice sender: G.711 RTP from the call's audio socket.
struct AudioOut {
    sock: UdpSocket,
    dest: SocketAddr,
    law: Law,
    dtmf_pt: Option<u8>,
    seq: u16,
    timestamp: u32,
    ssrc: u32,
    pending: Vec<i16>,
    first: bool,
}

impl AudioOut {
    fn packet(&mut self, pt: u8, marker: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = Vec::with_capacity(12 + payload.len());
        p.push(0x80);
        p.push(if marker { 0x80 | pt } else { pt });
        p.extend_from_slice(&self.seq.to_be_bytes());
        p.extend_from_slice(&self.timestamp.to_be_bytes());
        p.extend_from_slice(&self.ssrc.to_be_bytes());
        p.extend_from_slice(payload);
        self.seq = self.seq.wrapping_add(1);
        p
    }

    fn push(&mut self, pcm: &[i16]) {
        self.pending.extend_from_slice(pcm);
        while self.pending.len() >= FRAME {
            let frame: Vec<i16> = self.pending.drain(..FRAME).collect();
            let payload = self.law.encode(&frame);
            let first = std::mem::take(&mut self.first);
            let p = self.packet(self.law.payload_type(), first, &payload);
            let _ = self.sock.send_to(&p, self.dest);
            self.timestamp = self.timestamp.wrapping_add(FRAME as u32);
        }
    }

    /// RFC 4733 tone: three start packets, then three end packets.
    fn tone(&mut self, key: char) -> bool {
        let Some(pt) = self.dtmf_pt else {
            return false;
        };
        let event = match key {
            '0'..='9' => key as u8 - b'0',
            '*' => 10,
            '#' => 11,
            _ => return false,
        };
        for (i, (end, duration)) in [
            (false, 400u16),
            (false, 800),
            (false, 1200),
            (true, 1600),
            (true, 1600),
            (true, 1600),
        ]
        .into_iter()
        .enumerate()
        {
            let mut body = vec![event, if end { 0x80 | 10 } else { 10 }];
            body.extend_from_slice(&duration.to_be_bytes());
            let p = self.packet(pt, i == 0, &body);
            let _ = self.sock.send_to(&p, self.dest);
            std::thread::sleep(Duration::from_millis(20));
        }
        self.timestamp = self.timestamp.wrapping_add(1600);
        true
    }
}

/// A call's sockets and receiver threads.
struct Media {
    audio: UdpSocket,
    video: UdpSocket,
    stop: Arc<AtomicBool>,
}

impl Media {
    fn open() -> std::io::Result<Media> {
        let audio = UdpSocket::bind("0.0.0.0:0")?;
        let video = UdpSocket::bind("0.0.0.0:0")?;
        audio.set_read_timeout(Some(Duration::from_millis(250)))?;
        video.set_read_timeout(Some(Duration::from_millis(250)))?;
        Ok(Media {
            audio,
            video,
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    fn ports(&self) -> (u16, u16) {
        (
            self.audio.local_addr().map(|a| a.port()).unwrap_or(0),
            self.video.local_addr().map(|a| a.port()).unwrap_or(0),
        )
    }

    /// Start receiving and set up the voice sender.
    fn begin(
        &self,
        agreed: &Agreed,
        sink: MediaSink,
        out: &Arc<Mutex<Option<AudioOut>>>,
        diag: Diag,
    ) {
        diag.line(&format!(
            "agreed: voice to {:?} ({:?}, tones {:?}); video from {:?} (payload {:?}, sprop {})",
            agreed.audio,
            agreed.law,
            agreed.dtmf_pt,
            agreed.video,
            agreed.video_pt,
            agreed.sprop.is_some()
        ));
        if let (Some(dest), Some(law)) = (agreed.audio, agreed.law) {
            if let Ok(sock) = self.audio.try_clone() {
                let mut sender = AudioOut {
                    sock,
                    dest,
                    law,
                    dtmf_pt: agreed.dtmf_pt,
                    seq: rand_u16(),
                    timestamp: rand_u32(),
                    ssrc: rand_u32(),
                    pending: Vec::new(),
                    first: true,
                };
                // A little silence opens the way through NAT and relays
                // before the microphone starts.
                sender.push(&[0i16; FRAME * 5]);
                *out.lock().unwrap() = Some(sender);
            }
            if let Ok(sock) = self.audio.try_clone() {
                let stop = self.stop.clone();
                let sink = sink.clone();
                let diag = diag.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 2048];
                    let mut packets = 0u64;
                    let mut from = None;
                    let mut last = Instant::now();
                    while !stop.load(Ordering::SeqCst) {
                        if last.elapsed() > Duration::from_secs(2) {
                            diag.line(&format!("voice: {packets} packets from {from:?}"));
                            last = Instant::now();
                        }
                        let Ok((n, peer)) = sock.recv_from(&mut buf) else {
                            continue;
                        };
                        packets += 1;
                        from = Some(peer);
                        let Some((_, _, _, payload)) = super::h264::parse_rtp(&buf[..n]) else {
                            continue;
                        };
                        if let Some(law) = Law::from_payload_type(buf[1] & 0x7F) {
                            sink(MediaOut::Audio(law.decode(payload)));
                        }
                    }
                });
            }
        }
        if let (Some(dest), Some(pt)) = (agreed.video, agreed.video_pt) {
            if let Ok(sock) = self.video.try_clone() {
                let stop = self.stop.clone();
                let sprop = agreed.sprop.clone();
                let diag = diag.clone();
                std::thread::spawn(move || {
                    let mut depack = Depacketizer::new();
                    if let Some(s) = &sprop {
                        depack.set_parameter_sets(s);
                    }
                    let mut buf = vec![0u8; 65536];
                    let mut last_ping = Instant::now() - Duration::from_secs(10);
                    let ssrc = rand_u32();
                    let mut seq = rand_u16();
                    let mut seen = 0u64;
                    let mut other_pt = 0u64;
                    let mut from = None;
                    let mut last_log = Instant::now();
                    while !stop.load(Ordering::SeqCst) {
                        if last_log.elapsed() > Duration::from_secs(2) {
                            let st = &depack.stats;
                            let types: Vec<String> = st
                                .nal_types
                                .iter()
                                .enumerate()
                                .filter(|(_, n)| **n > 0)
                                .map(|(t, n)| format!("{t}:{n}"))
                                .collect();
                            diag.line(&format!(
                                "video: {seen} packets from {from:?} ({other_pt} other payload types); depacketized {} packets into {} pictures ({} keyframes), dropped {} damaged and {} waiting for a keyframe; NAL types {}",
                                st.packets, st.units, st.keyframes, st.damaged, st.waiting, types.join(" ")
                            ));
                            last_log = Instant::now();
                        }
                        // We send no video, so a bare RTP header now and then
                        // keeps NAT and the relay pointing back at us.
                        if last_ping.elapsed() > Duration::from_secs(2) {
                            let mut p = vec![0x80, pt];
                            p.extend_from_slice(&seq.to_be_bytes());
                            p.extend_from_slice(&0u32.to_be_bytes());
                            p.extend_from_slice(&ssrc.to_be_bytes());
                            let _ = sock.send_to(&p, dest);
                            seq = seq.wrapping_add(1);
                            last_ping = Instant::now();
                        }
                        let Ok((n, peer)) = sock.recv_from(&mut buf) else {
                            continue;
                        };
                        seen += 1;
                        from = Some(peer);
                        if n < 12 || buf[1] & 0x7F != pt {
                            if other_pt == 0 && n >= 2 {
                                diag.line(&format!(
                                    "video: first packet with payload type {}",
                                    buf[1] & 0x7F
                                ));
                            }
                            other_pt += 1;
                            continue;
                        }
                        if let Some(unit) = depack.push(&buf[..n]) {
                            sink(MediaOut::Video(unit));
                        }
                    }
                });
            }
        }
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A per-call diagnostic log in the system temp folder (`kagaz-call.log`),
/// replaced by each call: the media descriptions and what arrived. It
/// stays on this device; it holds addresses, never passwords.
#[derive(Clone)]
struct Diag(Arc<Mutex<Option<std::fs::File>>>);

impl Diag {
    fn new() -> Diag {
        let file = std::fs::File::create(std::env::temp_dir().join("kagaz-call.log")).ok();
        Diag(Arc::new(Mutex::new(file)))
    }

    fn line(&self, text: &str) {
        if let Some(f) = self.0.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{text}");
        }
    }
}

fn rand_u32() -> u32 {
    let b = uuid::Uuid::new_v4();
    let b = b.as_bytes();
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn rand_u16() -> u16 {
    rand_u32() as u16
}

// ---------------------------------------------------------- signalling

/// Messages from the TCP reader thread.
enum Wire {
    Msg(Message),
    Closed(String),
}

/// Split a byte stream into SIP messages (CRLF keep-alives skipped).
#[derive(Default)]
pub(crate) struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    pub(crate) fn feed(&mut self, data: &[u8]) -> Vec<Message> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            while self.buf.starts_with(b"\r\n") {
                self.buf.drain(..2);
            }
            let Some(end) = find(&self.buf, b"\r\n\r\n") else {
                break;
            };
            let head = String::from_utf8_lossy(&self.buf[..end]).into_owned();
            let Ok(mut msg) = sip::parse(&head) else {
                self.buf.drain(..end + 4);
                continue;
            };
            let len: usize = msg
                .header("Content-Length")
                .or_else(|| msg.header("l"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            if self.buf.len() < end + 4 + len {
                break;
            }
            msg.body = String::from_utf8_lossy(&self.buf[end + 4..end + 4 + len]).into_owned();
            self.buf.drain(..end + 4 + len);
            out.push(msg);
        }
        out
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum State {
    /// Incoming, not answered.
    Ringing,
    /// Outgoing, no final answer yet.
    Calling,
    /// Talking.
    Confirmed,
}

/// One call's SIP dialog.
struct Dialog {
    id: String,
    call_id: String,
    state: State,
    incoming: bool,
    local_tag: String,
    /// Our From in requests we send in this dialog (with our tag).
    local: String,
    /// Our To in requests (with their tag once known).
    remote: String,
    /// Where in-dialog requests go (their Contact).
    target: String,
    route: Vec<String>,
    cseq: u32,
    /// UAS: the INVITE to answer. UAC: the INVITE we sent (for CANCEL/auth).
    invite: Option<Message>,
    invite_raw: String,
    invite_branch: String,
    auth_tried: bool,
    media: Option<Media>,
    agreed: Agreed,
    started: Instant,
    number: String,
    diag: Diag,
}

struct Line {
    account: SipAccount,
    writer: TcpStream,
    local: String,
    wire: Receiver<Wire>,
    events: EventSink,
    media: MediaSink,
    audio: Arc<Mutex<Option<AudioOut>>>,
    reg_call_id: String,
    reg_tag: String,
    reg_cseq: u32,
    reg_auth_tried: bool,
    reg_due: Instant,
    registered: bool,
    last_ping: Instant,
    call: Option<Dialog>,
    /// Rings of our own outgoing call to our own number, coming back to us:
    /// left ringing (never answered busy, which could end the call for the
    /// other phones too) until the server cancels them.
    own_legs: Vec<(Message, String)>,
}

fn connect(account: &SipAccount) -> Result<(TcpStream, Receiver<Wire>, String), String> {
    use std::net::ToSocketAddrs;
    let addr = (account.server.as_str(), account.port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("the call server has no address")?;
    let stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);
    let local = stream.local_addr().map_err(|e| e.to_string())?.to_string();
    let mut reader = stream.try_clone().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut framer = Framer::default();
        let mut buf = vec![0u8; 16384];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(Wire::Closed("the call server closed the connection".into()));
                    return;
                }
                Ok(n) => {
                    for m in framer.feed(&buf[..n]) {
                        if tx.send(Wire::Msg(m)).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Wire::Closed(e.to_string()));
                    return;
                }
            }
        }
    });
    Ok((stream, rx, local))
}

fn run(
    account: SipAccount,
    commands: Receiver<Command>,
    events: EventSink,
    media: MediaSink,
    audio: Arc<Mutex<Option<AudioOut>>>,
) {
    let mut backoff = Duration::from_secs(2);
    loop {
        let (writer, wire, local) = match connect(&account) {
            Ok(c) => c,
            Err(e) => {
                events(Event::Line {
                    up: false,
                    detail: format!("cannot reach the call server: {e}"),
                });
                if wait_or_stop(&commands, backoff) {
                    return;
                }
                backoff = (backoff * 2).min(Duration::from_secs(60));
                continue;
            }
        };
        let mut line = Line {
            account: account.clone(),
            writer,
            local,
            wire,
            events: events.clone(),
            media: media.clone(),
            audio: audio.clone(),
            reg_call_id: token(),
            reg_tag: token(),
            reg_cseq: 0,
            reg_auth_tried: false,
            reg_due: Instant::now(),
            registered: false,
            last_ping: Instant::now(),
            call: None,
            own_legs: Vec::new(),
        };
        match line.serve(&commands) {
            Served::Stopped => {
                line.unregister();
                return;
            }
            Served::Dropped(reason) => {
                if line.registered {
                    backoff = Duration::from_secs(2);
                }
                line.end_call(&reason);
                events(Event::Line {
                    up: false,
                    detail: reason,
                });
                if wait_or_stop(&commands, backoff) {
                    return;
                }
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

/// Wait out a reconnect delay; true when told to stop meanwhile.
fn wait_or_stop(commands: &Receiver<Command>, delay: Duration) -> bool {
    let until = Instant::now() + delay;
    while Instant::now() < until {
        match commands.recv_timeout(Duration::from_millis(200)) {
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return true,
            _ => {}
        }
    }
    false
}

enum Served {
    Stopped,
    Dropped(String),
}

impl Line {
    fn proto(&self) -> &'static str {
        if self.account.over_tcp() {
            "TCP"
        } else {
            "UDP"
        }
    }

    fn server_uri(&self) -> String {
        format!("sip:{}:{}", self.account.server, self.account.port)
    }

    fn contact(&self) -> String {
        format!(
            "<sip:{}@{};transport={}>",
            self.account.username,
            self.local,
            self.proto().to_lowercase()
        )
    }

    fn via(&self, branch: &str) -> String {
        format!(
            "SIP/2.0/{} {};branch={branch};rport",
            self.proto(),
            self.local
        )
    }

    fn local_ip(&self) -> String {
        self.local
            .rsplit_once(':')
            .map(|(ip, _)| ip.to_string())
            .unwrap_or_default()
    }

    fn send(&mut self, raw: &str) -> Result<(), String> {
        self.writer
            .write_all(raw.as_bytes())
            .and_then(|()| self.writer.flush())
            .map_err(|e| e.to_string())
    }

    fn serve(&mut self, commands: &Receiver<Command>) -> Served {
        if let Err(e) = self.register(None) {
            return Served::Dropped(e);
        }
        // The answer sets the real renewal time; this only covers silence.
        self.reg_due = Instant::now() + Duration::from_secs(30);
        loop {
            match self.wire.recv_timeout(Duration::from_millis(100)) {
                Ok(Wire::Msg(m)) => {
                    if let Err(e) = self.on_message(m) {
                        return Served::Dropped(e);
                    }
                }
                Ok(Wire::Closed(reason)) => return Served::Dropped(reason),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Served::Dropped("the connection reader stopped".into())
                }
            }
            loop {
                match commands.try_recv() {
                    Ok(Command::Stop) => return Served::Stopped,
                    Ok(c) => {
                        if let Err(e) = self.on_command(c) {
                            return Served::Dropped(e);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return Served::Stopped,
                }
            }
            if Instant::now() >= self.reg_due {
                self.reg_auth_tried = false;
                if let Err(e) = self.register(None) {
                    return Served::Dropped(e);
                }
                // Until the answer comes, don't ask again.
                self.reg_due = Instant::now() + Duration::from_secs(30);
            }
            if self.last_ping.elapsed() > Duration::from_secs(25) {
                self.last_ping = Instant::now();
                if let Err(e) = self.send("\r\n\r\n") {
                    return Served::Dropped(e);
                }
            }
            if let Some(d) = &self.call {
                if d.state == State::Calling && d.started.elapsed() > Duration::from_secs(60) {
                    let _ = self.on_command(Command::Hangup);
                }
            }
        }
    }

    // ---------------- registration

    fn register(&mut self, auth: Option<String>) -> Result<(), String> {
        self.reg_cseq += 1;
        let aor = self.account.aor();
        let branch = format!("z9hG4bK{}", token());
        let mut lines = vec![
            format!("REGISTER {} SIP/2.0", self.server_uri()),
            format!("Via: {}", self.via(&branch)),
            "Max-Forwards: 70".into(),
            format!(
                "From: \"{}\" <{aor}>;tag={}",
                self.account.display_name, self.reg_tag
            ),
            format!("To: <{aor}>"),
            format!("Call-ID: {}", self.reg_call_id),
            format!("CSeq: {} REGISTER", self.reg_cseq),
            format!("Contact: {}", self.contact()),
            format!("Expires: {}", self.account.expires),
            "Allow: INVITE, ACK, CANCEL, BYE, OPTIONS, INFO, NOTIFY, MESSAGE".into(),
            "User-Agent: Kagaz".into(),
        ];
        if let Some(a) = auth {
            lines.push(format!("Authorization: {a}"));
        }
        lines.push("Content-Length: 0".into());
        let raw = format!("{}\r\n\r\n", lines.join("\r\n"));
        self.send(&raw)
    }

    fn unregister(&mut self) {
        self.account.expires = 0;
        self.reg_auth_tried = false;
        if self.register(None).is_err() {
            return;
        }
        // Answer one challenge, then go.
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            match self.wire.recv_timeout(Duration::from_millis(200)) {
                Ok(Wire::Msg(m)) if is_register_response(&m) => match m.status() {
                    Some(401) | Some(407) if !self.reg_auth_tried => {
                        self.reg_auth_tried = true;
                        let auth = self.answer_challenge(&m, "REGISTER", &self.server_uri());
                        let _ = self.register(auth);
                    }
                    Some(100) => {}
                    _ => return,
                },
                Ok(Wire::Closed(_)) => return,
                _ => {}
            }
        }
    }

    fn answer_challenge(&self, m: &Message, method: &str, uri: &str) -> Option<String> {
        let challenge = m
            .header("WWW-Authenticate")
            .or_else(|| m.header("Proxy-Authenticate"))?;
        Some(digest_authorization(&self.account, method, uri, challenge))
    }

    fn on_register_response(&mut self, m: &Message) -> Result<(), String> {
        match m.status() {
            Some(100) => Ok(()),
            Some(401) | Some(407) if !self.reg_auth_tried => {
                self.reg_auth_tried = true;
                let auth = self.answer_challenge(m, "REGISTER", &self.server_uri());
                self.register(auth)
            }
            Some(200) => {
                self.reg_auth_tried = false;
                let expires = m
                    .header("Expires")
                    .and_then(|v| v.trim().parse().ok())
                    .or_else(|| {
                        m.header("Contact")
                            .and_then(|c| c.split("expires=").nth(1))
                            .and_then(|e| e.split([';', ',', ' ']).next()?.parse().ok())
                    })
                    .unwrap_or(self.account.expires)
                    .max(30);
                self.reg_due = Instant::now() + Duration::from_secs(u64::from(expires) * 4 / 5);
                if !self.registered {
                    self.registered = true;
                    (self.events)(Event::Line {
                        up: true,
                        detail: format!("signed in for {expires} s"),
                    });
                }
                Ok(())
            }
            Some(code) => Err(format!(
                "the call server refused the sign-in ({code} {})",
                m.reason()
            )),
            None => Ok(()),
        }
    }

    // ---------------- incoming messages

    fn on_message(&mut self, m: Message) -> Result<(), String> {
        if let Some(method) = m.method() {
            return self.on_request(&method, m);
        }
        if is_register_response(&m) {
            return self.on_register_response(&m);
        }
        self.on_call_response(m)
    }

    fn reply(
        &mut self,
        m: &Message,
        code: u16,
        reason: &str,
        tag: &str,
        sdp: Option<&str>,
    ) -> Result<(), String> {
        let contact = format!(
            "sip:{}@{};transport={}",
            self.account.username,
            self.local,
            self.proto().to_lowercase()
        );
        let with_contact =
            matches!(code, 180 | 183 | 200..=299) && m.method().as_deref() != Some("BYE");
        let raw = m.response_to(
            code,
            reason,
            tag,
            with_contact.then_some(contact.as_str()),
            sdp,
        );
        self.send(&raw)
    }

    fn same_call(&self, m: &Message) -> bool {
        self.call
            .as_ref()
            .is_some_and(|d| Some(d.call_id.as_str()) == m.header("Call-ID"))
    }

    fn on_request(&mut self, method: &str, m: Message) -> Result<(), String> {
        match method {
            "INVITE" => {
                if self.same_call(&m) {
                    let d = self.call.as_ref().unwrap();
                    let looped = d.state != State::Confirmed
                        || !m.header("To").is_some_and(|t| t.contains(&d.local_tag));
                    if looped {
                        // Our own call came back to us.
                        return self.reply(&m, 482, "Loop Detected", &token(), None);
                    }
                    // A re-INVITE in our dialog (session refresh): keep the media.
                    let tag = d.local_tag.clone();
                    let (sdp, _) = self.answer_sdp(&m);
                    return self.reply(&m, 200, "OK", &tag, Some(&sdp));
                }
                let (from_number, _) = caller(&m);
                let calling_own_line = self.call.as_ref().is_some_and(|d| {
                    d.state == State::Calling && !d.incoming && d.number == self.account.username
                });
                if calling_own_line && from_number == self.account.username {
                    let tag = token();
                    self.reply(&m, 180, "Ringing", &tag, None)?;
                    self.own_legs.push((m, tag));
                    return Ok(());
                }
                if self.call.is_some() {
                    return self.reply(&m, 486, "Busy Here", &token(), None);
                }
                self.reply(&m, 100, "Trying", "", None)?;
                let tag = token();
                self.reply(&m, 180, "Ringing", &tag, None)?;
                let (number, display) = caller(&m);
                let offer = sdp::parse(&m.body);
                let video = offer.video.as_ref().is_some_and(|v| v.port != 0);
                let id = token();
                self.call = Some(Dialog {
                    id: id.clone(),
                    call_id: m.header("Call-ID").unwrap_or_default().to_string(),
                    state: State::Ringing,
                    incoming: true,
                    local: format!("{};tag={tag}", m.header("To").unwrap_or_default()),
                    remote: m.header("From").unwrap_or_default().to_string(),
                    target: uri_of(m.header("Contact").unwrap_or_default()).unwrap_or_else(|| {
                        uri_of(m.header("From").unwrap_or_default()).unwrap_or_default()
                    }),
                    route: m
                        .headers_named("Record-Route")
                        .map(str::to_string)
                        .collect(),
                    local_tag: tag,
                    cseq: 1,
                    invite: Some(m),
                    invite_raw: String::new(),
                    invite_branch: String::new(),
                    auth_tried: false,
                    media: None,
                    agreed: Agreed::default(),
                    started: Instant::now(),
                    number: number.clone(),
                    diag: Diag::new(),
                });
                if let Some(d) = &self.call {
                    if let Some(inv) = &d.invite {
                        d.diag.line(&format!("incoming offer:\n{}", inv.body));
                    }
                }
                (self.events)(Event::Incoming {
                    id,
                    number,
                    display,
                    video,
                });
                Ok(())
            }
            "ACK" => Ok(()),
            "CANCEL" => {
                self.reply(&m, 200, "OK", "", None)?;
                let cid = m.header("Call-ID").unwrap_or_default().to_string();
                if let Some(i) = self
                    .own_legs
                    .iter()
                    .position(|(inv, _)| inv.header("Call-ID") == Some(cid.as_str()))
                {
                    let (inv, tag) = self.own_legs.remove(i);
                    return self.reply(&inv, 487, "Request Terminated", &tag, None);
                }
                if self.same_call(&m) {
                    let d = self.call.as_ref().unwrap();
                    if d.state == State::Ringing {
                        if let Some(inv) = d.invite.clone() {
                            let tag = d.local_tag.clone();
                            self.reply(&inv, 487, "Request Terminated", &tag, None)?;
                        }
                        let elsewhere = m.header("Reason").is_some_and(|r| {
                            r.contains("200") || r.to_lowercase().contains("elsewhere")
                        });
                        self.end_call(if elsewhere {
                            "answered elsewhere"
                        } else {
                            "missed"
                        });
                    }
                }
                Ok(())
            }
            "BYE" => {
                self.reply(&m, 200, "OK", "", None)?;
                if self.same_call(&m) {
                    self.end_call("hung up");
                }
                Ok(())
            }
            "OPTIONS" | "NOTIFY" | "MESSAGE" | "INFO" | "UPDATE" => {
                self.reply(&m, 200, "OK", "", None)
            }
            _ => self.reply(&m, 501, "Not Implemented", "", None),
        }
    }

    fn answer_sdp(&mut self, invite: &Message) -> (String, Agreed) {
        let ports = self
            .call
            .as_ref()
            .and_then(|d| d.media.as_ref())
            .map(Media::ports)
            .unwrap_or((0, 0));
        sdp::answer(
            &sdp::parse(&invite.body),
            &self.local_ip(),
            ports.0,
            ports.1,
        )
    }

    fn on_call_response(&mut self, m: Message) -> Result<(), String> {
        if !self.same_call(&m) {
            return Ok(());
        }
        let cseq_method = m
            .header("CSeq")
            .and_then(|c| c.split_whitespace().nth(1))
            .unwrap_or("")
            .to_uppercase();
        if cseq_method != "INVITE" {
            return Ok(());
        }
        let code = m.status().unwrap_or(0);
        let d = self.call.as_mut().unwrap();
        if d.state != State::Calling {
            // A repeated 200 OK: ACK it again.
            if (200..300).contains(&code) {
                let ack = self.ack_for_2xx(&m);
                return self.send(&ack);
            }
            return Ok(());
        }
        match code {
            100 => Ok(()),
            180 | 183 => {
                let id = d.id.clone();
                (self.events)(Event::Ringing { id });
                Ok(())
            }
            200..=299 => {
                if let Some(to) = m.header("To") {
                    d.remote = to.to_string();
                }
                if let Some(c) = m.header("Contact").and_then(uri_of) {
                    d.target = c;
                }
                d.route = m
                    .headers_named("Record-Route")
                    .map(str::to_string)
                    .collect();
                d.route.reverse();
                d.state = State::Confirmed;
                d.diag.line(&format!("their answer:\n{}", m.body));
                d.agreed = sdp::agreed_from_answer(&sdp::parse(&m.body));
                let ack = self.ack_for_2xx(&m);
                self.send(&ack)?;
                self.start_media();
                Ok(())
            }
            401 | 407 => {
                // ACK the challenge, then send the INVITE again with credentials.
                let ack = ack_for_failure(d, &m);
                let tried = d.auth_tried;
                self.send(&ack)?;
                if tried {
                    self.end_call("the call server refused the call");
                    return Ok(());
                }
                let target = format!(
                    "sip:{}@{}:{}",
                    self.call.as_ref().unwrap().number,
                    self.account.server,
                    self.account.port
                );
                let header = if code == 407 {
                    "Proxy-Authorization"
                } else {
                    "Authorization"
                };
                let auth = self.answer_challenge(&m, "INVITE", &target);
                let branch = format!("z9hG4bK{}", token());
                let via = self.via(&branch);
                let d = self.call.as_mut().unwrap();
                d.auth_tried = true;
                d.cseq += 1;
                d.invite_branch = branch;
                let mut raw = d.invite_raw.clone();
                raw = replace_header(&raw, "CSeq", &format!("{} INVITE", d.cseq));
                raw = replace_header(&raw, "Via", &via);
                if let Some(a) = auth {
                    raw = add_header(&raw, header, &a);
                }
                d.invite_raw = raw.clone();
                self.send(&raw)
            }
            300..=699 => {
                let ack = ack_for_failure(d, &m);
                self.send(&ack)?;
                let reason = match code {
                    486 | 600 => "busy".to_string(),
                    603 => "declined".to_string(),
                    404 | 480 => "not reachable".to_string(),
                    487 => "cancelled".to_string(),
                    _ => format!("{code} {}", m.reason()),
                };
                self.end_call(&reason);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn ack_for_2xx(&self, ok: &Message) -> String {
        let d = self.call.as_ref().unwrap();
        let cseq = ok
            .header("CSeq")
            .and_then(|c| c.split_whitespace().next())
            .unwrap_or("1")
            .to_string();
        let mut lines = vec![
            format!("ACK {} SIP/2.0", d.target),
            format!("Via: {}", self.via(&format!("z9hG4bK{}", token()))),
            "Max-Forwards: 70".into(),
        ];
        for r in &d.route {
            lines.push(format!("Route: {r}"));
        }
        lines.extend([
            format!("From: {}", d.local),
            format!("To: {}", d.remote),
            format!("Call-ID: {}", d.call_id),
            format!("CSeq: {cseq} ACK"),
            "Content-Length: 0".into(),
        ]);
        format!("{}\r\n\r\n", lines.join("\r\n"))
    }

    // ---------------- commands

    fn on_command(&mut self, c: Command) -> Result<(), String> {
        match c {
            Command::Answer => {
                let Some(d) = self.call.as_mut() else {
                    return Ok(());
                };
                if d.state != State::Ringing {
                    return Ok(());
                }
                let media = Media::open().map_err(|e| e.to_string())?;
                d.media = Some(media);
                let invite = d.invite.clone().unwrap();
                let tag = d.local_tag.clone();
                let (sdp, agreed) = self.answer_sdp(&invite);
                self.reply(&invite, 200, "OK", &tag, Some(&sdp))?;
                let d = self.call.as_mut().unwrap();
                d.diag.line(&format!("our answer:\n{sdp}"));
                d.agreed = agreed;
                d.state = State::Confirmed;
                self.start_media();
                Ok(())
            }
            Command::Decline => {
                let Some(d) = self.call.as_ref() else {
                    return Ok(());
                };
                if d.state != State::Ringing {
                    return Ok(());
                }
                // 486 turns down only this phone; the other devices keep
                // ringing (603 would end the call for all of them).
                let invite = d.invite.clone().unwrap();
                let tag = d.local_tag.clone();
                self.reply(&invite, 486, "Busy Here", &tag, None)?;
                self.end_call("declined");
                Ok(())
            }
            Command::Hangup => {
                let Some(d) = self.call.as_ref() else {
                    return Ok(());
                };
                match d.state {
                    State::Ringing => self.on_command(Command::Decline),
                    State::Calling => {
                        let cancel = cancel_for(d);
                        self.send(&cancel)?;
                        self.end_call("cancelled");
                        Ok(())
                    }
                    State::Confirmed => {
                        let bye = self.in_dialog("BYE", None);
                        self.send(&bye)?;
                        self.end_call("you hung up");
                        Ok(())
                    }
                }
            }
            Command::Call(number) => {
                if self.call.is_some() {
                    return Ok(());
                }
                if !self.registered {
                    (self.events)(Event::Ended {
                        id: String::new(),
                        reason: "not signed in to the call server yet".into(),
                    });
                    return Ok(());
                }
                self.place_call(&number)
            }
            Command::Dtmf(key) => {
                if !self
                    .call
                    .as_ref()
                    .is_some_and(|d| d.state == State::Confirmed)
                {
                    return Ok(());
                }
                let sent = self
                    .audio
                    .lock()
                    .unwrap()
                    .as_mut()
                    .is_some_and(|a| a.tone(key));
                // SIP INFO too: some stations listen only for that.
                let body = super::media::dtmf_info_body(key);
                let info = self.in_dialog("INFO", Some(("application/dtmf-relay", &body)));
                let _ = sent;
                self.send(&info)
            }
            Command::Stop => Ok(()),
        }
    }

    fn place_call(&mut self, number: &str) -> Result<(), String> {
        let media = Media::open().map_err(|e| e.to_string())?;
        let (ap, vp) = media.ports();
        let body = sdp::offer(&self.local_ip(), ap, vp);
        let tag = token();
        let call_id = token();
        let branch = format!("z9hG4bK{}", token());
        let target = format!(
            "sip:{number}@{}:{};transport={}",
            self.account.server,
            self.account.port,
            self.proto().to_lowercase()
        );
        let from = format!(
            "\"{}\" <{}>;tag={tag}",
            self.account.display_name,
            self.account.aor()
        );
        let to = format!("<sip:{number}@{}>", self.account.server);
        let raw = format!(
            "INVITE {target} SIP/2.0\r\nVia: {}\r\nMax-Forwards: 70\r\nFrom: {from}\r\nTo: {to}\r\nCall-ID: {call_id}\r\nCSeq: 1 INVITE\r\nContact: {}\r\nAllow: INVITE, ACK, CANCEL, BYE, OPTIONS, INFO, NOTIFY, MESSAGE\r\nUser-Agent: Kagaz\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{body}",
            self.via(&branch),
            self.contact(),
            body.len()
        );
        let id = token();
        self.call = Some(Dialog {
            id: id.clone(),
            call_id,
            state: State::Calling,
            incoming: false,
            local_tag: tag,
            local: from,
            remote: to,
            target,
            route: Vec::new(),
            cseq: 1,
            invite: None,
            invite_raw: raw.clone(),
            invite_branch: branch,
            auth_tried: false,
            media: Some(media),
            agreed: Agreed::default(),
            started: Instant::now(),
            number: number.to_string(),
            diag: Diag::new(),
        });
        if let Some(d) = &self.call {
            d.diag.line(&format!("our offer:\n{body}"));
        }
        (self.events)(Event::Outgoing {
            id,
            number: number.to_string(),
        });
        self.send(&raw)
    }

    /// A request inside the current dialog (BYE, INFO).
    fn in_dialog(&mut self, method: &str, body: Option<(&str, &str)>) -> String {
        let via = self.via(&format!("z9hG4bK{}", token()));
        let d = self.call.as_mut().unwrap();
        d.cseq += 1;
        let mut lines = vec![
            format!("{method} {} SIP/2.0", d.target),
            format!("Via: {via}"),
            "Max-Forwards: 70".into(),
        ];
        for r in &d.route {
            lines.push(format!("Route: {r}"));
        }
        lines.extend([
            format!("From: {}", d.local),
            format!("To: {}", d.remote),
            format!("Call-ID: {}", d.call_id),
            format!("CSeq: {} {method}", d.cseq),
            "User-Agent: Kagaz".into(),
        ]);
        let content = body.map(|(_, b)| b).unwrap_or("");
        if let Some((kind, _)) = body {
            lines.push(format!("Content-Type: {kind}"));
        }
        lines.push(format!("Content-Length: {}", content.len()));
        format!("{}\r\n\r\n{content}", lines.join("\r\n"))
    }

    fn start_media(&mut self) {
        let Some(d) = self.call.as_mut() else {
            return;
        };
        if let Some(m) = &d.media {
            m.begin(&d.agreed, self.media.clone(), &self.audio, d.diag.clone());
        }
        let video = d.agreed.video.is_some();
        let id = d.id.clone();
        (self.events)(Event::Connected { id, video });
    }

    fn end_call(&mut self, reason: &str) {
        *self.audio.lock().unwrap() = None;
        for (inv, tag) in std::mem::take(&mut self.own_legs) {
            let _ = self.reply(&inv, 487, "Request Terminated", &tag, None);
        }
        if let Some(d) = self.call.take() {
            d.diag.line(&format!("ended: {reason}"));
            drop(d.media);
            (self.events)(Event::Ended {
                id: d.id,
                reason: reason.to_string(),
            });
            let _ = d.incoming;
        }
    }
}

fn is_register_response(m: &Message) -> bool {
    m.status().is_some()
        && m.header("CSeq")
            .is_some_and(|c| c.to_uppercase().ends_with("REGISTER"))
}

/// The ACK for a failure response to our INVITE (same branch, their To tag).
fn ack_for_failure(d: &Dialog, m: &Message) -> String {
    format!(
        "ACK {} SIP/2.0\r\nVia: {}\r\nMax-Forwards: 70\r\nFrom: {}\r\nTo: {}\r\nCall-ID: {}\r\nCSeq: {} ACK\r\nContent-Length: 0\r\n\r\n",
        request_uri(&d.invite_raw),
        header_of(&d.invite_raw, "Via").unwrap_or_default(),
        d.local,
        m.header("To").unwrap_or(&d.remote),
        d.call_id,
        d.cseq
    )
}

/// CANCEL for our pending INVITE: same request URI, Via and CSeq number.
fn cancel_for(d: &Dialog) -> String {
    format!(
        "CANCEL {} SIP/2.0\r\nVia: {}\r\nMax-Forwards: 70\r\nFrom: {}\r\nTo: {}\r\nCall-ID: {}\r\nCSeq: {} CANCEL\r\nContent-Length: 0\r\n\r\n",
        request_uri(&d.invite_raw),
        header_of(&d.invite_raw, "Via").unwrap_or_default(),
        d.local,
        d.remote,
        d.call_id,
        d.cseq
    )
}

fn request_uri(raw: &str) -> String {
    raw.lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or_default()
        .to_string()
}

fn header_of(raw: &str, name: &str) -> Option<String> {
    raw.lines().take_while(|l| !l.is_empty()).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}

fn replace_header(raw: &str, name: &str, value: &str) -> String {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let lines: Vec<String> = head
        .split("\r\n")
        .map(|l| match l.split_once(':') {
            Some((k, _)) if k.trim().eq_ignore_ascii_case(name) => format!("{name}: {value}"),
            _ => l.to_string(),
        })
        .collect();
    format!("{}\r\n\r\n{body}", lines.join("\r\n"))
}

fn add_header(raw: &str, name: &str, value: &str) -> String {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    format!("{head}\r\n{name}: {value}\r\n\r\n{body}")
}

/// The URI inside a header value like `"Name" <sip:1@host>;tag=x`.
fn uri_of(value: &str) -> Option<String> {
    if let Some(start) = value.find('<') {
        let rest = &value[start + 1..];
        return rest.find('>').map(|end| rest[..end].to_string());
    }
    let v = value.split(';').next()?.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// (number, display name) of the caller in an INVITE.
fn caller(m: &Message) -> (String, String) {
    let from = m.header("From").unwrap_or_default();
    let number = uri_of(from)
        .and_then(|u| {
            let u = u.trim_start_matches("sip:").trim_start_matches("sips:");
            u.split('@').next().map(str::to_string)
        })
        .unwrap_or_default();
    let display = from
        .split('<')
        .next()
        .map(|d| d.trim().trim_matches('"').to_string())
        .unwrap_or_default();
    (number, display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_messages_out_of_a_tcp_stream() {
        let mut f = Framer::default();
        let a = "SIP/2.0 200 OK\r\nCSeq: 2 REGISTER\r\nContent-Length: 0\r\n\r\n";
        let b = "INVITE sip:1@192.0.2.6 SIP/2.0\r\nCall-ID: x\r\nContent-Length: 5\r\n\r\nhello";
        let stream = format!("\r\n\r\n{a}{b}");
        // Arrives in awkward pieces.
        let mut got = Vec::new();
        for chunk in stream.as_bytes().chunks(7) {
            got.extend(f.feed(chunk));
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].status(), Some(200));
        assert!(is_register_response(&got[0]));
        assert_eq!(got[1].method().as_deref(), Some("INVITE"));
        assert_eq!(got[1].body, "hello");
    }

    #[test]
    fn reads_the_caller_and_uris() {
        let m = sip::parse(
            "INVITE sip:20011001@192.0.2.6 SIP/2.0\r\nFrom: \"Lobby\" <sip:20011002@203.0.113.10:5070>;tag=9\r\n",
        )
        .unwrap();
        assert_eq!(caller(&m), ("20011002".into(), "Lobby".into()));
        assert_eq!(
            uri_of("<sip:a@b;transport=tcp>;expires=60").unwrap(),
            "sip:a@b;transport=tcp"
        );
        assert_eq!(uri_of("sip:a@b;tag=1").unwrap(), "sip:a@b");
    }

    #[test]
    fn rewrites_headers_of_a_request() {
        let raw =
            "INVITE sip:x SIP/2.0\r\nVia: old\r\nCSeq: 1 INVITE\r\nContent-Length: 2\r\n\r\nhi";
        let r = replace_header(raw, "CSeq", "2 INVITE");
        let r = add_header(&r, "Proxy-Authorization", "Digest x");
        assert!(r.contains("CSeq: 2 INVITE\r\n"));
        assert!(r.ends_with("Proxy-Authorization: Digest x\r\n\r\nhi"));
        assert_eq!(header_of(&r, "via").as_deref(), Some("old"));
        assert_eq!(request_uri(&r), "sip:x");
    }

    #[test]
    fn a_dtmf_tone_is_six_rfc4733_packets() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut out = AudioOut {
            sock: UdpSocket::bind("127.0.0.1:0").unwrap(),
            dest: rx.local_addr().unwrap(),
            law: Law::Mu,
            dtmf_pt: Some(101),
            seq: 1,
            timestamp: 0,
            ssrc: 7,
            pending: Vec::new(),
            first: true,
        };
        assert!(out.tone('#'));
        let mut buf = [0u8; 512];
        let mut ends = 0;
        for i in 0..6 {
            let n = rx.recv(&mut buf).unwrap();
            assert_eq!(n, 16);
            assert_eq!(buf[1] & 0x7F, 101);
            assert_eq!(buf[1] & 0x80 != 0, i == 0);
            assert_eq!(buf[12], 11);
            if buf[13] & 0x80 != 0 {
                ends += 1;
            }
        }
        assert_eq!(ends, 3);
        // Voice: 20 ms frames.
        out.push(&[0i16; 400]);
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(n, 12 + FRAME);
    }

    /// A whole incoming call against a fake server on loopback: sign-in
    /// with a challenge, a ring, answer, voice both ways, hang-up by the
    /// caller, and sign-out on stop.
    #[test]
    fn a_whole_call_against_a_fake_server() {
        use std::net::TcpListener;
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let door_audio = UdpSocket::bind("127.0.0.1:0").unwrap();
        door_audio
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let door_port = door_audio.local_addr().unwrap().port();

        let (etx, erx) = mpsc::channel();
        let (mtx, mrx) = mpsc::channel();
        let phone = Phone::start(
            SipAccount {
                username: "20011001".into(),
                password: "pw".into(),
                display_name: "Kagaz".into(),
                server: "127.0.0.1".into(),
                port,
                transport: "tcp".into(),
                expires: 300,
            },
            move |e| {
                let _ = etx.send(e);
            },
            move |m| {
                if let MediaOut::Audio(pcm) = m {
                    let _ = mtx.send(pcm.len());
                }
            },
        );
        let (mut conn, _) = server.accept().unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut framer = Framer::default();
        let mut next = |conn: &mut TcpStream| -> Message {
            let mut buf = [0u8; 8192];
            loop {
                let n = conn.read(&mut buf).unwrap();
                let mut got = framer.feed(&buf[..n]);
                if !got.is_empty() {
                    return got.remove(0);
                }
            }
        };
        let reg = next(&mut conn);
        assert!(reg.start.starts_with("REGISTER"));
        conn.write_all(
            reg.response_to(401, "Unauthorized", "", None, None)
                .replace(
                    "User-Agent: Kagaz",
                    "WWW-Authenticate: Digest realm=\"example\", nonce=\"n1\"",
                )
                .as_bytes(),
        )
        .unwrap();
        let reg2 = next(&mut conn);
        assert!(reg2
            .header("Authorization")
            .unwrap()
            .contains("username=\"20011001\""));
        conn.write_all(
            reg2.response_to(200, "OK", "", None, None)
                .replace("User-Agent: Kagaz", "Expires: 300")
                .as_bytes(),
        )
        .unwrap();
        assert!(matches!(
            erx.recv_timeout(Duration::from_secs(3)).unwrap(),
            Event::Line { up: true, .. }
        ));

        let sdp = format!("v=0\r\nc=IN IP4 127.0.0.1\r\nm=audio {door_port} RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\n");
        let invite = format!("INVITE sip:20011001@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:{port};branch=z9hG4bKa\r\nFrom: \"Lobby\" <sip:20011002@127.0.0.1>;tag=d1\r\nTo: <sip:20011001@127.0.0.1>\r\nCall-ID: call-1\r\nCSeq: 1 INVITE\r\nContact: <sip:20011002@127.0.0.1:{port};transport=tcp>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{sdp}", sdp.len());
        conn.write_all(invite.as_bytes()).unwrap();
        assert_eq!(next(&mut conn).status(), Some(100));
        assert_eq!(next(&mut conn).status(), Some(180));
        match erx.recv_timeout(Duration::from_secs(3)).unwrap() {
            Event::Incoming {
                number,
                display,
                video,
                ..
            } => {
                assert_eq!(
                    (number.as_str(), display.as_str(), video),
                    ("20011002", "Lobby", false)
                );
            }
            e => panic!("expected a ring, got {e:?}"),
        }
        phone.answer();
        let ok = next(&mut conn);
        assert_eq!(ok.status(), Some(200));
        let answer = sdp::parse(&ok.body);
        let kagaz_audio = answer.audio.unwrap().port;
        assert!(matches!(
            erx.recv_timeout(Duration::from_secs(3)).unwrap(),
            Event::Connected { video: false, .. }
        ));

        // The door hears Kagaz's opening silence, then the microphone.
        let mut buf = [0u8; 512];
        let (n, _) = door_audio.recv_from(&mut buf).unwrap();
        assert_eq!((n, buf[1] & 0x7F), (12 + FRAME, 0));
        phone.send_audio(&[1000i16; FRAME]);
        // Kagaz hears the door.
        let mut pkt = vec![0x80u8, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 9];
        pkt.extend_from_slice(&[0xFFu8; FRAME]);
        door_audio
            .send_to(&pkt, ("127.0.0.1", kagaz_audio))
            .unwrap();
        assert_eq!(mrx.recv_timeout(Duration::from_secs(3)).unwrap(), FRAME);

        let bye = format!("BYE sip:20011001@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:{port};branch=z9hG4bKb\r\nFrom: \"Lobby\" <sip:20011002@127.0.0.1>;tag=d1\r\nTo: <sip:20011001@127.0.0.1>;tag=x\r\nCall-ID: call-1\r\nCSeq: 2 BYE\r\nContent-Length: 0\r\n\r\n");
        conn.write_all(bye.as_bytes()).unwrap();
        loop {
            let m = next(&mut conn);
            if m.status() == Some(200) && m.header("CSeq").unwrap().contains("BYE") {
                break;
            }
        }
        match erx.recv_timeout(Duration::from_secs(3)).unwrap() {
            Event::Ended { reason, .. } => assert_eq!(reason, "hung up"),
            e => panic!("expected the end, got {e:?}"),
        }
        drop(phone);
        let unreg = next(&mut conn);
        assert!(unreg.start.starts_with("REGISTER"));
        assert_eq!(unreg.header("Expires"), Some("0"));
    }
}
