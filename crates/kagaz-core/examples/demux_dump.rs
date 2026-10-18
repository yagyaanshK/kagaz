//! Take a recorded Tapo MPEG-TS apart with the window's demuxer and write
//! the first keyframe as Annex-B, for trying decoders on it:
//! `cargo run -p kagaz-core --example demux_dump -- in.ts out.h264`

use kagaz_core::cameras::tapo::demux::Demux;
use kagaz_core::media::{nal_type, nal_units, Media};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, output) = (&args[1], &args[2]);
    let bytes = std::fs::read(input).expect("read the TS");
    let mut demux = Demux::new();
    let mut written = false;
    let mut pictures = 0;
    let mut audio = 0;
    for chunk in bytes.chunks(64 * 1024) {
        for m in demux.push(chunk) {
            match m {
                Media::Video { data, key, timestamp } => {
                    pictures += 1;
                    if key && !written {
                        let types: Vec<u8> = nal_units(&data).iter().map(|u| nal_type(u)).collect();
                        let sps = nal_units(&data)
                            .into_iter()
                            .find(|u| nal_type(u) == 7)
                            .map(|s| format!("avc1.{:02X}{:02X}{:02X}", s[1], s[2], s[3]))
                            .unwrap_or_default();
                        println!(
                            "first keyframe: {} bytes, pts {timestamp}, NAL types {types:?}, codec {sps}",
                            data.len()
                        );
                        std::fs::write(output, &data).expect("write");
                        written = true;
                    }
                }
                Media::Audio { pcm, .. } => audio += pcm.len(),
                Media::Ended(_) => {}
            }
        }
    }
    let s = &demux.stats;
    println!(
        "{pictures} pictures ({} key, {} held), {audio} sound samples, {} packets",
        s.keyframes, s.waiting, s.packets
    );
}
