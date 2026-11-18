//! RTSP as Wi-Fi Display uses it: requests and replies in both directions on
//! one connection, and the `text/parameters` bodies the source and the TV
//! exchange (`name: value` lines).

use std::fmt::Write as _;

/// One RTSP message, a request or a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// `OPTIONS * RTSP/1.0` or `RTSP/1.0 200 OK`.
    pub first: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Message {
    pub fn request(method: &str, url: &str, cseq: u32) -> Message {
        Message {
            first: format!("{method} {url} RTSP/1.0"),
            headers: vec![("CSeq".into(), cseq.to_string())],
            body: String::new(),
        }
    }

    pub fn reply(cseq: &str) -> Message {
        Message {
            first: "RTSP/1.0 200 OK".into(),
            headers: vec![("CSeq".into(), cseq.into())],
            body: String::new(),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Message {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn with_parameters(mut self, body: String) -> Message {
        self.headers.push(("Content-Type".into(), "text/parameters".into()));
        self.body = body;
        self
    }

    pub fn is_reply(&self) -> bool {
        self.first.starts_with("RTSP/")
    }

    /// The method of a request (`OPTIONS`, `SETUP` ...), empty for a reply.
    pub fn method(&self) -> &str {
        if self.is_reply() {
            ""
        } else {
            self.first.split(' ').next().unwrap_or("")
        }
    }

    /// The status code of a reply, 0 for a request.
    pub fn status(&self) -> u16 {
        if !self.is_reply() {
            return 0;
        }
        self.first.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn cseq(&self) -> &str {
        self.get("CSeq").unwrap_or("0")
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut s = String::new();
        let _ = write!(s, "{}\r\n", self.first);
        for (n, v) in &self.headers {
            let _ = write!(s, "{n}: {v}\r\n");
        }
        if !self.body.is_empty() {
            let _ = write!(s, "Content-Length: {}\r\n", self.body.len());
        }
        s.push_str("\r\n");
        s.push_str(&self.body);
        s.into_bytes()
    }
}

/// Takes one whole message off the front of `buf`, if one has arrived.
pub fn take_message(buf: &mut Vec<u8>) -> Option<Message> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let first = lines.next()?.trim().to_string();
    let mut headers = Vec::new();
    let mut length = 0usize;
    for line in lines {
        if let Some((n, v)) = line.split_once(':') {
            let (n, v) = (n.trim().to_string(), v.trim().to_string());
            if n.eq_ignore_ascii_case("Content-Length") {
                length = v.parse().unwrap_or(0);
            } else {
                headers.push((n, v));
            }
        }
    }
    let start = end + 4;
    if buf.len() < start + length {
        return None;
    }
    let body = String::from_utf8_lossy(&buf[start..start + length]).into_owned();
    buf.drain(..start + length);
    Some(Message { first, headers, body })
}

/// The `name: value` lines of a parameters body, in order.
pub fn parameters(body: &str) -> Vec<(String, String)> {
    body.lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// The parameter names of a body that only names them (a GET_PARAMETER).
pub fn parameter_names(body: &str) -> Vec<String> {
    body.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// What a TV says it can take, from its reply to the source's GET_PARAMETER.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SinkAbilities {
    /// H.264 profiles offered: 1 = Constrained Baseline, 2 = Constrained High.
    pub profiles: Vec<u8>,
    /// The CEA resolution bitmap of the first H.264 entry.
    pub cea: u32,
    /// The highest level bit of the first entry (0x01 = 3.1 ... 0x10 = 4.2).
    pub levels: u8,
    /// The UDP port the TV takes the RTP stream on.
    pub rtp_port: u16,
    /// Audio codecs offered, as named (`LPCM`, `AAC`, `AC3`).
    pub audio: Vec<String>,
    /// The TV offers HDCP (it does not say whether it demands it).
    pub hdcp: bool,
    /// The TV can send a mouse and keyboard back (UIBC).
    pub uibc: bool,
}

/// Reads the TV's GET_PARAMETER reply.
pub fn sink_abilities(body: &str) -> SinkAbilities {
    let mut a = SinkAbilities::default();
    for (name, value) in parameters(body) {
        match name.as_str() {
            "wfd_video_formats" if value != "none" => {
                // native preferred profile level cea vesa hh latency ...,
                // with further codec entries after commas.
                let mut entries = value.split(',');
                if let Some(first) = entries.next() {
                    let f: Vec<&str> = first.split_whitespace().collect();
                    if f.len() >= 7 {
                        if let Ok(p) = u8::from_str_radix(f[2], 16) {
                            a.profiles.push(p);
                        }
                        a.levels = u8::from_str_radix(f[3], 16).unwrap_or(0);
                        a.cea = u32::from_str_radix(f[4], 16).unwrap_or(0);
                    }
                }
                for more in entries {
                    let f: Vec<&str> = more.split_whitespace().collect();
                    if let Some(p) = f.first().and_then(|s| u8::from_str_radix(s, 16).ok()) {
                        if !a.profiles.contains(&p) {
                            a.profiles.push(p);
                        }
                    }
                }
            }
            "wfd_audio_codecs" if value != "none" => {
                a.audio = value
                    .split(',')
                    .filter_map(|c| c.split_whitespace().next())
                    .map(str::to_string)
                    .collect();
            }
            "wfd_client_rtp_ports" => {
                // RTP/AVP/UDP;unicast 53000 0 mode=play
                a.rtp_port = value
                    .split_whitespace()
                    .nth(1)
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0);
            }
            "wfd_content_protection" => a.hdcp = value != "none",
            "wfd_uibc_capability" => a.uibc = value != "none",
            _ => {}
        }
    }
    a
}

/// A picture size Kagaz can send, with its bit in the CEA table of the
/// Wi-Fi Display specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub cea_bit: u32,
}

/// The CEA modes Kagaz sends, best first: at the same height, 60 pictures
/// a second before 30 (moving things look smoother).
pub const MODES: [Mode; 5] = [
    Mode { width: 1920, height: 1080, fps: 60, cea_bit: 8 },
    Mode { width: 1920, height: 1080, fps: 30, cea_bit: 7 },
    Mode { width: 1280, height: 720, fps: 60, cea_bit: 6 },
    Mode { width: 1280, height: 720, fps: 30, cea_bit: 5 },
    Mode { width: 640, height: 480, fps: 60, cea_bit: 0 },
];

/// The best mode the TV takes no larger than `max_height`.
pub fn choose_mode(a: &SinkAbilities, max_height: u32) -> Mode {
    MODES
        .iter()
        .copied()
        .find(|m| m.height <= max_height && a.cea & (1 << m.cea_bit) != 0)
        .unwrap_or(MODES[4])
}

/// The level Kagaz asks for: 3.1 (bit 0x01) up to 720p30, 3.2 (0x02) for
/// 720p60, 4.2 (0x10) for 1080p60, 4 (0x04) for 1080p30.
pub fn level_for(mode: Mode) -> u8 {
    match (mode.height > 720, mode.fps > 30) {
        (true, true) => 0x10,
        (true, false) => 0x04,
        (false, true) => 0x02,
        (false, false) => 0x01,
    }
}

/// The SET_PARAMETER body that fixes the stream (M4).
pub fn stream_parameters(mode: Mode, rtp_port: u16, presentation: &str) -> String {
    let level = level_for(mode);
    format!(
        "wfd_video_formats: 00 00 01 {level:02x} {:08x} 00000000 00000000 00 0000 0000 00 none none\r\n\
         wfd_presentation_URL: {presentation} none\r\n\
         wfd_client_rtp_ports: RTP/AVP/UDP;unicast {rtp_port} 0 mode=play\r\n",
        1u32 << mode.cea_bit
    )
}

/// The parameters the source asks the TV about (M3).
pub const ASKED: [&str; 8] = [
    "wfd_video_formats",
    "wfd_audio_codecs",
    "wfd_client_rtp_ports",
    "wfd_content_protection",
    "wfd_uibc_capability",
    "wfd_connector_type",
    "wfd_idr_request_capability",
    "wfd_standby_resume_capability",
];

#[cfg(test)]
mod tests {
    use super::*;

    const LG: &str = include_str!("../../../tests/fixtures/tv/lg-webos-2024-wfd-m3.txt");

    #[test]
    fn reads_the_lg_answer() {
        let a = sink_abilities(LG);
        assert_eq!(a.profiles, vec![1, 2]);
        assert_eq!(a.levels, 0x10);
        assert_eq!(a.cea, 0x0001_94FF);
        assert_eq!(a.rtp_port, 53000);
        assert_eq!(a.audio, vec!["LPCM", "AAC"]);
        assert!(a.hdcp);
        assert!(a.uibc);
        // The LG takes 1080p at 30 and 720p at 60, not 1080p at 60.
        let m = choose_mode(&a, 1080);
        assert_eq!((m.width, m.height, m.fps), (1920, 1080, 30));
        let m = choose_mode(&a, 720);
        assert_eq!((m.height, m.fps), (720, 60));
    }

    #[test]
    fn messages_round_trip() {
        let m = Message::request("GET_PARAMETER", "rtsp://localhost/wfd1.0", 3)
            .with_parameters("wfd_video_formats\r\n".into());
        let mut buf = m.to_bytes();
        buf.extend_from_slice(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\nRTSP/1.0");
        let got = take_message(&mut buf).unwrap();
        assert_eq!(got, m);
        let reply = take_message(&mut buf).unwrap();
        assert_eq!(reply.status(), 200);
        assert_eq!(reply.cseq(), "1");
        assert!(take_message(&mut buf).is_none());
        assert_eq!(buf, b"RTSP/1.0");
    }

    #[test]
    fn stream_parameters_name_one_mode() {
        let mode = MODES[3];
        let body = stream_parameters(mode, 53000, "rtsp://192.0.2.1/wfd1.0/streamid=0");
        assert!(body.starts_with("wfd_video_formats: 00 00 01 01 00000020 "));
        let body = stream_parameters(MODES[2], 53000, "rtsp://192.0.2.1/wfd1.0/streamid=0");
        assert!(body.starts_with("wfd_video_formats: 00 00 01 02 00000040 "));
        assert!(body.contains("unicast 53000 0 mode=play"));
    }
}
