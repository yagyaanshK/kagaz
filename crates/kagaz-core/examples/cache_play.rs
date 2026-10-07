//! Diagnostic: play part of a recorded MPEG-TS file the way the stream
//! server plays cached footage, and report where it started, how much video
//! it sent and how long that took.
//!
//! `cargo run -p kagaz-core --example cache_play -- <file.ts> <file-start-unix> <from-unix> <to-unix> [paced]`
use kagaz_core::cameras::tapo::cache::{play_file, Entry};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let start: i64 = a[2].parse().unwrap();
    let entry = Entry {
        device_id: "test".into(),
        start,
        end: start + 3600,
        exact_start: true,
        seconds: 3600.0,
        bytes: 0,
        complete: true,
        fetched_at: 0,
        path: a[1].clone().into(),
    };
    let (from, to): (i64, i64) = (a[3].parse().unwrap(), a[4].parse().unwrap());
    let pace = a.get(5).map(|_| 1.0);
    let mut pts = kagaz_core::cameras::tapo::relay::PtsTracker::default();
    let mut first: Option<std::time::Duration> = None;
    let began = std::time::Instant::now();
    let sent = play_file(&entry, from, to, pace, &mut |chunk| {
        first.get_or_insert(began.elapsed());
        pts.update(chunk);
        true
    })
    .unwrap();
    println!(
        "sent {} KB, {:.1} s of video, in {:.1} s",
        sent / 1000,
        pts.seconds(),
        began.elapsed().as_secs_f32()
    );
}
