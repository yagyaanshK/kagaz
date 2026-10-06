//! Diagnostic: open a relay media session of a given stream type (0 live,
//! 1 recorded) with a control frame of your own and print every control
//! answer, for working out what a camera accepts.
//!
//! `cargo run -p kagaz-core --example tapo_probe -- <device-id> <stream-type> '<frame json>' [track-id] ['<stop frame json>']`
//! `PROBE_SECS` bounds how long it listens (default 25).
use kagaz_core::cameras::tapo::relay::{
    control_frame, relay_head, request_relay_for, split_relay_url,
};
use kagaz_core::cameras::tapo::tls::tls_stream;
use kagaz_core::cameras::tapo::Session;
use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::load(&Session::default_path()).expect("session");
    let cams = session.cameras().expect("cameras");
    let cam = cams
        .iter()
        .find(|c| c.device_id == args[1])
        .expect("camera");
    let stream_type: u32 = args[2].parse().unwrap();
    let frame = &args[3];
    let track = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| format!("backup-{}-{}", cam.device_id, 1));
    let relay = request_relay_for(
        &session,
        &cam.device_id,
        &cam.app_server,
        &track,
        "HD",
        stream_type,
    )
    .expect("relay");
    let (host, path) = split_relay_url(&relay.relay_url).unwrap();
    let dial = if relay.relay_ip.is_empty() {
        host.clone()
    } else {
        relay.relay_ip.clone()
    };
    let mut tls = tls_stream(&dial, 443, &host, Duration::from_secs(15)).expect("tls");
    tls.sock
        .set_read_timeout(Some(Duration::from_secs(12)))
        .unwrap();
    tls.write_all(relay_head(&relay, &session.terminal_uuid, &track, &host, &path).as_bytes())
        .unwrap();
    tls.write_all(&control_frame(frame)).unwrap();
    tls.flush().unwrap();
    let mut r = BufReader::new(tls);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    println!("status: {}", line.trim());
    loop {
        line.clear();
        if r.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
            break;
        }
    }
    let started = std::time::Instant::now();
    let mut video = 0u64;
    let mut parts = 0;
    let limit: u64 = std::env::var("PROBE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    while started.elapsed() < Duration::from_secs(limit) {
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => {
                println!("closed");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                println!("read: {e}");
                break;
            }
        }
        if !line.contains("--device-stream-boundary--") {
            continue;
        }
        let mut ct = String::new();
        let mut len = 0usize;
        loop {
            line.clear();
            r.read_line(&mut line).unwrap();
            let t = line.trim();
            if t.is_empty() {
                break;
            }
            let (k, v) = t.split_once(':').unwrap_or((t, ""));
            if k.eq_ignore_ascii_case("content-type") {
                ct = v.trim().to_string();
            }
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload).unwrap();
        parts += 1;
        if ct.contains("mp2t") {
            video += len as u64;
            if parts % 50 == 0 {
                println!(
                    "  video {} KB after {:.1} s",
                    video / 1000,
                    started.elapsed().as_secs_f32()
                );
            }
        } else {
            println!("  {ct}: {}", String::from_utf8_lossy(&payload));
        }
    }
    println!("total video {} KB in {} parts", video / 1000, parts);
    // Free the camera's slot.
    if let Some(stop) = args.get(5) {
        let tls = r.get_mut();
        let _ = tls.write_all(&control_frame(stop));
        let _ = tls.flush();
        std::thread::sleep(Duration::from_secs(1));
    }
}
