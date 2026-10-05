//! Pulling pages with a vendor's own tool, for the cases the vendor's
//! protocol requires it: Brother's Scan button hands the job to the
//! computer, which must fetch the pages through Brother's `skey-scanimage`.

use super::{ColorMode, Page, ScanError};
use image::codecs::jpeg::JpegEncoder;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Brother's pull tool, installed by the brscan-skey package.
pub const BROTHER_SKEY_SCANIMAGE: &str = "/opt/brother/scanner/brscan-skey/skey-scanimage";

/// Brother's own settings for button scans, read the way their
/// `scantofile.sh` reads them: the user's file first, then the system one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrotherSettings {
    pub resolution: u32,
    pub duplex: bool,
    pub size: String,
}

impl Default for BrotherSettings {
    fn default() -> Self {
        Self {
            resolution: 100,
            duplex: false,
            size: "A4".into(),
        }
    }
}

pub fn brother_settings() -> BrotherSettings {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let candidates = [
        home.map(|h| h.join(".brscan-skey/scantofile.config")),
        Some(PathBuf::from(
            "/etc/opt/brother/scanner/brscan-skey/scantofile.config",
        )),
    ];
    for path in candidates.into_iter().flatten() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            return parse_brother_settings(&text);
        }
    }
    BrotherSettings::default()
}

pub fn parse_brother_settings(text: &str) -> BrotherSettings {
    let mut s = BrotherSettings::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "resolution" => {
                if let Ok(r) = v.parse() {
                    s.resolution = r;
                }
            }
            "duplex" => s.duplex = v.eq_ignore_ascii_case("ON"),
            "size" if !v.is_empty() => s.size = v.to_string(),
            _ => {}
        }
    }
    s
}

/// Fetch what the Brother is holding after its Scan button, exactly as
/// Brother's own `scantofile.sh` does: `skey-scanimage` with the settings
/// file's resolution and size, the flatbed source (or the feeder, both
/// sides, when duplex is on), a one-second wait for network devices, and
/// one retry. `device` is the SANE name the listener passes, e.g.
/// `brother4:net1;dev0`. Returns the TIFF files written into `dir`.
pub fn brother_pull(
    device: &str,
    settings: &BrotherSettings,
    dir: &Path,
) -> Result<Vec<PathBuf>, ScanError> {
    if !Path::new(BROTHER_SKEY_SCANIMAGE).is_file() {
        return Err(ScanError::NotSupported(
            "Brother's scan-key tool is not installed (kagaz driver installs it)".into(),
        ));
    }
    let out = dir.join("brscan.tif");
    let mut args: Vec<String> = vec![
        "--device-name".into(),
        device.into(),
        "--resolution".into(),
        settings.resolution.to_string(),
    ];
    if settings.duplex {
        args.extend(["--source".into(), "ADF_C".into()]);
    } else {
        args.extend(["--source".into(), "FB".into()]);
    }
    args.extend(["--size".into(), settings.size.clone()]);
    if settings.duplex {
        args.push("--duplex".into());
    }
    args.extend(["--outputfile".into(), out.display().to_string()]);
    if device.contains("net") {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let mut last_err = String::new();
    for attempt in 0..2 {
        if attempt == 1 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let output = Command::new(BROTHER_SKEY_SCANIMAGE)
            .args(&args)
            .output()
            .map_err(|e| ScanError::Transport(format!("cannot run skey-scanimage: {e}")))?;
        last_err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let files = tiffs_in(dir);
        if !files.is_empty() {
            return Ok(files);
        }
    }
    Err(ScanError::Refused(if last_err.is_empty() {
        "skey-scanimage delivered no pages".into()
    } else {
        format!("skey-scanimage delivered no pages: {last_err}")
    }))
}

fn tiffs_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                        e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff")
                    }) && std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Turn image files (TIFF from a vendor tool, or anything the image crate
/// reads) into pages for the output stage, as JPEG.
pub fn pages_from_files(files: &[PathBuf], dpi: u32) -> Result<Vec<Page>, ScanError> {
    let mut pages = Vec::with_capacity(files.len());
    for f in files {
        let img =
            image::open(f).map_err(|e| ScanError::Protocol(format!("{}: {e}", f.display())))?;
        let (data, color) = match img {
            image::DynamicImage::ImageLuma8(g) => {
                let mut buf = Cursor::new(Vec::new());
                JpegEncoder::new_with_quality(&mut buf, 92)
                    .encode_image(&g)
                    .map_err(|e| ScanError::Protocol(e.to_string()))?;
                (buf.into_inner(), ColorMode::Gray)
            }
            other => {
                let rgb = other.to_rgb8();
                let mut buf = Cursor::new(Vec::new());
                JpegEncoder::new_with_quality(&mut buf, 92)
                    .encode_image(&rgb)
                    .map_err(|e| ScanError::Protocol(e.to_string()))?;
                (buf.into_inner(), ColorMode::Color)
            }
        };
        pages.push(Page {
            data,
            mime: "image/jpeg",
            dpi,
            color,
        });
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brother_settings_follow_their_file() {
        let s = parse_brother_settings("#[brscan-skey]\n# resolution=100,150\nresolution=300\n\n#duplex=OFF,ON\nduplex=ON\nsize=Letter\n");
        assert_eq!(
            s,
            BrotherSettings {
                resolution: 300,
                duplex: true,
                size: "Letter".into()
            }
        );
        assert_eq!(parse_brother_settings(""), BrotherSettings::default());
    }

    #[test]
    fn tiff_pages_become_jpeg_pages() {
        let dir = std::env::temp_dir().join(format!("kagaz-vendor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = image::RgbImage::from_pixel(40, 30, image::Rgb([200, 100, 50]));
        img.save(dir.join("page_ADF.tif")).unwrap();
        let grey = image::GrayImage::from_pixel(20, 10, image::Luma([90]));
        grey.save(dir.join("page_ADF_2.tif")).unwrap();
        std::fs::write(dir.join("empty.tif"), b"").unwrap();
        let files = tiffs_in(&dir);
        assert_eq!(files.len(), 2);
        let pages = pages_from_files(&files, 300).unwrap();
        assert_eq!(pages[0].color, ColorMode::Color);
        assert_eq!(pages[1].color, ColorMode::Gray);
        assert_eq!(
            crate::output::jpeg::dimensions(&pages[0].data),
            Some((40, 30, 3))
        );
        assert_eq!(
            crate::output::jpeg::dimensions(&pages[1].data),
            Some((20, 10, 1))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
