//! Diagnostic: the window's sound route without the window. Starts the
//! stream server and the engine, opens one camera's live stream through the
//! per-tile proxy (as a tile does) and, alongside, its `/audio/<id>.wav`;
//! prints how much video and sound arrived.
//!
//! `cargo run -p kagaz-core --example tapo_audio_route -- <device-id> [seconds]`
use kagaz_core::cameras::tapo::{Session, TsServer};
use kagaz_core::extras::go2rtc;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn fetch(url: &str, secs: u64, label: &str) -> std::thread::JoinHandle<(u64, String)> {
    let url = url.to_string();
    let label = label.to_string();
    std::thread::spawn(move || {
        let (host, rest) = url
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
        let mut head = String::new();
        while started.elapsed() < Duration::from_secs(secs) {
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if head.is_empty() {
                        head = String::from_utf8_lossy(&buf[..n.min(400)])
                            .lines()
                            .take(1)
                            .collect();
                        println!(
                            "{label}: {head} after {:.1} s",
                            started.elapsed().as_secs_f32()
                        );
                    }
                    total += n as u64;
                }
                Err(e) => {
                    println!("{label}: read {e}");
                    break;
                }
            }
        }
        (total, head)
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::load(&Session::default_path()).expect("session");
    let cams = session.cameras().expect("cameras");
    let cam = cams
        .iter()
        .find(|c| c.device_id == args[1])
        .expect("camera")
        .clone();
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let server = TsServer::start(session, cams.clone(), 0).unwrap();
    let binary = go2rtc::ensure(&mut |_| {}).unwrap();
    let streams = vec![go2rtc::StreamSource {
        name: cam.name.clone(),
        url: server.url_for(&cam.device_id),
    }];
    let engine = go2rtc::Engine::start(&binary, 0, &streams).unwrap();
    server.set_engine_port(engine.api_port);
    let tile = server.tile_url(0, &go2rtc::yaml_key(&cam.name));
    let audio = format!(
        "http://{}:{}/audio/{}.wav",
        server.host_for(0),
        server.port,
        cam.device_id
    );
    let v = fetch(&tile, secs, "video");
    std::thread::sleep(Duration::from_secs(3));
    let a = fetch(&audio, secs - 3, "audio");
    let (vb, _) = v.join().unwrap();
    let (ab, _) = a.join().unwrap();
    println!(
        "video {} KB, audio {} KB = {:.1} s of 8 kHz 16-bit sound",
        vb / 1000,
        ab / 1000,
        ab.saturating_sub(44) as f64 / 16000.0
    );
}
