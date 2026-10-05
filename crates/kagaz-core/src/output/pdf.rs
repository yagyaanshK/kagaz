//! A minimal PDF writer: one image per page, nothing else. JPEG pages are
//! embedded as they are (DCTDecode), so a scan is not re-compressed on its
//! way into the PDF; black-and-white pages go in as 1-bit images (FlateDecode).

use std::io::Write;

pub enum Image<'a> {
    /// A JPEG as delivered; `components` is 1 (grey) or 3 (colour).
    Jpeg { data: &'a [u8], components: u8 },
    /// 1 bit per pixel, rows padded to a byte, 1 = white.
    Bilevel { packed: Vec<u8> },
}

pub struct PdfPage<'a> {
    pub width_px: u32,
    pub height_px: u32,
    pub dpi: u32,
    pub image: Image<'a>,
}

/// Serialise `pages` into a complete PDF 1.4 file.
pub fn write(pages: &[PdfPage]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");

    // Object numbers: 1 catalog, 2 pages, 3 info, then 3 per page.
    let page_obj = |i: usize| 4 + i * 3;
    let content_obj = |i: usize| 5 + i * 3;
    let image_obj = |i: usize| 6 + i * 3;

    let begin = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, num: usize| {
        offsets.push(out.len());
        let _ = writeln!(out, "{num} 0 obj");
    };

    begin(&mut out, &mut offsets, 1);
    out.extend_from_slice(b"<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    begin(&mut out, &mut offsets, 2);
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", page_obj(i)))
        .collect();
    let _ = write!(
        out,
        "<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
        kids.join(" "),
        pages.len()
    );

    begin(&mut out, &mut offsets, 3);
    out.extend_from_slice(b"<< /Producer (Kagaz) /Creator (Kagaz) >>\nendobj\n");

    for (i, page) in pages.iter().enumerate() {
        let dpi = page.dpi.max(1) as f64;
        let width_pt = page.width_px as f64 * 72.0 / dpi;
        let height_pt = page.height_px as f64 * 72.0 / dpi;

        begin(&mut out, &mut offsets, page_obj(i));
        let _ = write!(
            out,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width_pt:.2} {height_pt:.2}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>\nendobj\n",
            image_obj(i),
            content_obj(i)
        );

        let content = format!("q {width_pt:.2} 0 0 {height_pt:.2} 0 0 cm /Im0 Do Q");
        begin(&mut out, &mut offsets, content_obj(i));
        let _ = write!(out, "<< /Length {} >>\nstream\n", content.len());
        out.extend_from_slice(content.as_bytes());
        out.extend_from_slice(b"\nendstream\nendobj\n");

        begin(&mut out, &mut offsets, image_obj(i));
        match &page.image {
            Image::Jpeg { data, components } => {
                let cs = if *components == 1 {
                    "/DeviceGray"
                } else {
                    "/DeviceRGB"
                };
                let _ = write!(
                    out,
                    "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {cs} /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                    page.width_px,
                    page.height_px,
                    data.len()
                );
                out.extend_from_slice(data);
            }
            Image::Bilevel { packed } => {
                let compressed = deflate(packed);
                let _ = write!(
                    out,
                    "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode /Length {} >>\nstream\n",
                    page.width_px,
                    page.height_px,
                    compressed.len()
                );
                out.extend_from_slice(&compressed);
            }
        }
        out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for off in &offsets {
        let _ = writeln!(out, "{off:010} 00000 n ");
    }
    let _ = write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        offsets.len() + 1
    );
    out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use flate2::{write::ZlibEncoder, Compression};
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
    let _ = enc.write_all(data);
    enc.finish().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xref_offsets_point_at_objects() {
        let jpeg = b"\xff\xd8fakejpeg\xff\xd9";
        let pages = [
            PdfPage {
                width_px: 300,
                height_px: 600,
                dpi: 300,
                image: Image::Jpeg {
                    data: jpeg,
                    components: 3,
                },
            },
            PdfPage {
                width_px: 8,
                height_px: 2,
                dpi: 100,
                image: Image::Bilevel {
                    packed: vec![0xff, 0x00],
                },
            },
        ];
        let pdf = write(&pages);
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count 2"));
        assert!(text.contains("/MediaBox [0 0 72.00 144.00]"));
        assert!(text.contains("/MediaBox [0 0 5.76 1.44]"));
        assert!(text.contains("/Filter /DCTDecode /Length 12"));
        assert!(text.contains("/BitsPerComponent 1 /Filter /FlateDecode"));

        // Every xref entry must land on "N 0 obj". Work on bytes: the file
        // holds binary streams, so text offsets would drift.
        let find = |needle: &[u8]| {
            pdf.windows(needle.len())
                .rposition(|w| w == needle)
                .unwrap()
        };
        let xref_pos = find(b"\nxref\n") + 1;
        let table = String::from_utf8_lossy(&pdf[xref_pos..]).to_string();
        let entries: Vec<&str> = table
            .lines()
            .skip(3)
            .take_while(|l| l.ends_with(" n "))
            .collect();
        assert_eq!(entries.len(), 9);
        for (i, e) in entries.iter().enumerate() {
            let off: usize = e[..10].parse().unwrap();
            assert!(
                pdf[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()),
                "object {} at {off}",
                i + 1
            );
        }
        let startxref: usize = table
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(startxref, xref_pos);
    }
}
