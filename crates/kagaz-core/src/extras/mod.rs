//! Heavy optional pieces that never ship in the base app: downloaded on
//! request, pinned by checksum, kept in the extras folder.

pub mod ffmpeg;
pub mod go2rtc;

use std::io::Read;
use std::path::Path;

/// Extract the first zip member whose name satisfies `wanted` to `dest`
/// (stored or deflated members; a minimal reader that walks local headers).
pub(crate) fn unzip_member(
    zip: &[u8],
    wanted: &dyn Fn(&str) -> bool,
    dest: &Path,
) -> Result<(), go2rtc::ExtraError> {
    use go2rtc::ExtraError;
    let mut pos = 0;
    while pos + 30 <= zip.len() && &zip[pos..pos + 4] == b"PK\x03\x04" {
        let flags = u16::from_le_bytes([zip[pos + 6], zip[pos + 7]]);
        let method = u16::from_le_bytes([zip[pos + 8], zip[pos + 9]]);
        let csize = u32::from_le_bytes([zip[pos + 18], zip[pos + 19], zip[pos + 20], zip[pos + 21]])
            as usize;
        let name_len = u16::from_le_bytes([zip[pos + 26], zip[pos + 27]]) as usize;
        let extra_len = u16::from_le_bytes([zip[pos + 28], zip[pos + 29]]) as usize;
        let name = String::from_utf8_lossy(&zip[pos + 30..pos + 30 + name_len]).to_string();
        let data_start = pos + 30 + name_len + extra_len;
        if flags & 0x8 != 0 && csize == 0 {
            // Sizes only in a trailing descriptor: fall back to the central directory.
            return unzip_member_central(zip, wanted, dest);
        }
        let data = &zip[data_start..(data_start + csize).min(zip.len())];
        if wanted(&name) {
            let bytes = inflate_member(method, data)?;
            return std::fs::write(dest, bytes).map_err(|source| ExtraError::Io {
                path: dest.to_path_buf(),
                source,
            });
        }
        pos = data_start + csize;
    }
    Err(ExtraError::Download("zip without the wanted file".into()))
}

fn inflate_member(method: u16, data: &[u8]) -> Result<Vec<u8>, go2rtc::ExtraError> {
    use go2rtc::ExtraError;
    match method {
        0 => Ok(data.to_vec()),
        8 => {
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(data)
                .read_to_end(&mut out)
                .map_err(|e| ExtraError::Download(format!("zip: {e}")))?;
            Ok(out)
        }
        m => Err(ExtraError::Download(format!(
            "zip method {m} not supported"
        ))),
    }
}

/// The same through the central directory (needed when local headers carry no sizes).
fn unzip_member_central(
    zip: &[u8],
    wanted: &dyn Fn(&str) -> bool,
    dest: &Path,
) -> Result<(), go2rtc::ExtraError> {
    use go2rtc::ExtraError;
    let eocd = zip
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .ok_or_else(|| ExtraError::Download("zip without a central directory".into()))?;
    let cd_start = u32::from_le_bytes([
        zip[eocd + 16],
        zip[eocd + 17],
        zip[eocd + 18],
        zip[eocd + 19],
    ]) as usize;
    let mut pos = cd_start;
    while pos + 46 <= zip.len() && &zip[pos..pos + 4] == b"PK\x01\x02" {
        let method = u16::from_le_bytes([zip[pos + 10], zip[pos + 11]]);
        let csize = u32::from_le_bytes([zip[pos + 20], zip[pos + 21], zip[pos + 22], zip[pos + 23]])
            as usize;
        let name_len = u16::from_le_bytes([zip[pos + 28], zip[pos + 29]]) as usize;
        let extra_len = u16::from_le_bytes([zip[pos + 30], zip[pos + 31]]) as usize;
        let comment_len = u16::from_le_bytes([zip[pos + 32], zip[pos + 33]]) as usize;
        let local = u32::from_le_bytes([zip[pos + 42], zip[pos + 43], zip[pos + 44], zip[pos + 45]])
            as usize;
        let name = String::from_utf8_lossy(&zip[pos + 46..pos + 46 + name_len]).to_string();
        if wanted(&name) && local + 30 <= zip.len() {
            let lname = u16::from_le_bytes([zip[local + 26], zip[local + 27]]) as usize;
            let lextra = u16::from_le_bytes([zip[local + 28], zip[local + 29]]) as usize;
            let start = local + 30 + lname + lextra;
            let data = &zip[start..(start + csize).min(zip.len())];
            let bytes = inflate_member(method, data)?;
            return std::fs::write(dest, bytes).map_err(|source| ExtraError::Io {
                path: dest.to_path_buf(),
                source,
            });
        }
        pos += 46 + name_len + extra_len + comment_len;
    }
    Err(ExtraError::Download("zip without the wanted file".into()))
}
