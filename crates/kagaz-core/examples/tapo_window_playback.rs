//! Diagnostic: the window's playback route without the window. Starts the
//! stream server and the engine, registers a paced playback of one camera's
//! footage and reads it through the per-tile proxy, printing progress.
//!
//! `cargo run -p kagaz-core --example tapo_window_playback -- <device-id> <from-unix> <to-unix> [seconds]`
use kagaz_core::cameras::tapo::{Session, TsServer};
use kagaz_core::extras::go2rtc;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::load(&Session::default_path()).expect("session");
    let cams = session.cameras().expect("cameras");
    let cam = cams
        .iter()
        .find(|c| c.device_id == args[1])
        .expect("camera")
        .clone();
    let from: i64 = args[2].parse().unwrap();
    let to: i64 = args[3].parse().unwrap();
    let secs: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
    let server = TsServer::start(session, cams.clone(), 0).unwrap();
    let binary = go2rtc::ensure(&mut |_| {}).unwrap();
    let engine = go2rtc::Engine::start(&binary, 0, &[]).unwrap();
    server.set_engine_port(engine.api_port);
    let (token, url) = server.playback_url(&cam.device_id, from, to, false);
    let stream = format!("playback-{token}");
    engine.add_stream(&stream, &url).unwrap();
    let tile = server.tile_url(0, &stream);
    println!("tile url shape: {}", tile.replace(&token, "<token>"));
    let (host, rest) = tile
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let mut sock = std::net::TcpStream::connect(host).unwrap();
    sock.write_all(format!("GET /{rest} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let started = Instant::now();
    let mut buf = vec![0u8; 65536];
    let mut total = 0u64;
    let mut head_shown = false;
    while started.elapsed() < Duration::from_secs(secs) {
        match sock.read(&mut buf) {
            Ok(0) => {
                println!("closed");
                break;
            }
            Ok(n) => {
                if !head_shown {
                    let h = String::from_utf8_lossy(&buf[..n.min(200)]);
                    println!("head: {}", h.lines().next().unwrap_or(""));
                    head_shown = true;
                }
                total += n as u64;
            }
            Err(e) => {
                println!("read: {e}");
                break;
            }
        }
        if started.elapsed().as_secs().is_multiple_of(5) {
            if let Some(st) = server.playback_state(&token) {
                println!("  {:.0} s: proxied {} KB, server state started={} finished={} bytes={} err={:?}", started.elapsed().as_secs_f32(), total / 1000, st.started, st.finished, st.bytes, st.error);
                std::thread::sleep(Duration::from_millis(1000));
            }
        }
    }
    println!("total proxied {} KB", total / 1000);
    drop(sock);
    engine.remove_stream(&stream);
    std::thread::sleep(Duration::from_secs(2));
    println!(
        "after stop: {:?}",
        server
            .playback_state(&token)
            .map(|s| (s.finished, s.bytes, s.error))
    );
}
