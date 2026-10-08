//! Diagnostic: prepare the MP4 of the cached clip holding a moment and
//! serve it from the stream server for `seconds`, printing its URL and the
//! clip's first-frame time.
//!
//! `cargo run -p kagaz-core --example clip_serve -- <device-id> <at-unix> <seconds>`
use kagaz_core::cameras::tapo::{cache, Session, TsServer};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let at: i64 = a[2].parse().unwrap();
    let secs: u64 = a[3].parse().unwrap();
    let entry = cache::find(&kagaz_core::paths::recordings_dir(), &a[1], at).expect("on disk");
    let ffmpeg = kagaz_core::extras::ffmpeg::ensure(&mut |_| {}).unwrap();
    let t = std::time::Instant::now();
    cache::ensure_mp4(&entry, &ffmpeg).unwrap();
    eprintln!("mp4 ready in {:.1} s", t.elapsed().as_secs_f32());
    let session = Session::load(&Session::default_path()).expect("session");
    let server = TsServer::start(session, Vec::new(), 0).unwrap();
    println!("{} {}", server.clip_url(0, &entry).unwrap(), entry.start);
    std::thread::sleep(std::time::Duration::from_secs(secs));
}
