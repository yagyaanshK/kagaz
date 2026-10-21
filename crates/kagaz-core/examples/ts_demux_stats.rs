//! Split an MPEG-TS file the way the window's own player does and print
//! what came out: `cargo run -p kagaz-core --example ts_demux_stats -- file.ts`.
use kagaz_core::cameras::tapo::demux::Demux;
use kagaz_core::media::Media;

fn main() {
    let path = std::env::args().nth(1).expect("a .ts file");
    let bytes = std::fs::read(&path).expect("read");
    let mut demux = Demux::new();
    let (mut pictures, mut keys, mut sound) = (0, 0, 0);
    for chunk in bytes.chunks(7 * 188) {
        for m in demux.push(chunk) {
            match m {
                Media::Video { key, .. } => {
                    pictures += 1;
                    keys += key as u32;
                }
                Media::Audio { .. } => sound += 1,
                _ => {}
            }
        }
    }
    println!("{pictures} pictures ({keys} key), {sound} sound pieces");
}
