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

/// Fetch whatever the Brother is holding for `device` (a SANE name such as
/// `brother4:net1;dev0`): the feeder when it has paper, else the glass.
/// Returns the TIFF files it wrote into `dir`, in page order.
pub fn brother_pull(device: &str, dpi: u32, dir: &Path) -> Result<Vec<PathBuf>, ScanError> {
    if !Path::new(BROTHER_SKEY_SCANIMAGE).is_file() {
        return Err(ScanError::NotSupported(
            "Brother's scan-key tool is not installed (kagaz driver installs it)".into(),
        ));
    }
    for source in ["ADF", "FB"] {
        let out = dir.join(format!("page_{source}.tif"));
        let status = Command::new(BROTHER_SKEY_SCANIMAGE)
            .args([
                "--device-name",
                device,
                "--resolution",
                &dpi.to_string(),
                "--size",
                "A4",
                "--source",
                source,
                "--outputfile",
            ])
            .arg(&out)
            .output()
            .map_err(|e| ScanError::Transport(format!("cannot run skey-scanimage: {e}")))?;
        let files = tiffs_in(dir);
        if !files.is_empty() {
            return Ok(files);
        }
        if !status.status.success() && source == "FB" {
            return Err(ScanError::Refused(format!(
                "skey-scanimage gave nothing: {}",
                String::from_utf8_lossy(&status.stderr).trim()
            )));
        }
    }
    Err(ScanError::Refused("the scanner delivered no pages".into()))
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
