//! Reading a JPEG's size without decoding it, so pages that need no
//! conversion go into the PDF untouched; and marking its resolution.

/// (width, height, components) from the start-of-frame marker.
/// Components is 1 for greyscale, 3 for colour (YCbCr/RGB), 4 for CMYK.
pub fn dimensions(data: &[u8]) -> Option<(u32, u32, u8)> {
    if data.len() < 4 || data[0] != 0xff || data[1] != 0xd8 {
        return None;
    }
    let mut pos = 2;
    while pos + 4 <= data.len() {
        if data[pos] != 0xff {
            return None;
        }
        let marker = data[pos + 1];
        pos += 2;
        match marker {
            0xff => {
                pos -= 1; // fill byte
                continue;
            }
            0xd8 | 0x01 | 0xd0..=0xd7 => continue, // standalone markers
            0xd9 | 0xda => return None,            // end of image / scan data before SOF
            _ => {}
        }
        let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        let is_sof = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if is_sof {
            if pos + 8 > data.len() {
                return None;
            }
            let height = u16::from_be_bytes([data[pos + 3], data[pos + 4]]) as u32;
            let width = u16::from_be_bytes([data[pos + 5], data[pos + 6]]) as u32;
            let components = data[pos + 7];
            return Some((width, height, components));
        }
        pos += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sof0_after_app0() {
        let mut jpeg = vec![0xff, 0xd8];
        jpeg.extend_from_slice(&[0xff, 0xe0, 0x00, 0x04, 0x4a, 0x46]); // APP0, 4 bytes
        jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08, 0x01, 0x2c, 0x02, 0x58, 0x03]); // SOF0 300x600, 3 comps
        assert_eq!(dimensions(&jpeg), Some((600, 300, 3)));
    }

    #[test]
    fn rejects_non_jpeg_and_truncated() {
        assert_eq!(dimensions(b"\x89PNG\r\n"), None);
        assert_eq!(dimensions(&[0xff, 0xd8, 0xff, 0xc0, 0x00]), None);
        assert_eq!(dimensions(&[0xff, 0xd8, 0xff, 0xd9]), None);
    }
}

/// The JPEG with its resolution recorded (the JFIF header's dots per inch),
/// so a viewer or a print dialog shows and prints it at its real size. The
/// image data itself is not touched.
pub fn with_dpi(mut data: Vec<u8>, dpi: u32) -> Vec<u8> {
    if data.len() < 4 || data[0] != 0xff || data[1] != 0xd8 {
        return data;
    }
    let dpi = dpi.clamp(1, u16::MAX as u32) as u16;
    let d = dpi.to_be_bytes();
    if data.len() >= 18 && data[2..4] == [0xff, 0xe0] && &data[6..11] == b"JFIF\0" {
        data[13] = 1; // dots per inch
        data[14..16].copy_from_slice(&d);
        data[16..18].copy_from_slice(&d);
        return data;
    }
    // No JFIF header: add one right after the start-of-image marker.
    let app0 = [
        0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 1, d[0], d[1], d[0], d[1], 0, 0,
    ];
    data.splice(2..2, app0);
    data
}

#[cfg(test)]
mod dpi_tests {
    use super::*;

    #[test]
    fn resolution_is_written_into_the_header() {
        let img = image::RgbImage::new(4, 4);
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&img).unwrap();
        let marked = with_dpi(jpeg.clone(), 1200);
        assert_eq!(&marked[6..11], b"JFIF\0");
        assert_eq!(marked[13], 1);
        assert_eq!(u16::from_be_bytes([marked[14], marked[15]]), 1200);
        assert_eq!(dimensions(&marked), dimensions(&jpeg));
        image::load_from_memory(&marked).unwrap();

        // A JPEG with no JFIF header gets one.
        let bare: Vec<u8> = [&jpeg[..2], &jpeg[2 + 4 + u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize - 2..]].concat();
        let marked = with_dpi(bare, 300);
        assert_eq!(u16::from_be_bytes([marked[16], marked[17]]), 300);
        image::load_from_memory(&marked).unwrap();
    }
}
