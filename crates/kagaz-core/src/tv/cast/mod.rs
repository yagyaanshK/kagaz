//! Screen mirroring through Google Cast, as Chrome's "Cast screen" does
//! it: the TV starts its mirroring receiver, both sides agree a stream
//! (H.264 here, so the same encoders serve as for Miracast), and the
//! pictures go over UDP, encrypted, each kept until the TV has it.
//!
//! This reaches any TV with Cast built in that is on the same network,
//! from a computer and from a phone alike, with nothing to switch on at
//! the TV. Sound is not sent yet.

pub mod channel;
pub mod stream;

use super::miracast::rtsp::Mode;
use super::miracast::{video, Event, Options, Pictures, Report};
use channel::{Channel, RECEIVER, STREAMING, TV};
use serde_json::{json, Value};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use stream::{Agreed, Sender};

/// Chrome's mirroring receiver, which TVs with Cast built in carry.
const APP: &str = "0F5096E8";
/// Cast's number for H.264 pictures.
const H264: u8 = 101;
/// How long after its moment the TV shows a picture: room for a missed
/// packet to come again, short enough that a pointer follows the hand.
const DELAY: Duration = Duration::from_millis(200);

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// "60" or "60000/1001" as pictures a second.
fn rate(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    let s = v.as_str()?;
    match s.split_once('/') {
        Some((a, b)) => Some(a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok()?),
        None => s.trim().parse().ok(),
    }
}

/// The offer: one H.264 stream at this size and rate.
fn offer(mode: Mode, bits_per_second: u32, agreed: &Agreed) -> Value {
    json!({
        "type": "OFFER",
        "seqNum": 1,
        "offer": {
            "castMode": "mirroring",
            "receiverGetStatus": true,
            "supportedStreams": [{
                "index": 0,
                "type": "video_source",
                "codecName": "h264",
                "rtpProfile": "cast",
                "rtpPayloadType": agreed.payload_type,
                "ssrc": agreed.ssrc,
                "targetDelay": agreed.delay.as_millis() as u64,
                "aesKey": hex(&agreed.key),
                "aesIvMask": hex(&agreed.iv_mask),
                "timeBase": "1/90000",
                "receiverRtcpEventLog": false,
                "renderMode": "video",
                "maxFrameRate": mode.fps.to_string(),
                "maxBitRate": bits_per_second,
                "resolutions": [{ "width": mode.width, "height": mode.height }],
                "errorRecoveryMode": "castv2",
            }],
        },
    })
}

/// What the TV answered: the UDP port it listens on, its number for its
/// reports, and the most pictures a second its screen shows.
fn answer(said: &Value) -> Result<(u16, u32, Option<f64>), String> {
    if said["result"] != "ok" {
        let why = said["error"]["description"].as_str().unwrap_or("no reason given");
        return Err(format!("the TV refused the stream Kagaz offered ({why})"));
    }
    let a = &said["answer"];
    let port = a["udpPort"].as_u64().filter(|p| (1..=65535).contains(p)).ok_or("the TV's answer names no port")?;
    let takes = a["sendIndexes"].as_array().is_some_and(|i| i.iter().any(|n| n == 0));
    let ssrc = a["ssrcs"].as_array().and_then(|s| s.first()).and_then(Value::as_u64);
    let (true, Some(ssrc)) = (takes, ssrc) else { return Err("the TV did not take H.264 pictures over Cast".into()) };
    let most = rate(&a["constraints"]["video"]["maxDimensions"]["frameRate"]).or_else(|| rate(&a["display"]["dimensions"]["frameRate"]));
    Ok((port as u16, ssrc as u32, most))
}

/// Mirrors to the TV at this address through Google Cast until `stop` is
/// set, the TV ends it, or the length runs out.
pub fn mirror(
    tv: IpAddr,
    opts: &Options,
    pictures: &mut dyn Pictures,
    stop: &AtomicBool,
    mut event: impl FnMut(Event),
) -> Result<Report, String> {
    let mut ch = Channel::open(tv, Duration::from_secs(8))?;
    event(Event::Waiting);
    ch.send(TV, RECEIVER, &json!({ "type": "LAUNCH", "appId": APP, "requestId": 1 }))?;
    // The TV's status names the app once it is up, with the address to
    // talk to it on.
    let up = ch.until(opts.wait.max(Duration::from_secs(20)), stop, |s| match s.kind() {
        "LAUNCH_ERROR" => Some(Err(format!(
            "the TV would not start Cast's screen mirroring ({})",
            s.json["reason"].as_str().unwrap_or("no reason given")
        ))),
        "RECEIVER_STATUS" => s.json["status"]["applications"].as_array().and_then(|apps| {
            let app = apps.iter().find(|a| a["appId"] == APP)?;
            Some(Ok((app["transportId"].as_str()?.to_string(), app["sessionId"].as_str()?.to_string())))
        }),
        _ => None,
    })?;
    let Some((app, session)) = up else {
        return Err("the TV did not start Cast's screen mirroring. Is Cast (Chromecast built-in) on in its settings?".into());
    };
    event(Event::Connected(tv));

    let result = (|| -> Result<Report, String> {
        ch.greet(&app)?;
        let fps = if opts.priority == video::Priority::Battery { 30 } else { 60 };
        let mut mode = if opts.max_height >= 1080 {
            Mode { width: 1920, height: 1080, fps, cea_bit: 0 }
        } else {
            Mode { width: 1280, height: 720, fps, cea_bit: 0 }
        };
        let bits = bit_rate(mode);
        // The TV's number is this one plus one, by Cast's habit; its answer
        // says so itself.
        let ssrc = 50_001 + u32::from_be_bytes(super::miracast::random_bytes()) % 50_000;
        let mut agreed = Agreed {
            ssrc,
            tv_ssrc: ssrc + 1,
            key: super::miracast::random_bytes(),
            iv_mask: super::miracast::random_bytes(),
            payload_type: H264,
            delay: DELAY,
        };
        ch.send(&app, STREAMING, &offer(mode, bits, &agreed))?;
        let answered = ch.until(Duration::from_secs(10), stop, |s| (s.kind() == "ANSWER").then(|| answer(&s.json)))?;
        let (port, tv_ssrc, most) = answered.ok_or("the TV did not answer the stream Kagaz offered")?;
        agreed.tv_ssrc = tv_ssrc;
        if most.is_some_and(|m| m < 50.0) {
            mode.fps = 30;
        }
        let udp = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| format!("no UDP port: {e}"))?;
        let _ = socket2::SockRef::from(&udp).set_send_buffer_size(4 << 20);
        // The TV's status without the app, or the app closing the
        // conversation, means the TV ended it (its remote's Back).
        let ended = |ch: &mut Channel| -> Result<bool, String> {
            Ok(ch.heard()?.iter().any(|s| match s.kind() {
                "CLOSE" => s.from == app,
                "RECEIVER_STATUS" => {
                    !s.json["status"]["applications"].as_array().is_some_and(|apps| apps.iter().any(|a| a["appId"] == APP))
                }
                _ => false,
            }))
        };
        run(&udp, SocketAddr::new(tv, port), agreed, mode, opts, pictures, stop, &mut event, &mut || ended(&mut ch))
    })();
    // Closing the app puts the TV back where it was.
    let _ = ch.send(TV, RECEIVER, &json!({ "type": "STOP", "sessionId": session, "requestId": 2 }));
    result
}

/// Room for whole-screen changes; a still desktop uses far less.
fn bit_rate(mode: Mode) -> u32 {
    match (mode.height > 720, mode.fps > 30) {
        (true, true) => 16_000_000,
        (true, false) => 10_000_000,
        (false, true) => 8_000_000,
        (false, false) => 6_000_000,
    }
}

/// Sends the pictures over the agreed stream. `tv_ended` is asked now and
/// then whether the TV ended the session.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    udp: &UdpSocket,
    target: SocketAddr,
    agreed: Agreed,
    mode: Mode,
    opts: &Options,
    pictures: &mut dyn Pictures,
    stop: &AtomicBool,
    event: &mut dyn FnMut(Event),
    tv_ended: &mut dyn FnMut() -> Result<bool, String>,
) -> Result<Report, String> {
    let mut report = Report { mode: Some(mode), ..Default::default() };
    let bits = bit_rate(mode);
    let (w, h) = (mode.width as usize, mode.height as usize);
    let (mut encoder, which) = video::AnyVideo::open(w, h, mode.fps, bits, opts.engine, opts.priority)?;
    event(Event::Encoder(which));
    event(Event::Playing(mode));
    let mut sender = Sender::new(agreed);
    // The wait for the TV's reports is the loop's tick.
    udp.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let frame_time = Duration::from_secs_f64(1.0 / mode.fps as f64);
    let mut next_frame = start;
    let mut last_progress = start;
    let mut last_asked = start;
    let mut bytes_at_progress = 0u64;
    let (mut busy, mut worst, mut made) = (Duration::ZERO, Duration::ZERO, 0u32);
    let mut delivered_at_progress = pictures.delivered();
    // Packets leave at a steady pace, not a picture at a time in a burst
    // (a burst overflows a Wi-Fi link): at most twice the stream's rate,
    // and no less than 24 Mbit/s, with 10 ms saved up at most.
    let pace = (bits as f64 * 2.0).max(24_000_000.0) / 8.0 / stream::PACKET as f64;
    let mut allowance = 0f64;
    let mut paced_at = start;
    let mut buf = [0u8; 2048];
    #[cfg(target_os = "linux")]
    let mut gpu_ok = true;
    loop {
        if stop.load(Ordering::Relaxed) {
            report.ended_by = "you".into();
            break;
        }
        if opts.length.is_some_and(|l| start.elapsed() >= l) {
            report.ended_by = "time".into();
            break;
        }
        match udp.recv_from(&mut buf) {
            Ok((n, from)) if from.ip() == target.ip() => sender.heard(&buf[..n], Instant::now()),
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            // A port the TV has closed answers with a refusal on Linux.
            Err(e) if e.kind() == ErrorKind::ConnectionRefused => {}
            Err(e) => return Err(format!("the stream to the TV broke: {e}")),
        }
        let now = Instant::now();
        if let Some(r) = sender.report(now) {
            let _ = udp.send_to(&r, target);
        }
        allowance = (allowance + now.duration_since(paced_at).as_secs_f64() * pace).min(pace / 100.0);
        paced_at = now;
        while allowance >= 1.0 {
            let Some(p) = sender.packet(now) else { break };
            let _ = udp.send_to(&p, target);
            allowance -= 1.0;
        }
        if now.duration_since(last_asked) >= Duration::from_millis(50) {
            last_asked = now;
            if tv_ended()? {
                report.ended_by = "the TV".into();
                break;
            }
        }
        if last_progress.elapsed() >= Duration::from_secs(1) {
            let secs = last_progress.elapsed().as_secs_f64();
            event(Event::Progress {
                seconds: start.elapsed().as_secs(),
                frames: report.frames,
                kilobits_per_second: ((sender.tally.bytes - bytes_at_progress) as f64 * 8.0 / 1000.0 / secs) as u64,
                busy_ms: busy.as_secs_f32() * 1000.0 / made.max(1) as f32,
                worst_ms: worst.as_secs_f32() * 1000.0,
                budget_ms: frame_time.as_secs_f32() * 1000.0,
                dropped: report.dropped,
                delivered_per_second: ((pictures.delivered() - delivered_at_progress) as f64 / secs) as u64,
            });
            delivered_at_progress = pictures.delivered();
            (busy, worst, made) = (Duration::ZERO, Duration::ZERO, 0);
            bytes_at_progress = sender.tally.bytes;
            last_progress = Instant::now();
        }
        if now < next_frame {
            continue;
        }
        // Each picture is stamped with the time it was due.
        let due = next_frame - start;
        next_frame += frame_time;
        if now > next_frame + frame_time * 4 {
            let behind = (now - next_frame).as_secs_f64() / frame_time.as_secs_f64();
            report.dropped += behind as u64 + 1;
            next_frame = now + frame_time;
        }
        let rtp = (due.as_micros() as u64 * stream::CLOCK / 1_000_000) as u32;
        // The TV is behind (a weak link): this picture is not made, so
        // that every picture made is one the next can build on.
        if !sender.room(rtp) {
            report.dropped += 1;
            continue;
        }
        if sender.take_key_request(now) {
            encoder.key_next();
            event(Event::KeyFrameAsked);
        }
        let began = Instant::now();
        #[cfg(target_os = "linux")]
        let from_gpu = if gpu_ok && encoder.takes_gpu() {
            match pictures.gpu_picture() {
                Some(g) => match encoder.encode_gpu(g.as_deref()) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        gpu_ok = false;
                        event(Event::Encoder(format!("the screen goes through the processor now: {e}")));
                        None
                    }
                },
                None => None,
            }
        } else {
            None
        };
        #[cfg(not(target_os = "linux"))]
        let from_gpu = None;
        let picture = match from_gpu {
            Some(p) => p,
            None => encoder.encode(pictures.picture(due, w, h))?,
        };
        if picture.data.is_empty() {
            continue;
        }
        sender.queue(&picture.data, picture.key, rtp, start + due);
        report.frames += 1;
        let took = began.elapsed();
        busy += took;
        worst = worst.max(took);
        made += 1;
    }
    report.bytes = sender.tally.bytes;
    report.sent_again = sender.tally.again;
    report.whole_asked = sender.tally.lost_pictures;
    report.round_trip_ms = Some(sender.round_trip().as_secs_f32() * 1000.0).filter(|ms| *ms > 0.0);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn the_offer_and_the_answer() {
        let agreed = Agreed { ssrc: 60_002, tv_ssrc: 60_003, key: [1; 16], iv_mask: [0xab; 16], payload_type: H264, delay: DELAY };
        let o = offer(Mode { width: 1920, height: 1080, fps: 60, cea_bit: 0 }, 16_000_000, &agreed);
        let s = &o["offer"]["supportedStreams"][0];
        assert_eq!((s["codecName"].as_str(), s["rtpPayloadType"].as_u64(), s["ssrc"].as_u64()), (Some("h264"), Some(101), Some(60_002)));
        assert_eq!(s["aesKey"], "01010101010101010101010101010101");
        assert_eq!(s["aesIvMask"].as_str().unwrap().len(), 32);
        assert_eq!(s["resolutions"][0]["height"], 1080);

        // An LG's answer (2024 webOS), its port changed.
        let lg = json!({"answer": {"display": {"dimensions": {"frameRate": "60", "height": 2160, "width": 3840}, "scaling": "sender"},
            "sendIndexes": [0], "ssrcs": [60003], "udpPort": 50000}, "result": "ok", "seqNum": 1, "type": "ANSWER"});
        assert_eq!(answer(&lg).unwrap(), (50000, 60003, Some(60.0)));
        let no = json!({"result": "error", "error": {"code": 1, "description": "no codec in common"}, "type": "ANSWER"});
        assert!(answer(&no).unwrap_err().contains("no codec in common"));
        assert_eq!(rate(&json!("60000/1001")).map(|r| r.round()), Some(60.0));
    }

    /// A whole stream to a stand-in receiver over this computer's own
    /// loopback, a packet in every 20 lost on the way: what arrives
    /// decodes, picture after picture.
    #[test]
    fn pictures_reach_a_stand_in_receiver_and_decode() {
        let agreed = Agreed { ssrc: 60_002, tv_ssrc: 60_003, key: [7; 16], iv_mask: [9; 16], payload_type: H264, delay: DELAY };
        let tv = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let target = tv.local_addr().unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let (done_tv, agreed_tv) = (done.clone(), agreed.clone());
        let receiver = std::thread::spawn(move || {
            let mut r = stream::Receiver::new(agreed_tv);
            let mut decoder = openh264::decoder::Decoder::new().unwrap();
            tv.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
            let (mut whole, mut decoded, mut packets, mut sender) = (0u32, 0u32, 0u64, None);
            let mut buf = [0u8; 2048];
            let mut last_report = Instant::now();
            while !done_tv.load(Ordering::Relaxed) {
                if let Ok((n, from)) = tv.recv_from(&mut buf) {
                    sender = Some(from);
                    packets += 1;
                    if packets % 20 != 0 {
                        r.take(&buf[..n], Instant::now());
                    }
                }
                while let Some(p) = r.picture() {
                    whole += 1;
                    for nal in openh264::nal_units(&p) {
                        if let Ok(Some(yuv)) = decoder.decode(nal) {
                            use openh264::formats::YUVSource;
                            assert_eq!(yuv.dimensions(), (1280, 720));
                            decoded += 1;
                        }
                    }
                }
                if let (Some(to), true) = (sender, last_report.elapsed() >= Duration::from_millis(10)) {
                    last_report = Instant::now();
                    let _ = tv.send_to(&r.report(Instant::now()), to);
                }
            }
            (whole, decoded)
        });
        let opts = Options {
            name: "Kagaz test".into(),
            max_height: 720,
            engine: video::Engine::Software,
            priority: video::Priority::Performance,
            length: Some(Duration::from_millis(2000)),
            wait: Duration::from_secs(5),
        };
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let stop = AtomicBool::new(false);
        let mut pattern = video::Pattern::new(16, 16);
        let mut events = Vec::new();
        let mode = Mode { width: 1280, height: 720, fps: 30, cea_bit: 0 };
        let report = run(&udp, target, agreed, mode, &opts, &mut pattern, &stop, &mut |e| events.push(e), &mut || Ok(false)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        done.store(true, Ordering::Relaxed);
        let (whole, decoded) = receiver.join().unwrap();
        assert_eq!(report.ended_by, "time");
        assert!(report.frames >= 40, "{} pictures made", report.frames);
        // All but the last few (still on their way when the time ran out).
        assert!(whole as u64 + 6 >= report.frames, "{whole} of {} pictures arrived whole", report.frames);
        assert!(decoded + 2 >= whole, "{decoded} of {whole} pictures decoded");
        assert!(events.iter().any(|e| matches!(e, Event::Playing(_))));
    }
}
