//! Diagnostic: put ffmpeg in place from an archive already on disk (the
//! same extraction the extra does after downloading), e.g. to test a new
//! build before pinning it.
//!
//! `cargo run -p kagaz-core --example ffmpeg_extract -- <archive.tar.xz> <member path inside it>`
use kagaz_core::extras::ffmpeg;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).expect("archive");
    let dest = ffmpeg::binary_path();
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    let started = std::time::Instant::now();
    ffmpeg::untar_xz_member(&bytes, &args[2], &dest).expect("extract");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    println!(
        "{} ready in {:.1} s",
        dest.display(),
        started.elapsed().as_secs_f32()
    );
}
