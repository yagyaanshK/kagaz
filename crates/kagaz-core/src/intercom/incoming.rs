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

    /// Answer the call: allocate our RTP ports, send 200 OK with our SDP,
    /// and hand back the live call (to open the door, speak, hang up). The
    /// RTP sockets are held open; audio/video processing plugs in later.
    pub fn answer(&self) -> Result<ActiveCall, Error> {
        let local_ip = self
            .contact
            .rsplit_once(':')
            .map(|(ip, _)| ip.to_string())
            .unwrap_or_else(|| self.contact.clone());
        let audio = UdpSocket::bind("0.0.0.0:0").map_err(|e| Error::Transport(e.to_string()))?;
        let audio_port = audio
            .local_addr()
            .map_err(|e| Error::Transport(e.to_string()))?
            .port();
        let media = self.media();
        let with_video = media.has_video();
        let video = if with_video {
            Some(UdpSocket::bind("0.0.0.0:0").map_err(|e| Error::Transport(e.to_string()))?)
        } else {
            None
        };
        let video_port = video
            .as_ref()
            .and_then(|v| v.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(0);
        let sdp = super::media::answer_sdp(&local_ip, audio_port, video_port, with_video);
        self.reply_body(200, "OK", Some(&sdp))?;
        let peer = self.invite.header("From").unwrap_or("").to_string();
        let our_identity = self.invite.header("To").unwrap_or("").to_string();
        let remote_target = self
            .invite
            .header("Contact")
            .and_then(uri_in)
            .or_else(|| uri_in(&peer))
            .unwrap_or_default();
        let remote_media = media.connection_ip.clone().zip(media.audio_port);
        Ok(ActiveCall {
            sock: self
                .sock
                .try_clone()
                .map_err(|e| Error::Transport(e.to_string()))?,
            remote_sig: self.remote,
            remote_target,
            our_identity,
            our_tag: self.to_tag.clone(),
            peer,
            call_id: self.invite.header("Call-ID").unwrap_or("").to_string(),
            contact: self.contact.clone(),
            cseq: 1,
            _audio: audio,
            _video: video,
            remote_media,
        })
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
        while !self.poll_end(Duration::from_millis(500))? {}
        Ok(())
    }

    /// Read the signalling for up to `wait`: true once the caller has given
    /// up (CANCEL) or hung up (BYE). Anyone else calling meanwhile hears busy.
    pub fn poll_end(&self, wait: Duration) -> Result<bool, Error> {
        drain(
            &self.sock,
            &self.to_tag,
            self.call_id(),
            Some(&self.invite),
            wait,
        )
    }
}

/// Service the socket for up to `wait`. Answers a BYE or CANCEL for our
/// call (and the INVITE itself with 487 when cancelled) and says whether the
/// call is over; a new INVITE for another call gets 486 Busy Here.
fn drain(
    sock: &UdpSocket,
    to_tag: &str,
    call_id: &str,
    invite: Option<&Message>,
    wait: Duration,
) -> Result<bool, Error> {
    let started = std::time::Instant::now();
    let mut buf = vec![0u8; 65535];
    loop {
        let left = wait.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Ok(false);
        }
        sock.set_read_timeout(Some(left))
            .map_err(|e| Error::Transport(e.to_string()))?;
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(x) => x,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Ok(false);
            }
            Err(e) => return Err(Error::Transport(e.to_string())),
        };
        let Ok(msg) = sip::parse(&String::from_utf8_lossy(&buf[..n])) else {
            continue;
        };
        let ours = msg.header("Call-ID") == Some(call_id);
        match msg.method().as_deref() {
            Some("BYE") if ours => {
                let _ = sock.send_to(
                    msg.response_to(200, "OK", to_tag, None, None).as_bytes(),
                    from,
                );
                return Ok(true);
            }
            Some("CANCEL") if ours => {
                let _ = sock.send_to(msg.response_to(200, "OK", "", None, None).as_bytes(), from);
                if let Some(invite) = invite {
                    let _ = sock.send_to(
                        invite
                            .response_to(487, "Request Terminated", to_tag, None, None)
                            .as_bytes(),
                        from,
                    );
                }
                return Ok(true);
            }
            Some("INVITE") if !ours => {
                let _ = sock.send_to(
                    msg.response_to(486, "Busy Here", &sip::token(), None, None)
                        .as_bytes(),
                    from,
                );
            }
            Some("OPTIONS") => {
                let _ = sock.send_to(msg.response_to(200, "OK", "", None, None).as_bytes(), from);
            }
            _ => {}
        }
    }
}

/// A call we have answered: the dialog is up, so we can open the door,
/// (later) carry voice and video, and hang up. The RTP sockets live here.
pub struct ActiveCall {
    sock: UdpSocket,
    remote_sig: SocketAddr,
    /// Their Contact URI — where in-dialog requests are addressed.
    remote_target: String,
    /// Our side of the dialog (the INVITE's To) plus our tag.
    our_identity: String,
    our_tag: String,
    /// Their side of the dialog (the INVITE's From, already tagged).
    peer: String,
    call_id: String,
    contact: String,
    cseq: u32,
    _audio: UdpSocket,
    _video: Option<UdpSocket>,
    /// Where the caller receives audio RTP (for DTMF tones / voice later).
    remote_media: Option<(String, u16)>,
}

impl ActiveCall {
    /// Open the door: ask the station to press `#`, which it maps to its
    /// relay (SIP INFO, `application/dtmf-relay`).
    pub fn open_door(&mut self) -> Result<(), Error> {
        self.press('#')
    }

    /// Send one DTMF digit in-dialog.
    pub fn press(&mut self, digit: char) -> Result<(), Error> {
        self.cseq += 1;
        let body = super::media::dtmf_info_body(digit);
        let req = self.in_dialog_request("INFO", Some("application/dtmf-relay"), &body);
        self.sock
            .send_to(req.as_bytes(), self.remote_sig)
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(())
    }

    /// End the call (BYE).
    pub fn hangup(&mut self) -> Result<(), Error> {
        self.cseq += 1;
        let req = self.in_dialog_request("BYE", None, "");
        self.sock
            .send_to(req.as_bytes(), self.remote_sig)
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(())
    }

    /// Where the caller wants its audio RTP, if the SDP said so.
    pub fn remote_audio(&self) -> Option<&(String, u16)> {
        self.remote_media.as_ref()
    }

    /// Read the signalling for up to `wait`: true once the other side has
    /// hung up (BYE, answered with 200 OK).
    pub fn poll_end(&self, wait: Duration) -> Result<bool, Error> {
        drain(&self.sock, &self.our_tag, &self.call_id, None, wait)
    }

    fn in_dialog_request(&self, method: &str, content_type: Option<&str>, body: &str) -> String {
        let branch = format!("z9hG4bK{}", sip::token());
        let mut lines = vec![
            format!("{method} {} SIP/2.0", self.remote_target),
            format!("Via: SIP/2.0/UDP {};branch={branch};rport", self.contact),
            "Max-Forwards: 70".to_string(),
            format!("From: {};tag={}", self.our_identity, self.our_tag),
            format!("To: {}", self.peer),
            format!("Call-ID: {}", self.call_id),
            format!("CSeq: {} {method}", self.cseq),
            format!("Contact: <sip:kagaz@{}>", self.contact),
            "User-Agent: Kagaz".to_string(),
        ];
        if let Some(ct) = content_type {
            lines.push(format!("Content-Type: {ct}"));
        }
        lines.push(format!("Content-Length: {}", body.len()));
        format!("{}\r\n\r\n{body}", lines.join("\r\n"))
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

    /// The `ip:port` we answer on, as advertised to callers.
    pub fn local(&self) -> &str {
        &self.local
    }

    /// Block until a call comes in. Replies 100 Trying straight away so the
    /// device stops retransmitting, and returns the call for the caller to
    /// ring/answer/decline. Non-INVITE messages are handled quietly.
    pub fn next_call(&self) -> Result<Incoming, Error> {
        loop {
            if let Some(call) = self.next_call_within(Duration::from_millis(500))? {
                return Ok(call);
            }
        }
    }

    /// Like `next_call`, but gives up after `wait` with `None`, so a caller
    /// can check for other work (or a stop request) between calls.
    pub fn next_call_within(&self, wait: Duration) -> Result<Option<Incoming>, Error> {
        let started = std::time::Instant::now();
        let mut buf = vec![0u8; 65535];
        loop {
            let left = wait.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Ok(None);
            }
            self.sock
                .set_read_timeout(Some(left))
                .map_err(|e| Error::Transport(e.to_string()))?;
            let (n, from) = match self.sock.recv_from(&mut buf) {
                Ok(x) => x,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(Error::Transport(e.to_string())),
            };
            let Ok(msg) = sip::parse(&String::from_utf8_lossy(&buf[..n])) else {
                continue;
            };
            if msg.method().as_deref() == Some("OPTIONS") {
                let _ = self
                    .sock
                    .send_to(msg.response_to(200, "OK", "", None, None).as_bytes(), from);
                continue;
            }
            if msg.method().as_deref() != Some("INVITE") {
                continue;
            }
            let to_tag = sip::token();
            // 100 Trying stops the INVITE retransmissions.
            let _ = self.sock.send_to(
                msg.response_to(100, "Trying", "", None, None).as_bytes(),
                from,
            );
            return Ok(Some(Incoming {
                sock: self
                    .sock
                    .try_clone()
                    .map_err(|e| Error::Transport(e.to_string()))?,
                invite: msg,
                remote: from,
                to_tag,
                contact: self.local.clone(),
            }));
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

    #[test]
    fn open_door_sends_a_dtmf_info_in_dialog() {
        let station = UdpSocket::bind("127.0.0.1:0").unwrap();
        station
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut call = ActiveCall {
            sock: UdpSocket::bind("127.0.0.1:0").unwrap(),
            remote_sig: station.local_addr().unwrap(),
            remote_target: "sip:20011001@127.0.0.1".into(),
            our_identity: "<sip:kagaz@127.0.0.1:15060>".into(),
            our_tag: "ourtag".into(),
            peer: "\"Door\" <sip:20011001@127.0.0.1>;tag=111".into(),
            call_id: "cid".into(),
            contact: "127.0.0.1:15060".into(),
            cseq: 1,
            _audio: UdpSocket::bind("127.0.0.1:0").unwrap(),
            _video: None,
            remote_media: None,
        };
        call.open_door().unwrap();
        let mut buf = [0u8; 2048];
        let (n, _) = station.recv_from(&mut buf).unwrap();
        let req = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(req.starts_with("INFO sip:20011001@127.0.0.1 SIP/2.0\r\n"));
        assert!(req.contains("From: <sip:kagaz@127.0.0.1:15060>;tag=ourtag"));
        assert!(req.contains("To: \"Door\" <sip:20011001@127.0.0.1>;tag=111"));
        assert!(req.contains("CSeq: 2 INFO"));
        assert!(req.contains("Content-Type: application/dtmf-relay"));
        assert!(req.contains("Signal=#"));
    }
    #[test]
    fn a_cancel_ends_the_ring_and_a_stranger_hears_busy() {
        let listener = Listener::bind(0).unwrap();
        let our_port = listener.sock.local_addr().unwrap().port();
        let station = UdpSocket::bind("127.0.0.1:0").unwrap();
        station
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        station
            .send_to(INVITE.as_bytes(), ("127.0.0.1", our_port))
            .unwrap();
        let call = listener
            .next_call_within(Duration::from_secs(1))
            .unwrap()
            .expect("the INVITE arrives");
        assert_eq!(call.caller(), "Door");
        assert!(call.media().has_video());
        // Nothing yet: polling just times out.
        assert!(!call.poll_end(Duration::from_millis(50)).unwrap());
        // Another INVITE while this one rings is told busy.
        let other = INVITE.replace("Call-ID: call-9", "Call-ID: call-10");
        station
            .send_to(other.as_bytes(), ("127.0.0.1", our_port))
            .unwrap();
        assert!(!call.poll_end(Duration::from_millis(200)).unwrap());
        let mut buf = [0u8; 4096];
        let mut seen = Vec::new();
        while let Ok((n, _)) = station.recv_from(&mut buf) {
            seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
            if seen.len() == 2 {
                break;
            }
        }
        assert!(seen.iter().any(|m| m.starts_with("SIP/2.0 100 Trying")));
        assert!(seen
            .iter()
            .any(|m| m.starts_with("SIP/2.0 486 Busy Here") && m.contains("Call-ID: call-10")));
        // The caller gives up: CANCEL is answered and the INVITE gets 487.
        let cancel = INVITE
            .replace(
                "INVITE sip:kagaz@192.0.2.6:5060 SIP/2.0",
                "CANCEL sip:kagaz@192.0.2.6:5060 SIP/2.0",
            )
            .replace("CSeq: 1 INVITE", "CSeq: 1 CANCEL");
        station
            .send_to(cancel.as_bytes(), ("127.0.0.1", our_port))
            .unwrap();
        assert!(call.poll_end(Duration::from_secs(1)).unwrap());
        let mut seen = Vec::new();
        while let Ok((n, _)) = station.recv_from(&mut buf) {
            seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
            if seen.len() == 2 {
                break;
            }
        }
        assert!(seen
            .iter()
            .any(|m| m.starts_with("SIP/2.0 200 OK") && m.contains("CSeq: 1 CANCEL")));
        assert!(seen.iter().any(
            |m| m.starts_with("SIP/2.0 487 Request Terminated") && m.contains("CSeq: 1 INVITE")
        ));
        // With nobody calling, the listener comes back empty within the wait.
        assert!(listener
            .next_call_within(Duration::from_millis(50))
            .unwrap()
            .is_none());
    }
}
