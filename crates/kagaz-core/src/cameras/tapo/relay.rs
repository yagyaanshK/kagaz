//! Live video through TP-Link's cloud relay: one request to the relay API
//! for a session, then a long-lived TLS connection to the relay carrying a
//! multipart stream whose `video/mp2t` parts are plain MPEG-TS. The same
//! live stream is served to any number of viewers.

use super::cloud::{CloudError, Session, APP_VERSION};
use super::tls::{agent, tls_stream};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

pub const CIPC_HOST: &str = "aps1-cipc-api.i.tplinkcloud.com";
const CLIENT_BOUNDARY: &str = "--client-stream-boundary--";
const DEVICE_BOUNDARY: &str = "--device-stream-boundary--";
const MAX_PART: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayParams {
    pub relay_ip: String,
    pub relay_token: String,
    pub relay_url: String,
    #[serde(default)]
    pub elb_cookie: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error(transparent)]
    Cloud(#[from] CloudError),
    #[error("relay request failed (HTTP {status}, code {code}): {message}")]
    Request {
        status: u16,
        code: i64,
        message: String,
    },
    #[error("cannot reach the relay: {0}")]
    Transport(String),
    #[error("the relay answered HTTP {0}")]
    Status(u16),
    #[error("the camera refused the stream (error {0})")]
    Refused(i64),
    #[error("the relay stream is malformed: {0}")]
    Malformed(String),
}

/// The relay API host for an app server, by region prefix
/// (`aps1-app-server...` -> `aps1-cipc-api...`).
pub fn cipc_host_for(app_server: &str) -> String {
    match app_server.split('-').next() {
        Some(region) if !region.is_empty() && region != app_server => {
            format!("{region}-cipc-api.i.tplinkcloud.com")
        }
        _ => CIPC_HOST.to_string(),
    }
}

/// Ask the cloud for a relay session to the camera's live stream.
/// Live view.
pub const STREAM_PREVIEW: u32 = 0;
/// Recorded footage from the SD card.
pub const STREAM_PLAYBACK: u32 = 1;

pub fn request_relay(
    session: &Session,
    device_id: &str,
    app_server: &str,
    track_id: &str,
    resolution: &str,
) -> Result<RelayParams, RelayError> {
    request_relay_for(
        session,
        device_id,
        app_server,
        track_id,
        resolution,
        STREAM_PREVIEW,
    )
}

/// Ask the cloud for a relay session of the given stream type.
pub fn request_relay_for(
    session: &Session,
    device_id: &str,
    app_server: &str,
    track_id: &str,
    resolution: &str,
    stream_type: u32,
) -> Result<RelayParams, RelayError> {
    let body = json!({
        "cloudType": 2,
        "customParams": "{\"audio_config\":{\"encode_type\":\"G711ulaw\",\"sample_rate\":\"16\"}}",
        "dataTimeoutCount": 0,
        "deviceId": device_id,
        "deviceType": "SMART.IPCAMERA",
        "playerId": session.terminal_uuid,
        "preConnection": 0,
        "resolution": resolution,
        "rootCaVer": "1",
        "streamType": stream_type,
        "trackId": track_id,
    });
    let bytes = serde_json::to_vec(&body).expect("json");
    let url = format!("https://{}/v2/relay/request", cipc_host_for(app_server));
    let response = agent()
        .post(&url)
        .set("Authorization", &session.token)
        .set("X-Client-Id", &session.terminal_uuid)
        .set("X-Source", "tapo-app")
        .set("X-Brand", "TPLINK")
        .set("X-Ca-Type", "cloud-self")
        .set("X-Request-Signature", "Kagaz")
        .set("Content-Type", "application/json; charset=UTF-8")
        .set(
            "User-Agent",
            &format!("Tapo_APP/kagaz/{APP_VERSION}_Android/Android 15"),
        )
        .send_bytes(&bytes);
    let (status, response) = match response {
        Ok(r) => (200, r),
        Err(ureq::Error::Status(401, _)) => return Err(CloudError::Unauthorized.into()),
        Err(ureq::Error::Status(s, r)) => (s, r),
        Err(ureq::Error::Transport(t)) => return Err(RelayError::Transport(t.to_string())),
    };
    let text = response
        .into_string()
        .map_err(|e| RelayError::Transport(e.to_string()))?;
    let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let code = data.get("errorCode").and_then(Value::as_i64).unwrap_or(0);
    if status != 200 || code != 0 {
        return Err(RelayError::Request {
            status,
            code,
            message: text.chars().take(200).collect(),
        });
    }
    let r = data.get("result").cloned().unwrap_or(Value::Null);
    let get = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let params = RelayParams {
        relay_ip: get("relayIp"),
        relay_token: get("relayToken"),
        relay_url: get("relayUrl"),
        elb_cookie: get("elbCookie"),
    };
    if params.relay_url.is_empty() || params.relay_token.is_empty() {
        return Err(RelayError::Malformed(format!(
            "relay answer without url/token: {}",
            text.chars().take(200).collect::<String>()
        )));
    }
    Ok(params)
}

/// (host, request path with query) from the relay URL.
pub fn split_relay_url(relay_url: &str) -> Option<(String, String)> {
    let rest = relay_url.split("://").nth(1)?;
    let (host, path) = rest.split_once('/')?;
    Some((host.to_string(), format!("/{path}&retryTime=0")))
}

/// The HTTP head that opens a relay media session, as the app sends it.
pub fn relay_head(
    relay: &RelayParams,
    terminal_uuid: &str,
    track_id: &str,
    host: &str,
    path: &str,
) -> String {
    format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         User-Agent: Client=TP-Link_Tapo_Android Android {APP_VERSION}/1.3\r\n\
         Accept: */*\r\n\
         Keep-Relay: 3600\r\n\
         Content-Type: multipart/mixed;boundary={CLIENT_BOUNDARY}\r\n\
         Content-Length: 9223372036854775807\r\n\
         X-token: {}\r\n\
         X-Client-Model: Kagaz\r\n\
         X-Client-UUID: {terminal_uuid}\r\n\
         X-Track-Id: {track_id}\r\n\
         X-Redirect-Times: 0\r\n\
         X-Pull-Mode: 2\r\n\
         X-Compete: 1\r\n\
         X-Version: 2.0\r\n\
         X-Arrive-Latency: 200\r\n\
         \r\n",
        relay.relay_token
    )
}

/// One multipart control frame carrying a JSON request.
pub fn control_frame(payload: &str) -> Vec<u8> {
    let mut v = format!(
        "--{CLIENT_BOUNDARY}\r\nContent-Type: application/json\r\nX-Data-Window-Size: 40\r\nContent-Length: {}\r\n\r\n",
        payload.len()
    )
    .into_bytes();
    v.extend_from_slice(payload.as_bytes());
    v.extend_from_slice(b"\r\n");
    v
}

pub fn preview_request(resolution: &str) -> String {
    format!(
        "{{\"type\":\"request\",\"seq\":1,\"params\":{{\"preview\":{{\"audio\":[\"default\"],\"channels\":[0],\"resolutions\":[\"{resolution}\"]}},\"method\":\"get\"}}}}"
    )
}

/// The control frame that asks for recorded footage between two unix times.
pub fn playback_request(seq: u32, client_id: u64, start: i64, end: i64) -> String {
    format!(
        "{{\"type\":\"request\",\"seq\":{seq},\"params\":{{\"playback\":{{\"client_id\":{client_id},\"channels\":[0,1],\"scale\":\"1/1\",\"start_time\":\"{start}\",\"end_time\":\"{end}\",\"event_type\":[1,2]}},\"method\":\"get\"}}}}"
    )
}

/// The control frame that asks for recorded footage as a download.
pub fn download_request(seq: u32, client_id: u64, player_id: &str, start: i64, end: i64) -> String {
    format!(
        "{{\"type\":\"request\",\"seq\":{seq},\"params\":{{\"download\":{{\"client_id\":{client_id},\"channels\":[0],\"media_type\":0,\"start_time\":\"{start}\",\"end_time\":\"{end}\",\"player_id\":\"{player_id}\"}},\"method\":\"get\"}}}}"
    )
}

/// The frame that ends a playback session so the camera frees its slot.
pub fn playback_stop(seq: u32, client_id: u64) -> String {
    format!(
        "{{\"type\":\"request\",\"seq\":{seq},\"params\":{{\"playback\":{{\"client_id\":{client_id},\"channels\":[0,1]}},\"method\":\"stop\"}}}}"
    )
}

/// Does a control notification say the recording has finished?
pub fn is_end_notification(payload: &[u8]) -> bool {
    let s = String::from_utf8_lossy(payload);
    (s.contains("stream_status") && s.contains("finished"))
        || s.contains("playback_done")
        || s.contains("playfinished")
}

/// The `error_code` of a control response, 0 when it is not one.
pub fn control_error(payload: &[u8]) -> i64 {
    let Ok(v) = serde_json::from_slice::<Value>(payload) else {
        return 0;
    };
    if v.get("type").and_then(Value::as_str) != Some("response") {
        return 0;
    }
    v.get("params")
        .and_then(|p| p.get("error_code"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// How many consecutive 12-second read timeouts to tolerate before giving up
/// on a silent relay (a minute in all).
const QUIET_READS: u32 = 5;

fn read_line(r: &mut impl BufRead) -> Result<String, RelayError> {
    let mut s = String::new();
    let mut quiet = 0;
    loop {
        match r.read_line(&mut s) {
            Ok(0) => {
                return Err(RelayError::Transport(
                    "the relay closed the connection".into(),
                ))
            }
            Ok(_) => return Ok(s),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                quiet += 1;
                if quiet >= QUIET_READS {
                    return Err(RelayError::Transport(format!(
                        "no data from the relay for {} seconds",
                        12 * QUIET_READS
                    )));
                }
            }
            Err(e) => return Err(RelayError::Transport(e.to_string())),
        }
    }
}

fn read_headers(r: &mut impl BufRead) -> Result<Vec<(String, String)>, RelayError> {
    let mut out = Vec::new();
    loop {
        let line = read_line(r)?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return Ok(out);
        }
        if let Some((k, v)) = line.split_once(':') {
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
}

/// What a relay media session should ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    /// Live view at this profile ("HD" or "VGA").
    Preview(&'a str),
    /// Recorded footage between two unix times, paced at real time, as
    /// device user `client_id`. The camera does not stop at `end`; the
    /// reader does, by measuring the video's own clock.
    Playback {
        client_id: u64,
        start: i64,
        end: i64,
    },
    /// The same footage as fast as the link allows (what the app sends when
    /// you tap download); the camera stops by itself at `end`.
    Download {
        client_id: u64,
        start: i64,
        end: i64,
    },
}

/// Open the camera's live stream over the relay and hand every MPEG-TS part
/// to `sink` until it returns `false` or the relay stops.
pub fn stream_preview(
    relay: &RelayParams,
    terminal_uuid: &str,
    track_id: &str,
    resolution: &str,
    sink: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<u64, RelayError> {
    stream(
        relay,
        terminal_uuid,
        track_id,
        Request::Preview(resolution),
        sink,
    )
}

/// Pull recorded footage between `start` and `end` (unix seconds) over the
/// relay, handing every MPEG-TS part to `sink`. Ends when the camera says
/// the span is finished, the relay goes quiet, or `sink` returns `false`.
pub fn stream_playback(
    relay: &RelayParams,
    terminal_uuid: &str,
    track_id: &str,
    client_id: u64,
    start: i64,
    end: i64,
    sink: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<u64, RelayError> {
    stream(
        relay,
        terminal_uuid,
        track_id,
        Request::Playback {
            client_id,
            start,
            end,
        },
        sink,
    )
}

/// Pull recorded footage between `start` and `end` as fast as the camera
/// sends it; ends when the camera reports the download finished.
pub fn stream_download(
    relay: &RelayParams,
    terminal_uuid: &str,
    track_id: &str,
    client_id: u64,
    start: i64,
    end: i64,
    sink: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<u64, RelayError> {
    stream(
        relay,
        terminal_uuid,
        track_id,
        Request::Download {
            client_id,
            start,
            end,
        },
        sink,
    )
}

/// Elapsed media time from the PTS values in MPEG-TS packets (90 kHz), so
/// a real-time playback can stop once the requested span has arrived.
#[derive(Debug, Default, Clone, Copy)]
pub struct PtsTracker {
    first: u64,
    last: u64,
    have: bool,
}

impl PtsTracker {
    pub fn update(&mut self, buf: &[u8]) {
        let n = buf.len();
        let mut i = 0;
        while i + 188 <= n {
            if buf[i] != 0x47 {
                i += 1;
                continue;
            }
            let packet = &buf[i..i + 188];
            i += 188;
            if packet[1] & 0x40 == 0 {
                continue;
            }
            let mut off = 4;
            if (packet[3] >> 4) & 0x3 >= 2 {
                off += 1 + packet[4] as usize;
            }
            if off + 14 > 188 || packet[off..off + 3] != [0, 0, 1] || packet[off + 7] & 0x80 == 0 {
                continue;
            }
            let p = &packet[off + 9..off + 14];
            let pts = ((u64::from(p[0]) & 0x0E) << 29)
                | (u64::from(p[1]) << 22)
                | ((u64::from(p[2]) & 0xFE) << 14)
                | (u64::from(p[3]) << 7)
                | (u64::from(p[4]) >> 1);
            if !self.have {
                self.first = pts;
                self.last = pts;
                self.have = true;
            } else if pts > self.last {
                self.last = pts;
            }
        }
    }

    /// Seconds of video seen so far.
    pub fn seconds(&self) -> f64 {
        if self.have {
            (self.last - self.first) as f64 / 90_000.0
        } else {
            0.0
        }
    }
}

fn stream(
    relay: &RelayParams,
    terminal_uuid: &str,
    track_id: &str,
    request: Request<'_>,
    sink: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<u64, RelayError> {
    let (host, path) = split_relay_url(&relay.relay_url)
        .ok_or_else(|| RelayError::Malformed(format!("relay url {}", relay.relay_url)))?;
    let dial = if relay.relay_ip.is_empty() {
        host.clone()
    } else {
        relay.relay_ip.clone()
    };
    let mut tls = tls_stream(&dial, 443, &host, Duration::from_secs(15))
        .map_err(|e| RelayError::Transport(e.to_string()))?;
    tls.sock
        .set_read_timeout(Some(Duration::from_secs(12)))
        .map_err(|e| RelayError::Transport(e.to_string()))?;
    let first = match request {
        Request::Preview(resolution) => preview_request(resolution),
        Request::Playback {
            client_id,
            start,
            end,
        } => playback_request(1, client_id, start, end),
        Request::Download {
            client_id,
            start,
            end,
        } => download_request(1, client_id, terminal_uuid, start, end),
    };
    let recorded = !matches!(request, Request::Preview(_));
    let want_seconds = match request {
        Request::Playback { start, end, .. } => Some((end - start).max(1) as f64),
        _ => None,
    };
    let mut pts = PtsTracker::default();
    tls.write_all(relay_head(relay, terminal_uuid, track_id, &host, &path).as_bytes())
        .and_then(|()| tls.write_all(&control_frame(&first)))
        .and_then(|()| tls.flush())
        .map_err(|e| RelayError::Transport(e.to_string()))?;

    let mut reader = BufReader::with_capacity(1 << 16, tls);
    let status_line = read_line(&mut reader)?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| RelayError::Malformed(format!("status line {status_line:?}")))?;
    let _headers = read_headers(&mut reader)?;
    if status != 200 {
        return Err(RelayError::Status(status));
    }

    let mut total = 0u64;
    let outcome = 'parts: loop {
        // Skip to the next device boundary.
        loop {
            match read_line(&mut reader) {
                Ok(line) if line.contains(DEVICE_BOUNDARY) => break,
                Ok(_) => {}
                // A recording simply ends when the camera closes the stream.
                Err(RelayError::Transport(_)) if recorded => break 'parts Ok(total),
                Err(e) => return Err(e),
            }
        }
        let headers = read_headers(&mut reader)?;
        let content_type = headers
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let length: usize = headers
            .iter()
            .find(|(k, _)| k == "content-length")
            .and_then(|(_, v)| v.parse().ok())
            .ok_or_else(|| RelayError::Malformed("part without content-length".into()))?;
        if length > MAX_PART {
            return Err(RelayError::Malformed(format!("part of {length} bytes")));
        }
        let mut payload = vec![0u8; length];
        reader
            .read_exact(&mut payload)
            .map_err(|e| RelayError::Transport(e.to_string()))?;
        if content_type.contains("video/mp2t") {
            total += length as u64;
            if !sink(&payload) {
                break Ok(total);
            }
            if let Some(want) = want_seconds {
                pts.update(&payload);
                if pts.seconds() >= want {
                    break Ok(total);
                }
            }
        } else if content_type.contains("application/json") {
            let code = control_error(&payload);
            if code != 0 {
                break Err(RelayError::Refused(code));
            }
            if recorded && is_end_notification(&payload) {
                break Ok(total);
            }
        }
    };
    if let Request::Playback { client_id, .. } | Request::Download { client_id, .. } = request {
        // Free the camera's playback slot; best effort, the relay may be gone.
        let tls = reader.get_mut();
        let _ = tls
            .write_all(&control_frame(&playback_stop(2, client_id)))
            .and_then(|()| tls.flush());
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_hosts_follow_the_region() {
        assert_eq!(
            cipc_host_for("aps1-app-server.iot.i.tplinkcloud.com"),
            "aps1-cipc-api.i.tplinkcloud.com"
        );
        assert_eq!(
            cipc_host_for("euw1-app-server.iot.i.tplinkcloud.com"),
            "euw1-cipc-api.i.tplinkcloud.com"
        );
        assert_eq!(cipc_host_for("weird"), CIPC_HOST);
    }

    #[test]
    fn relay_url_and_head_are_shaped_like_the_app() {
        let (host, path) =
            split_relay_url("https://r1.example.com/relayservice?deviceid=X&blob=Y").unwrap();
        assert_eq!(host, "r1.example.com");
        assert_eq!(path, "/relayservice?deviceid=X&blob=Y&retryTime=0");
        let relay = RelayParams {
            relay_ip: "203.0.113.9".into(),
            relay_token: "token=abc;tokenType=cipcAuthToken".into(),
            relay_url: "https://r1.example.com/relayservice?x=1".into(),
            elb_cookie: String::new(),
        };
        let head = relay_head(&relay, "UUID", "preview-X", &host, &path);
        assert!(head.starts_with(
            "POST /relayservice?deviceid=X&blob=Y&retryTime=0 HTTP/1.1\r\nHost: r1.example.com\r\n"
        ));
        assert!(head.contains("X-token: token=abc;tokenType=cipcAuthToken\r\n"));
        assert!(
            head.contains("Content-Type: multipart/mixed;boundary=--client-stream-boundary--\r\n")
        );
        assert!(head.ends_with("\r\n\r\n"));
        let frame = control_frame(&preview_request("HD"));
        let text = String::from_utf8(frame).unwrap();
        assert!(
            text.starts_with("----client-stream-boundary--\r\nContent-Type: application/json\r\n")
        );
        assert!(text.contains("\"resolutions\":[\"HD\"]"));
        assert_eq!(
            control_error(br#"{"type":"response","seq":1,"params":{"error_code":-52405}}"#),
            -52405
        );
        assert_eq!(control_error(br#"{"type":"notification"}"#), 0);
    }
}
