//! The media half of a call (SDP, RFC 4566 / 3264): what the other side
//! offers or answers, and what Kagaz offers or answers back. Kagaz sends
//! and receives G.711 voice with telephone-event tones, and only receives
//! H.264 video: it has no camera to show.

use super::g711::Law;
use std::net::{IpAddr, SocketAddr};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stream {
    pub ip: Option<IpAddr>,
    pub port: u16,
    /// Payload types in preference order.
    pub payloads: Vec<u8>,
    /// payload type -> encoding name, e.g. 0 -> "PCMU/8000".
    pub rtpmap: Vec<(u8, String)>,
    /// payload type -> format parameters.
    pub fmtp: Vec<(u8, String)>,
    /// sendrecv, sendonly, recvonly or inactive.
    pub direction: String,
}

impl Stream {
    fn encoding(&self, pt: u8) -> Option<&str> {
        self.rtpmap
            .iter()
            .find(|(p, _)| *p == pt)
            .map(|(_, e)| e.as_str())
    }

    fn fmtp_of(&self, pt: u8) -> Option<&str> {
        self.fmtp
            .iter()
            .find(|(p, _)| *p == pt)
            .map(|(_, f)| f.as_str())
    }

    /// The payload type of an encoding by name (case-insensitive), using
    /// the static table for 0 and 8 when there is no rtpmap.
    fn find(&self, name: &str) -> Option<u8> {
        self.payloads
            .iter()
            .copied()
            .find(|&pt| match self.encoding(pt) {
                Some(e) => e
                    .split('/')
                    .next()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name)),
                None => matches!((pt, name), (0, "PCMU") | (8, "PCMA")),
            })
    }

    pub fn addr(&self) -> Option<SocketAddr> {
        (self.port != 0).then_some(SocketAddr::new(self.ip?, self.port))
    }

    /// Whether the other side will send on this stream.
    fn they_send(&self) -> bool {
        matches!(self.direction.as_str(), "sendrecv" | "sendonly" | "")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Description {
    pub audio: Option<Stream>,
    pub video: Option<Stream>,
}

pub fn parse(sdp: &str) -> Description {
    let mut session_ip = None;
    let mut out = Description::default();
    let mut current: Option<(String, Stream)> = None;
    let finish = |out: &mut Description, cur: Option<(String, Stream)>, ip: Option<IpAddr>| {
        if let Some((kind, mut s)) = cur {
            if s.ip.is_none() {
                s.ip = ip;
            }
            if s.direction.is_empty() {
                s.direction = "sendrecv".into();
            }
            match kind.as_str() {
                "audio" if out.audio.is_none() => out.audio = Some(s),
                "video" if out.video.is_none() => out.video = Some(s),
                _ => {}
            }
        }
    };
    for line in sdp.lines().map(str::trim) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "m" => {
                finish(&mut out, current.take(), session_ip);
                let mut parts = value.split_whitespace();
                let kind = parts.next().unwrap_or("").to_string();
                let port = parts.next().and_then(|p| p.split('/').next()?.parse().ok());
                let _proto = parts.next();
                let payloads = parts.filter_map(|p| p.parse().ok()).collect();
                current = Some((
                    kind,
                    Stream {
                        port: port.unwrap_or(0),
                        payloads,
                        ..Default::default()
                    },
                ));
            }
            "c" => {
                let ip = value
                    .split_whitespace()
                    .nth(2)
                    .and_then(|a| a.split('/').next()?.parse().ok());
                match current.as_mut() {
                    Some((_, s)) => s.ip = ip,
                    None => session_ip = ip,
                }
            }
            "a" => {
                let Some((_, s)) = current.as_mut() else {
                    continue;
                };
                if let Some(rest) = value.strip_prefix("rtpmap:") {
                    if let Some((pt, enc)) = rest.split_once(' ') {
                        if let Ok(pt) = pt.parse() {
                            s.rtpmap.push((pt, enc.trim().to_string()));
                        }
                    }
                } else if let Some(rest) = value.strip_prefix("fmtp:") {
                    if let Some((pt, f)) = rest.split_once(' ') {
                        if let Ok(pt) = pt.parse() {
                            s.fmtp.push((pt, f.trim().to_string()));
                        }
                    }
                } else if matches!(value, "sendrecv" | "sendonly" | "recvonly" | "inactive") {
                    s.direction = value.to_string();
                }
            }
            _ => {}
        }
    }
    finish(&mut out, current, session_ip);
    out
}

/// What a call ended up using.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Agreed {
    /// Where to send voice; None when there is no voice.
    pub audio: Option<SocketAddr>,
    pub law: Option<Law>,
    pub dtmf_pt: Option<u8>,
    /// Where the video comes from (and where to send keep-alives).
    pub video: Option<SocketAddr>,
    pub video_pt: Option<u8>,
    /// `sprop-parameter-sets` of the H.264 stream, when given.
    pub sprop: Option<String>,
}

fn sprop(fmtp: Option<&str>) -> Option<String> {
    fmtp?
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("sprop-parameter-sets="))
        .map(str::to_string)
}

fn head(local_ip: &str) -> String {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "v=0\r\no=kagaz {id} {id} IN IP4 {local_ip}\r\ns=Kagaz\r\nc=IN IP4 {local_ip}\r\nt=0 0\r\n"
    )
}

/// Kagaz's offer for a call it places.
pub fn offer(local_ip: &str, audio_port: u16, video_port: u16) -> String {
    let mut s = head(local_ip);
    s.push_str(&format!("m=audio {audio_port} RTP/AVP 0 8 101\r\n"));
    s.push_str("a=rtpmap:0 PCMU/8000\r\na=rtpmap:8 PCMA/8000\r\n");
    s.push_str("a=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-15\r\n");
    s.push_str("a=ptime:20\r\na=sendrecv\r\n");
    s.push_str(&format!("m=video {video_port} RTP/AVP 96\r\n"));
    s.push_str("a=rtpmap:96 H264/90000\r\n");
    s.push_str("a=fmtp:96 profile-level-id=42e01f;packetization-mode=1\r\n");
    s.push_str("a=recvonly\r\n");
    s
}

/// What the other side's answer to our [`offer`] agreed.
pub fn agreed_from_answer(answer: &Description) -> Agreed {
    let mut a = Agreed::default();
    if let Some(s) = &answer.audio {
        a.law = s
            .find("PCMU")
            .or_else(|| s.find("PCMA"))
            .and_then(Law::from_payload_type);
        if a.law.is_some() {
            a.audio = s.addr();
        }
        a.dtmf_pt = s.find("telephone-event");
    }
    if let Some(s) = &answer.video {
        a.video_pt = s.find("H264");
        if a.video_pt.is_some() && s.they_send() {
            a.video = s.addr();
            a.sprop = sprop(a.video_pt.and_then(|pt| s.fmtp_of(pt)));
        }
    }
    a
}

/// Answer an offer: pick G.711 (and tones), and take H.264 video when it
/// is offered for us to receive. Returns the answer SDP and the agreement.
pub fn answer(
    offer: &Description,
    local_ip: &str,
    audio_port: u16,
    video_port: u16,
) -> (String, Agreed) {
    let mut agreed = Agreed::default();
    let mut s = head(local_ip);
    if let Some(a) = &offer.audio {
        let law_pt = a.find("PCMU").or_else(|| a.find("PCMA"));
        agreed.law = law_pt.and_then(Law::from_payload_type);
        agreed.dtmf_pt = a.find("telephone-event");
        match law_pt {
            Some(pt) => {
                agreed.audio = a.addr();
                let mut pts = vec![pt];
                pts.extend(agreed.dtmf_pt);
                let list: Vec<String> = pts.iter().map(u8::to_string).collect();
                s.push_str(&format!(
                    "m=audio {audio_port} RTP/AVP {}\r\n",
                    list.join(" ")
                ));
                s.push_str(&format!(
                    "a=rtpmap:{pt} {}/8000\r\n",
                    if pt == 8 { "PCMA" } else { "PCMU" }
                ));
                if let Some(d) = agreed.dtmf_pt {
                    s.push_str(&format!(
                        "a=rtpmap:{d} telephone-event/8000\r\na=fmtp:{d} 0-15\r\n"
                    ));
                }
                s.push_str("a=ptime:20\r\na=sendrecv\r\n");
            }
            None => {
                let pts: Vec<String> = a.payloads.iter().map(u8::to_string).collect();
                s.push_str(&format!("m=audio 0 RTP/AVP {}\r\n", pts.join(" ")));
            }
        }
    }
    if let Some(v) = &offer.video {
        let pt = v.find("H264").filter(|_| v.they_send());
        match pt {
            Some(pt) => {
                agreed.video_pt = Some(pt);
                agreed.video = v.addr();
                agreed.sprop = sprop(v.fmtp_of(pt));
                s.push_str(&format!("m=video {video_port} RTP/AVP {pt}\r\n"));
                s.push_str(&format!("a=rtpmap:{pt} H264/90000\r\n"));
                if let Some(f) = v.fmtp_of(pt) {
                    s.push_str(&format!("a=fmtp:{pt} {f}\r\n"));
                }
                s.push_str("a=recvonly\r\n");
            }
            None => {
                let pts: Vec<String> = v.payloads.iter().map(u8::to_string).collect();
                s.push_str(&format!("m=video 0 RTP/AVP {}\r\n", pts.join(" ")));
            }
        }
    }
    (s, agreed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFER: &str = "v=0\r\no=door 1 1 IN IP4 192.0.2.3\r\ns=Talk\r\nc=IN IP4 203.0.113.10\r\nt=0 0\r\n\
m=audio 12000 RTP/AVP 8 0 101\r\na=rtpmap:8 PCMA/8000\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=sendrecv\r\n\
m=video 12002 RTP/AVP 104\r\na=rtpmap:104 H264/90000\r\na=fmtp:104 profile-level-id=42e01f;packetization-mode=1;sprop-parameter-sets=Z0LgH6o=,aM48gA==\r\na=sendonly\r\n";

    #[test]
    fn reads_an_offer() {
        let d = parse(OFFER);
        let a = d.audio.unwrap();
        assert_eq!(a.addr().unwrap().to_string(), "203.0.113.10:12000");
        assert_eq!(a.payloads, vec![8, 0, 101]);
        assert_eq!(a.direction, "sendrecv");
        let v = d.video.unwrap();
        assert_eq!(v.port, 12002);
        assert_eq!(v.direction, "sendonly");
    }

    #[test]
    fn answers_with_mu_law_tones_and_receive_only_video() {
        let (sdp, agreed) = answer(&parse(OFFER), "192.0.2.6", 40000, 40002);
        assert_eq!(agreed.law, Some(Law::Mu));
        assert_eq!(agreed.dtmf_pt, Some(101));
        assert_eq!(agreed.audio.unwrap().to_string(), "203.0.113.10:12000");
        assert_eq!(agreed.video_pt, Some(104));
        assert_eq!(agreed.sprop.as_deref(), Some("Z0LgH6o=,aM48gA=="));
        assert!(sdp.contains("c=IN IP4 192.0.2.6"));
        assert!(sdp.contains("m=audio 40000 RTP/AVP 0 101"));
        assert!(sdp.contains("m=video 40002 RTP/AVP 104"));
        assert!(sdp.contains("a=recvonly"));
        // The answer parses back into what was agreed.
        let back = parse(&sdp);
        assert_eq!(back.audio.unwrap().payloads, vec![0, 101]);
    }

    #[test]
    fn refuses_video_it_cannot_receive_and_audio_it_cannot_speak() {
        let offer = "v=0\r\nc=IN IP4 203.0.113.10\r\nm=audio 5000 RTP/AVP 18\r\na=rtpmap:18 G729/8000\r\nm=video 5002 RTP/AVP 96\r\na=rtpmap:96 VP8/90000\r\n";
        let (sdp, agreed) = answer(&parse(offer), "192.0.2.6", 40000, 40002);
        assert!(agreed.audio.is_none() && agreed.video.is_none());
        assert!(sdp.contains("m=audio 0 RTP/AVP 18"));
        assert!(sdp.contains("m=video 0 RTP/AVP 96"));
    }

    #[test]
    fn reads_an_answer_to_our_offer() {
        let ans = "v=0\r\nc=IN IP4 203.0.113.10\r\nm=audio 7000 RTP/AVP 8 101\r\na=rtpmap:8 PCMA/8000\r\na=rtpmap:101 telephone-event/8000\r\nm=video 7002 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=sendonly\r\n";
        let a = agreed_from_answer(&parse(ans));
        assert_eq!(a.law, Some(Law::A));
        assert_eq!(a.dtmf_pt, Some(101));
        assert_eq!(a.video.unwrap().port(), 7002);
        assert!(offer("192.0.2.6", 1, 2).contains("a=recvonly"));
    }
}
