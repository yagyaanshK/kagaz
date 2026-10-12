//! The receiving side: listen for an incoming SIP call and raise the ring.
//! This is the heart of "someone is at the door" — it works both for calls
//! the server forwards to us and for peer-to-peer Direct IP calls on the
//! LAN (which need no account at all). Media (hearing, speaking, the door
//! camera, opening the door) builds on the call this hands back.

use super::sip::{self, Message};
use super::Error;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

/// What we learned from the call's SDP: where the caller wants media and
/// whether it offers video (the door camera).
#[derive(Debug, Clone, Default)]
pub struct Media {
    /// The address media should go to (the `c=` line, else the signalling peer).
    pub connection_ip: Option<String>,
    pub audio_port: Option<u16>,
    pub video_port: Option<u16>,
}

impl Media {
    pub fn has_video(&self) -> bool {
        self.video_port.is_some_and(|p| p != 0)
    }
}

/// An incoming call, before we answer it.
pub struct Incoming {
    sock: UdpSocket,
    invite: Message,
    remote: SocketAddr,
    to_tag: String,
    contact: String,
}

impl Incoming {
    /// Who is calling, as a display name (falls back to the SIP user).
    pub fn caller(&self) -> String {
        let from = self.invite.header("From").unwrap_or("");
        caller_display(from)
    }

    /// The caller's SIP URI.
    pub fn caller_uri(&self) -> String {
        let from = self.invite.header("From").unwrap_or("");
        uri_in(from).unwrap_or_default()
    }

    pub fn call_id(&self) -> &str {
        self.invite.header("Call-ID").unwrap_or("")
    }

    /// Where the call came from on the network.
    pub fn remote(&self) -> SocketAddr {
        self.remote
    }

    /// What media the caller offered.
    pub fn media(&self) -> Media {
        parse_sdp(&self.invite.body)
    }

    /// Tell the caller we are alerting the user (SIP 180 Ringing).
    pub fn ringing(&self) -> Result<(), Error> {
        self.reply(180, "Ringing", None)
    }

    /// Decline the call (486 Busy Here).
    pub fn decline(&self) -> Result<(), Error> {
        self.reply(486, "Busy Here", None)
    }

    /// Answer the call with our SDP (200 OK). Media handling comes next.
    pub fn answer(&self, sdp: &str) -> Result<(), Error> {
        self.reply_body(200, "OK", Some(sdp))
    }

    fn reply(&self, code: u16, reason: &str, sdp: Option<&str>) -> Result<(), Error> {
        self.reply_body(code, reason, sdp)
    }

    fn reply_body(&self, code: u16, reason: &str, sdp: Option<&str>) -> Result<(), Error> {
        let contact = format!("sip:kagaz@{}", self.contact);
        let msg = self
            .invite
            .response_to(code, reason, &self.to_tag, Some(&contact), sdp);
        self.sock
            .send_to(msg.as_bytes(), self.remote)
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(())
    }

    /// Wait until the caller hangs up (BYE/CANCEL), answering it so the
    /// device stops. Returns when the call is over.
    pub fn wait_end(&self) -> Result<(), Error> {
        let mut buf = vec![0u8; 65535];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    if let Ok(msg) = sip::parse(&String::from_utf8_lossy(&buf[..n])) {
                        match msg.method().as_deref() {
                            Some("BYE") | Some("CANCEL") => {
                                let _ = self.sock.send_to(
                                    msg.response_to(200, "OK", &self.to_tag, None, None)
                                        .as_bytes(),
                                    from,
                                );
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => return Err(Error::Transport(e.to_string())),
            }
        }
    }
}

/// A UDP SIP endpoint that waits for incoming calls. For Direct IP this
/// binds the device's expected port (5060); for a server-forwarded call it
/// is the port we registered a Contact on.
pub struct Listener {
    sock: UdpSocket,
    local: String,
}

impl Listener {
    pub fn bind(port: u16) -> Result<Listener, Error> {
        let sock =
            UdpSocket::bind(("0.0.0.0", port)).map_err(|e| Error::Transport(e.to_string()))?;
        sock.set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| Error::Transport(e.to_string()))?;
        let local = local_contact(&sock, port);
        Ok(Listener { sock, local })
    }

    /// Block until a call comes in. Replies 100 Trying straight away so the
    /// device stops retransmitting, and returns the call for the caller to
    /// ring/answer/decline. Non-INVITE messages are handled quietly.
    pub fn next_call(&self) -> Result<Incoming, Error> {
        let mut buf = vec![0u8; 65535];
        loop {
            let (n, from) = match self.sock.recv_from(&mut buf) {
                Ok(x) => x,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => return Err(Error::Transport(e.to_string())),
            };
            let Ok(msg) = sip::parse(&String::from_utf8_lossy(&buf[..n])) else {
                continue;
            };
            if msg.method().as_deref() != Some("INVITE") {
                continue;
            }
            let to_tag = sip::token();
            // 100 Trying stops the INVITE retransmissions.
            let _ = self.sock.send_to(
                msg.response_to(100, "Trying", "", None, None).as_bytes(),
                from,
            );
            return Ok(Incoming {
                sock: self
                    .sock
                    .try_clone()
                    .map_err(|e| Error::Transport(e.to_string()))?,
                invite: msg,
                remote: from,
                to_tag,
                contact: self.local.clone(),
            });
        }
    }
}

/// The `ip:port` to advertise in a Contact, best effort.
fn local_contact(sock: &UdpSocket, port: u16) -> String {
    if let Ok(addr) = sock.local_addr() {
        if !addr.ip().is_unspecified() {
            return addr.to_string();
        }
    }
    // Bound to 0.0.0.0: find the LAN address another way.
    match local_ip() {
        Some(ip) => format!("{ip}:{port}"),
        None => format!("0.0.0.0:{port}"),
    }
}

fn local_ip() -> Option<String> {
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("8.8.8.8:80").ok()?;
    Some(probe.local_addr().ok()?.ip().to_string())
}

/// The display name from a From header, e.g. `"Door" <sip:20011001@...>`.
fn caller_display(from: &str) -> String {
    if let Some(start) = from.find('"') {
        if let Some(end) = from[start + 1..].find('"') {
            let name = &from[start + 1..start + 1 + end];
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    // No quoted name: use the user part of the URI.
    uri_in(from)
        .and_then(|u| {
            u.trim_start_matches("sip:")
                .split('@')
                .next()
                .map(str::to_string)
        })
        .unwrap_or_else(|| "someone at the door".to_string())
}

/// The `sip:...` URI inside a From/To/Contact value.
fn uri_in(value: &str) -> Option<String> {
    if let Some(start) = value.find('<') {
        let end = value[start..].find('>')?;
        Some(value[start + 1..start + end].to_string())
    } else {
        value.split(';').next().map(|s| s.trim().to_string())
    }
}

/// Pull the media directions out of an SDP offer.
fn parse_sdp(sdp: &str) -> Media {
    let mut media = Media::default();
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("c=IN IP4 ") {
            media.connection_ip = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("m=audio ") {
            media.audio_port = rest.split_whitespace().next().and_then(|p| p.parse().ok());
        } else if let Some(rest) = line.strip_prefix("m=video ") {
            media.video_port = rest.split_whitespace().next().and_then(|p| p.parse().ok());
        }
    }
    media
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "INVITE sip:kagaz@192.0.2.6:5060 SIP/2.0\r\n\
Via: SIP/2.0/UDP 192.0.2.3:5060;branch=z9hG4bKabc\r\n\
From: \"Door\" <sip:20011001@192.0.2.3>;tag=111\r\n\
To: <sip:kagaz@192.0.2.6>\r\n\
Call-ID: call-9\r\n\
CSeq: 1 INVITE\r\n\
Content-Type: application/sdp\r\n\
Content-Length: 0\r\n\r\n\
v=0\r\n\
c=IN IP4 192.0.2.3\r\n\
m=audio 20000 RTP/AVP 0 8\r\n\
m=video 20002 RTP/AVP 104\r\n";

    #[test]
    fn reads_the_caller_and_media_from_an_invite() {
        let msg = sip::parse(INVITE).unwrap();
        assert_eq!(caller_display(msg.header("From").unwrap()), "Door");
        assert_eq!(
            uri_in(msg.header("From").unwrap()).as_deref(),
            Some("sip:20011001@192.0.2.3")
        );
        let media = parse_sdp(&sdp_body());
        assert_eq!(media.connection_ip.as_deref(), Some("192.0.2.3"));
        assert_eq!(media.audio_port, Some(20000));
        assert_eq!(media.video_port, Some(20002));
        assert!(media.has_video());
    }

    fn sdp_body() -> String {
        INVITE.split("\r\n\r\n").nth(1).unwrap().to_string()
    }

    #[test]
    fn falls_back_to_the_user_part_without_a_quoted_name() {
        assert_eq!(caller_display("<sip:1001@192.168.1.5>;tag=x"), "1001");
    }

    #[test]
    fn a_ringing_response_echoes_the_dialog_and_tags_to() {
        let msg = sip::parse(INVITE).unwrap();
        let resp = msg.response_to(
            180,
            "Ringing",
            "mytag",
            Some("sip:kagaz@192.0.2.6:5060"),
            None,
        );
        assert!(resp.starts_with("SIP/2.0 180 Ringing\r\n"));
        assert!(resp.contains("Via: SIP/2.0/UDP 192.0.2.3:5060;branch=z9hG4bKabc"));
        assert!(resp.contains("To: <sip:kagaz@192.0.2.6>;tag=mytag"));
        assert!(resp.contains("Call-ID: call-9"));
        assert!(resp.contains("CSeq: 1 INVITE"));
        assert!(resp.contains("Contact: <sip:kagaz@192.0.2.6:5060>"));
    }
}
