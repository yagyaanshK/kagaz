//! Reading a JPEG's size without decoding it, so pages that need no
//! conversion go into the PDF untouched.

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
