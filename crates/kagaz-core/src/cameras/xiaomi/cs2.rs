//! CS2, the peer-to-peer transport newer Xiaomi cameras (and some older
//! ones after a firmware update) use on the home network: a short UDP
//! handshake on port 32108, then numbered "DRW" packets on four channels
//! (0 commands to the app, 2 media to the app, 3 media to the camera),
//! each acknowledged; the camera may instead ask for the same over TCP.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PORT: u16 = 32108;
const MAGIC: u8 = 0xF1;
const MAGIC_DRW: u8 = 0xD1;
const MAGIC_TCP: u8 = 0x68;
const LAN_SEARCH: u8 = 0x30;
const PUNCH: u8 = 0x41;
const READY_UDP: u8 = 0x42;
const READY_TCP: u8 = 0x43;
const DRW: u8 = 0xD0;
const DRW_ACK: u8 = 0xD1;
const PING: u8 = 0xE0;
const PONG: u8 = 0xE1;
const CLOSE: u8 = 0xF1;
/// How many packets ahead of the next one a channel keeps.
const EARLY: usize = 256;

/// The socket a connection runs on.
#[derive(Clone)]
enum Link {
    Udp { sock: Arc<UdpSocket>, to: SocketAddr },
    Tcp(Arc<Mutex<TcpStream>>),
}

impl Link {
    fn send(&self, packet: &[u8]) -> std::io::Result<()> {
        match self {
            Link::Udp { sock, to } => sock.send_to(packet, to).map(|_| ()),
            Link::Tcp(stream) => {
                // TCP frames each packet: its length, a magic byte, padding.
                let mut framed = vec![0u8; 8 + packet.len()];
                framed[..2].copy_from_slice(&(packet.len() as u16).to_be_bytes());
                framed[2] = MAGIC_TCP;
                framed[8..].copy_from_slice(packet);
                stream.lock().map_err(|_| std::io::Error::other("poisoned"))?.write_all(&framed)
            }
        }
    }
}

pub struct Conn {
    link: Link,
    tcp: bool,
    commands: Receiver<Vec<u8>>,
    media: Receiver<Vec<u8>>,
    acks: Receiver<()>,
    seq_commands: u16,
    seq_media: u16,
    closed: Arc<AtomicBool>,
    /// Why the reader stopped, once it has.
    error: Arc<Mutex<Option<String>>>,
}

impl Conn {
    /// Connect to the camera at `ip`, over UDP, or TCP when it asks for it.
    pub fn dial(ip: Ipv4Addr) -> Result<Conn, String> {
        let sock = UdpSocket::bind(("0.0.0.0", 0)).map_err(|e| e.to_string())?;
        let mut to = SocketAddr::V4(SocketAddrV4::new(ip, PORT));
        let deadline = Instant::now() + Duration::from_secs(5);
        // 1. Who is there: the camera answers a LAN search with a punch packet.
        let punch = exchange(&sock, &mut to, &[MAGIC, LAN_SEARCH, 0, 0], deadline, |t| t == PUNCH)?;
        // 2. The punch packet sent back makes it ready, on UDP or TCP.
        let ready = exchange(&sock, &mut to, &punch, deadline, |t| t == READY_UDP || t == READY_TCP)?;
        let (link, tcp, tcp_in) = if ready[1] == READY_TCP {
            let stream = TcpStream::connect_timeout(&to, Duration::from_secs(3)).map_err(|e| format!("CS2 over TCP: {e}"))?;
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
            let reading = stream.try_clone().map_err(|e| e.to_string())?;
            (Link::Tcp(Arc::new(Mutex::new(stream))), true, Some(reading))
        } else {
            (Link::Udp { sock: Arc::new(sock), to }, false, None)
        };
        let (cmd_tx, commands) = sync_channel(16);
        let (media_tx, media) = sync_channel(400);
        let (ack_tx, acks) = sync_channel(64);
        let closed = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let reader = Reader {
            link: link.clone(),
            tcp_in,
            tcp,
            channels: [Channel::new(cmd_tx), Channel::new(media_tx)],
            acks: ack_tx,
            closed: closed.clone(),
            error: error.clone(),
        };
        std::thread::Builder::new()
            .name("kagaz-cs2".into())
            .spawn(move || reader.run())
            .map_err(|e| e.to_string())?;
        Ok(Conn {
            link,
            tcp,
            commands,
            media,
            acks,
            seq_commands: 0,
            seq_media: 0,
            closed,
            error,
        })
    }

    pub fn protocol(&self) -> &'static str {
        if self.tcp { "cs2+tcp" } else { "cs2+udp" }
    }

    fn why(&self) -> String {
        self.error
            .lock()
            .ok()
            .and_then(|e| e.clone())
            .unwrap_or_else(|| "the camera closed the connection".into())
    }

    /// The next command from the camera: its number and data.
    pub fn read_command(&self, wait: Duration) -> Result<(u32, Vec<u8>), String> {
        match self.commands.recv_timeout(wait) {
            Ok(b) if b.len() >= 4 => Ok((u32::from_be_bytes(b[..4].try_into().unwrap()), b[4..].to_vec())),
            Ok(_) => Err("a command too short to read".into()),
            Err(RecvTimeoutError::Timeout) => Err("the camera did not answer".into()),
            Err(RecvTimeoutError::Disconnected) => Err(self.why()),
        }
    }

    /// Send a command; over UDP, again each second until the camera
    /// acknowledges it (five tries).
    pub fn write_command(&mut self, cmd: u32, data: &[u8]) -> Result<(), String> {
        let mut packet = vec![0u8; 16 + data.len()];
        packet[0] = MAGIC;
        packet[1] = DRW;
        packet[2..4].copy_from_slice(&((12 + data.len()) as u16).to_be_bytes());
        packet[4] = MAGIC_DRW;
        packet[5] = 0;
        packet[6..8].copy_from_slice(&self.seq_commands.to_be_bytes());
        packet[8..12].copy_from_slice(&((4 + data.len()) as u32).to_be_bytes());
        packet[12..16].copy_from_slice(&cmd.to_be_bytes());
        packet[16..].copy_from_slice(data);
        self.seq_commands = self.seq_commands.wrapping_add(1);
        if self.tcp {
            return self.link.send(&packet).map_err(|e| e.to_string());
        }
        while self.acks.try_recv().is_ok() {}
        for _ in 0..5 {
            self.link.send(&packet).map_err(|e| e.to_string())?;
            match self.acks.recv_timeout(Duration::from_secs(1)) {
                Ok(()) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err(self.why()),
            }
        }
        Err(format!("the camera did not take command {cmd:#x}"))
    }

    /// The next media piece: its 32-byte header and payload.
    pub fn read_packet(&self, wait: Duration) -> Result<(Vec<u8>, Vec<u8>), String> {
        match self.media.recv_timeout(wait) {
            Ok(mut b) if b.len() >= 32 => {
                let payload = b.split_off(32);
                Ok((b, payload))
            }
            Ok(_) => Err("a media piece too short to read".into()),
            Err(RecvTimeoutError::Timeout) => Err("the camera stopped sending".into()),
            Err(RecvTimeoutError::Disconnected) => Err(self.why()),
        }
    }

    /// Send a media piece (talk-back sound) on channel 3.
    pub fn write_packet(&mut self, header: &[u8; 32], payload: &[u8]) -> Result<(), String> {
        let n = 32 + payload.len();
        let mut packet = vec![0u8; 12 + n];
        packet[0] = MAGIC;
        packet[1] = DRW;
        packet[2..4].copy_from_slice(&((n + 8) as u16).to_be_bytes());
        packet[4] = MAGIC_DRW;
        packet[5] = 3;
        packet[6..8].copy_from_slice(&self.seq_media.to_be_bytes());
        packet[8..12].copy_from_slice(&(n as u32).to_be_bytes());
        packet[12..44].copy_from_slice(header);
        packet[44..].copy_from_slice(payload);
        self.seq_media = self.seq_media.wrapping_add(1);
        self.link.send(&packet).map_err(|e| e.to_string())
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.link.send(&[MAGIC, CLOSE, 0, 0]);
        if let Link::Tcp(s) = &self.link {
            if let Ok(s) = s.lock() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

/// Send `request` each second until the camera answers with a packet of a
/// type `wanted` accepts; the camera's port is taken from its answer.
fn exchange(sock: &UdpSocket, to: &mut SocketAddr, request: &[u8], deadline: Instant, wanted: impl Fn(u8) -> bool) -> Result<Vec<u8>, String> {
    let mut buf = [0u8; 1500];
    let mut next_send = Instant::now();
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err("the camera did not answer on this network".into());
        }
        if now >= next_send {
            sock.send_to(request, *to).map_err(|e| e.to_string())?;
            next_send = now + Duration::from_secs(1);
        }
        let wait = next_send.min(deadline).saturating_duration_since(Instant::now()).max(Duration::from_millis(10));
        let _ = sock.set_read_timeout(Some(wait));
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if from.ip() != to.ip() || n < 16 {
                    continue;
                }
                if wanted(buf[1]) {
                    to.set_port(from.port());
                    return Ok(buf[..n].to_vec());
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// One incoming channel: packets put back in order (UDP), then split into
/// the messages they carry, each prefixed by its length.
struct Channel {
    wait_seq: u16,
    /// Packets that came before their turn, up to [`EARLY`].
    early: HashMap<u16, Vec<u8>>,
    pending: Vec<u8>,
    out: SyncSender<Vec<u8>>,
}

impl Channel {
    fn new(out: SyncSender<Vec<u8>>) -> Channel {
        Channel {
            wait_seq: 0,
            early: HashMap::new(),
            pending: Vec::new(),
            out,
        }
    }

    /// Bytes in order: split off every whole message.
    fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.pending.extend_from_slice(bytes);
        loop {
            if self.pending.len() < 4 {
                return Ok(());
            }
            let size = u32::from_be_bytes(self.pending[..4].try_into().unwrap()) as usize;
            if size > 4 << 20 {
                return Err("a message larger than any picture".into());
            }
            if self.pending.len() < 4 + size {
                return Ok(());
            }
            let msg = self.pending[4..4 + size].to_vec();
            self.pending.drain(..4 + size);
            self.out.try_send(msg).map_err(|_| "Kagaz fell behind the camera".to_string())?;
        }
    }

    /// A numbered packet (UDP): taken now if it is the next, kept if it is
    /// early, ignored if it came before. Returns whether to acknowledge it.
    fn push_seq(&mut self, seq: u16, data: &[u8]) -> Result<bool, String> {
        let diff = seq.wrapping_sub(self.wait_seq) as i16;
        if diff > 0 {
            if self.early.contains_key(&seq) {
                return Ok(true);
            }
            if self.early.len() >= EARLY {
                return Ok(false); // no room: the camera sends it again
            }
            self.early.insert(seq, data.to_vec());
            return Ok(true);
        }
        if diff < 0 {
            return Ok(true); // a repeat of one already taken
        }
        self.push(data)?;
        self.wait_seq = self.wait_seq.wrapping_add(1);
        while let Some(next) = self.early.remove(&self.wait_seq) {
            self.push(&next)?;
            self.wait_seq = self.wait_seq.wrapping_add(1);
        }
        Ok(true)
    }
}

struct Reader {
    link: Link,
    /// The reading half of a TCP connection.
    tcp_in: Option<TcpStream>,
    tcp: bool,
    /// Channel 0 (commands) and 2 (media).
    channels: [Channel; 2],
    acks: SyncSender<()>,
    closed: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}

impl Reader {
    fn run(mut self) {
        let why = self.read_all();
        if !self.closed.load(Ordering::SeqCst) {
            if let Ok(mut e) = self.error.lock() {
                *e = Some(why);
            }
        }
        // Dropping the channels' senders tells the readers it is over.
    }

    fn read_all(&mut self) -> String {
        let mut buf = vec![0u8; 2048];
        let mut next_ping = Instant::now();
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return String::new();
            }
            let n = match self.read(&mut buf) {
                Ok(Some(n)) => n,
                Ok(None) => continue,
                Err(e) => return e,
            };
            if n < 4 {
                continue;
            }
            match buf[1] {
                DRW if n >= 8 => {
                    let index = match buf[5] {
                        0 => 0,
                        2 => 1,
                        _ => continue,
                    };
                    let result = if self.tcp {
                        // The Mi Home app pings about every second over TCP.
                        if Instant::now() >= next_ping {
                            let _ = self.link.send(&[MAGIC, PING, 0, 0]);
                            next_ping = Instant::now() + Duration::from_secs(1);
                        }
                        self.channels[index].push(&buf[8..n])
                    } else {
                        let seq = u16::from_be_bytes([buf[6], buf[7]]);
                        match self.channels[index].push_seq(seq, &buf[8..n]) {
                            Ok(true) => {
                                let ack = [MAGIC, DRW_ACK, 0, 6, MAGIC_DRW, buf[5], 0, 1, buf[6], buf[7]];
                                let _ = self.link.send(&ack);
                                Ok(())
                            }
                            Ok(false) => Ok(()),
                            Err(e) => Err(e),
                        }
                    };
                    if let Err(e) = result {
                        return e;
                    }
                }
                PING => {
                    let _ = self.link.send(&[MAGIC, PONG, 0, 0]);
                }
                DRW_ACK => {
                    let _ = self.acks.try_send(());
                }
                CLOSE => return "the camera closed the connection".into(),
                _ => {}
            }
        }
    }

    /// One packet; None when the wait ran out (to look at `closed`).
    fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, String> {
        if let Some(s) = self.tcp_in.as_mut() {
            let mut head = [0u8; 8];
            s.read_exact(&mut head).map_err(|e| e.to_string())?;
            let n = u16::from_be_bytes([head[0], head[1]]) as usize;
            if n > buf.len() {
                return Err("a packet larger than expected".into());
            }
            s.read_exact(&mut buf[..n]).map_err(|e| e.to_string())?;
            return Ok(Some(n));
        }
        match &self.link {
            Link::Udp { sock, to } => {
                let _ = sock.set_read_timeout(Some(Duration::from_secs(1)));
                match sock.recv_from(buf) {
                    Ok((n, from)) if from.ip() == to.ip() => Ok(Some(n)),
                    Ok(_) => Ok(None),
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
                    Err(e) => Err(e.to_string()),
                }
            }
            Link::Tcp(_) => Err("no reading half".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(body: &[u8]) -> Vec<u8> {
        let mut m = (body.len() as u32).to_be_bytes().to_vec();
        m.extend_from_slice(body);
        m
    }

    #[test]
    fn messages_are_split_across_and_within_packets() {
        let (tx, rx) = sync_channel(10);
        let mut c = Channel::new(tx);
        let mut stream = message(b"first");
        stream.extend(message(b"second one"));
        c.push(&stream[..7]).unwrap();
        assert!(rx.try_recv().is_err());
        c.push(&stream[7..]).unwrap();
        assert_eq!(rx.try_recv().unwrap(), b"first");
        assert_eq!(rx.try_recv().unwrap(), b"second one");
    }

    #[test]
    fn packets_out_of_order_are_put_back_in_order() {
        let (tx, rx) = sync_channel(10);
        let mut c = Channel::new(tx);
        let stream = message(b"abcdef");
        // Packet 1 arrives before packet 0; a repeat of 0 is ignored.
        assert!(c.push_seq(1, &stream[5..]).unwrap());
        assert!(rx.try_recv().is_err());
        assert!(c.push_seq(0, &stream[..5]).unwrap());
        assert_eq!(rx.try_recv().unwrap(), b"abcdef");
        assert!(c.push_seq(0, &stream[..5]).unwrap());
        assert!(rx.try_recv().is_err());
        assert_eq!(c.wait_seq, 2);
    }

    #[test]
    fn numbering_wraps_around() {
        let (tx, rx) = sync_channel(10);
        let mut c = Channel::new(tx);
        c.wait_seq = u16::MAX;
        let stream = message(b"xy");
        assert!(c.push_seq(0, &stream[3..]).unwrap()); // early, after the wrap
        assert!(c.push_seq(u16::MAX, &stream[..3]).unwrap());
        assert_eq!(rx.try_recv().unwrap(), b"xy");
        assert_eq!(c.wait_seq, 1);
    }
}
