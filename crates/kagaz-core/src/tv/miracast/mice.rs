//! Miracast over the ordinary network: Microsoft's MS-MICE (Miracast over
//! Infrastructure Connection Establishment), a published specification.
//!
//! The source opens TCP port 7250 on the TV and says it is ready, naming
//! the port its RTSP server listens on; the TV connects back to that port
//! and the session goes on as over Wi-Fi Direct. Every number is
//! big-endian.

/// The TV's port for these messages.
pub const PORT: u16 = 7250;

const VERSION: u8 = 0x01;
const SOURCE_READY: u8 = 0x01;
const STOP_PROJECTION: u8 = 0x02;

const FRIENDLY_NAME: u8 = 0x00;
const RTSP_PORT: u8 = 0x02;
const SOURCE_ID: u8 = 0x03;

fn tlv(out: &mut Vec<u8>, kind: u8, value: &[u8]) {
    out.push(kind);
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value);
}

fn message(command: u8, tlvs: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(4 + tlvs.len());
    m.extend_from_slice(&((4 + tlvs.len()) as u16).to_be_bytes());
    m.push(VERSION);
    m.push(command);
    m.extend_from_slice(tlvs);
    m
}

/// The name as the TV shows it: UTF-16 (little-endian, as Windows sends
/// it), at most 520 bytes.
fn name_value(name: &str) -> Vec<u8> {
    let mut v: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    v.truncate(520);
    v
}

/// "Ready: connect to my RTSP port."
pub fn source_ready(name: &str, rtsp_port: u16, source_id: &[u8; 16]) -> Vec<u8> {
    let mut t = Vec::new();
    tlv(&mut t, FRIENDLY_NAME, &name_value(name));
    tlv(&mut t, RTSP_PORT, &rtsp_port.to_be_bytes());
    tlv(&mut t, SOURCE_ID, source_id);
    message(SOURCE_READY, &t)
}

/// "The projection has ended."
pub fn stop_projection(name: &str, source_id: &[u8; 16]) -> Vec<u8> {
    let mut t = Vec::new();
    tlv(&mut t, FRIENDLY_NAME, &name_value(name));
    tlv(&mut t, SOURCE_ID, source_id);
    message(STOP_PROJECTION, &t)
}

/// A message as the TV reads it: (command, the source's name, its RTSP
/// port), or `None` when it is not one. 1 is "ready", 2 "stopped".
pub fn read(m: &[u8]) -> Option<(u8, String, Option<u16>)> {
    let size = u16::from_be_bytes([*m.first()?, *m.get(1)?]) as usize;
    if size < 4 || m.len() < size || m[2] != VERSION {
        return None;
    }
    let (mut name, mut port) = (String::new(), None);
    let mut at = 4;
    while at + 3 <= size {
        let kind = m[at];
        let len = u16::from_be_bytes([m[at + 1], m[at + 2]]) as usize;
        let value = m.get(at + 3..at + 3 + len)?;
        match kind {
            FRIENDLY_NAME => {
                let units: Vec<u16> = value.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                name = String::from_utf16_lossy(&units);
            }
            RTSP_PORT if len == 2 => port = Some(u16::from_be_bytes([value[0], value[1]])),
            _ => {}
        }
        at += 3 + len;
    }
    Some((m[3], name, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ready_layout() {
        let id = [7u8; 16];
        let m = source_ready("Ab", 7236, &id);
        // size, version, command
        assert_eq!(&m[..4], &[0x00, 0x23, 0x01, 0x01]);
        // friendly name: type 0, length 4, "Ab" in UTF-16LE
        assert_eq!(&m[4..11], &[0x00, 0x00, 0x04, b'A', 0, b'b', 0]);
        // RTSP port 7236 = 0x1c44
        assert_eq!(&m[11..16], &[0x02, 0x00, 0x02, 0x1c, 0x44]);
        assert_eq!(&m[16..19], &[0x03, 0x00, 0x10]);
        assert_eq!(m.len(), 0x23);
    }

    #[test]
    fn reads_what_it_writes() {
        let m = source_ready("Phone", 7236, &[1; 16]);
        assert_eq!(read(&m), Some((1, "Phone".to_string(), Some(7236))));
    }

    #[test]
    fn stop_layout() {
        let m = stop_projection("A", &[0; 16]);
        assert_eq!(m[3], 0x02);
        assert_eq!(u16::from_be_bytes([m[0], m[1]]) as usize, m.len());
    }
}
