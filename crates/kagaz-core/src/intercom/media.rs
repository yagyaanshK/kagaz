//! Media negotiation for an answered call: the SDP we send back, and the
//! RTP endpoints for voice and the door camera's video. Actual audio
//! playback/capture and video decode plug in on top of these; what's here
//! is the wire format — enough to answer a call and hold the media open.

/// Build the SDP answer. We offer to receive and send G.711 (PCMU/PCMA)
/// audio with RFC 2833 tones, and H.264 video when the caller offered it.
/// `audio_port`/`video_port` are the local UDP ports we will receive RTP on.
pub fn answer_sdp(local_ip: &str, audio_port: u16, video_port: u16, with_video: bool) -> String {
    let mut s = String::new();
    s.push_str("v=0\r\n");
    s.push_str(&format!("o=kagaz 0 0 IN IP4 {local_ip}\r\n"));
    s.push_str("s=Kagaz\r\n");
    s.push_str(&format!("c=IN IP4 {local_ip}\r\n"));
    s.push_str("t=0 0\r\n");
    // Audio: PCMU (0), PCMA (8), and telephone-event (101) for DTMF.
    s.push_str(&format!("m=audio {audio_port} RTP/AVP 0 8 101\r\n"));
    s.push_str("a=rtpmap:0 PCMU/8000\r\n");
    s.push_str("a=rtpmap:8 PCMA/8000\r\n");
    s.push_str("a=rtpmap:101 telephone-event/8000\r\n");
    s.push_str("a=fmtp:101 0-15\r\n");
    s.push_str("a=sendrecv\r\n");
    if with_video {
        // H.264 (104), the door camera's codec.
        s.push_str(&format!("m=video {video_port} RTP/AVP 104\r\n"));
        s.push_str("a=rtpmap:104 H264/90000\r\n");
        s.push_str("a=recvonly\r\n");
    }
    s
}

/// A SIP INFO body that asks the station to press DTMF `digit` — how we open
/// the door (the station maps `#` to its relay). `application/dtmf-relay`.
pub fn dtmf_info_body(digit: char) -> String {
    format!("Signal={digit}\r\nDuration=250\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_offers_g711_dtmf_and_optional_video() {
        let sdp = answer_sdp("192.0.2.6", 40000, 40002, true);
        assert!(sdp.contains("c=IN IP4 192.0.2.6"));
        assert!(sdp.contains("m=audio 40000 RTP/AVP 0 8 101"));
        assert!(sdp.contains("a=rtpmap:101 telephone-event/8000"));
        assert!(sdp.contains("m=video 40002 RTP/AVP 104"));
        assert!(sdp.contains("a=rtpmap:104 H264/90000"));
    }

    #[test]
    fn audio_only_answer_has_no_video_line() {
        let sdp = answer_sdp("10.0.0.2", 5000, 0, false);
        assert!(sdp.contains("m=audio 5000"));
        assert!(!sdp.contains("m=video"));
    }

    #[test]
    fn dtmf_body_names_the_signal() {
        assert_eq!(dtmf_info_body('#'), "Signal=#\r\nDuration=250\r\n");
    }
}
