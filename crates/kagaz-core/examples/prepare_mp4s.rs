//! Make the playable MP4 beside every cached recording that lacks one (the
//! window does this on first play; this does it ahead of time).
//!
//! `cargo run --release -p kagaz-core --example prepare_mp4s`
use kagaz_core::cameras::tapo::cache;
fn main() {
    let root = kagaz_core::paths::recordings_dir();
    let ffmpeg = kagaz_core::extras::ffmpeg::ensure(&mut |_| {}).expect("ffmpeg");
    let mut made = 0;
    for dev in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        let id = dev.file_name().to_string_lossy().to_string();
        for e in cache::list(&root, &id) {
            if cache::mp4_path(&e).is_file() {
                continue;
            }
            let t = std::time::Instant::now();
            match cache::ensure_mp4(&e, &ffmpeg) {
                Ok(_) => {
                    made += 1;
                    println!(
                        "{} {}: {:.0} s",
                        &id[..8],
                        e.start,
                        t.elapsed().as_secs_f32()
                    );
                }
                Err(err) => println!("{} {}: {err}", &id[..8], e.start),
            }
        }
    }
    println!("made {made}");
}
