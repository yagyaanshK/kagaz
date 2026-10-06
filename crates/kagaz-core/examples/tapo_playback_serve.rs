//! Diagnostic: serve one camera's recorded span through the engine and the
//! per-tile proxy for `seconds`, printing the tile URL first, so another
//! program (a webview probe) can play it.
//!
//! `cargo run -p kagaz-core --example tapo_playback_serve -- <device-id> <from-unix> <to-unix> <seconds> [fast|paced] [speed]`
use kagaz_core::cameras::tapo::{Session, TsServer};
use kagaz_core::extras::go2rtc;
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
    let secs: u64 = args[4].parse().unwrap();
    let fast = args.get(5).is_some_and(|f| f == "fast");
    let speed: f64 = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let server = TsServer::start(session, cams.clone(), 0).unwrap();
    let binary = go2rtc::ensure(&mut |_| {}).unwrap();
    let engine = go2rtc::Engine::start(&binary, 0, &[]).unwrap();
    server.set_engine_port(engine.api_port);
    let (token, url) = server.playback_url_at(&cam.device_id, from, to, fast, speed);
    let stream = format!("playback-{token}");
    engine.add_stream(&stream, &url).unwrap();
    println!("{}", server.tile_url(0, &stream));
    std::thread::sleep(std::time::Duration::from_secs(secs));
    engine.remove_stream(&stream);
    println!(
        "state: {:?}",
        server
            .playback_state(&token)
            .map(|s| (s.finished, s.bytes, s.error))
    );
}
