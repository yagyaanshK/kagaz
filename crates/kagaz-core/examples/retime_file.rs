//! Diagnostic: rewrite an MPEG-TS file's timestamps for a playback speed,
//! the way the stream server does for sped-up playback.
//!
//! `cargo run -p kagaz-core --example retime_file -- <in.ts> <out.ts> <speed>`
use kagaz_core::cameras::tapo::retime::Retimer;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&a[1]).expect("input");
    let mut r = Retimer::new(a[3].parse().expect("speed"));
    std::fs::write(&a[2], r.push(&data)).expect("output");
}
