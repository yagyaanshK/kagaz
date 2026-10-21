//! Atomberg fans over Wi-Fi: each fan on the network broadcasts who it is
//! and how it is on UDP 5625 every few seconds, and takes a JSON command on
//! UDP 5600, answering with its state string.

use super::atomberg::{self, Datagram, UDP_BROADCAST_PORT, UDP_COMMAND_PORT};
use super::FanError;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// A broadcast heard: which fan, from where, and its state when it said.
#[derive(Debug, Clone)]
pub struct Heard {
    pub ip: IpAddr,
    pub datagram: Datagram,
}

/// Listen for fans' broadcasts for `seconds`. Each fan once, its latest.
pub fn listen(seconds: f32) -> Result<Vec<Heard>, FanError> {
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)?;
    let _ = socket.set_reuse_address(true);
    #[cfg(unix)]
    let _ = socket.set_reuse_port(true);
    socket.bind(&SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), UDP_BROADCAST_PORT).into())?;
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(Duration::from_millis(300)))?;
    let end = Instant::now() + Duration::from_secs_f32(seconds.clamp(0.5, 60.0));
    let mut heard: Vec<Heard> = Vec::new();
    let mut buf = [0u8; 2048];
    while Instant::now() < end {
        let (n, from) = match socket.recv_from(&mut buf) {
            Ok(x) => x,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(e) => return Err(e.into()),
        };
        let Some(d) = atomberg::parse_datagram(&buf[..n]) else {
            continue;
        };
        if d.id.is_empty() {
            continue;
        }
        match heard.iter_mut().find(|h| h.datagram.id == d.id) {
            Some(h) => {
                h.ip = from.ip();
                if d.state.is_some() {
                    h.datagram = d;
                } else if h.datagram.series.is_none() {
                    h.datagram.series = d.series;
                }
            }
            None => heard.push(Heard {
                ip: from.ip(),
                datagram: d,
            }),
        }
    }
    Ok(heard)
}

/// Send one JSON command to the fan at `ip` and wait up to `timeout` for its
/// state string. None when it sent nothing back in time (the command may
/// still have been taken).
pub fn send(ip: IpAddr, json: &str, timeout: Duration) -> Result<Option<String>, FanError> {
    let socket = UdpSocket::bind(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0))?;
    socket.set_read_timeout(Some(timeout))?;
    socket.send_to(json.as_bytes(), SocketAddr::new(ip, UDP_COMMAND_PORT))?;
    let mut buf = [0u8; 2048];
    match socket.recv_from(&mut buf) {
        Ok((n, _)) => {
            let text = String::from_utf8_lossy(&buf[..n]).trim().to_string();
            // The reply is a bare state string, or JSON carrying one.
            if atomberg::FanState::decode(&text).is_some() {
                return Ok(Some(text));
            }
            if let Some(Datagram {
                state: Some(state), ..
            }) = atomberg::parse_datagram(text.as_bytes())
            {
                return Ok(Some(format!("{}", state.raw)));
            }
            Err(FanError::Reply(text))
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}
