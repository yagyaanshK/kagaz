//! One shape for live pictures and sound, whatever sends them: a camera's
//! relay stream, the doorbell's relay, or a call. The window plays them all
//! with the same player, decoding in the page itself, so no helper program
//! is needed on any platform.

/// A piece of a live stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Media {
    /// One H.264 picture as Annex-B with 4-byte start codes; a keyframe
    /// carries its parameter sets (SPS, PPS) in front.
    Video {
        data: Vec<u8>,
        key: bool,
        /// 90 kHz.
        timestamp: u32,
    },
    /// Sound: 8 kHz, 16-bit, one channel.
    Audio { pcm: Vec<i16>, timestamp: u32 },
    /// The stream is over: why, or empty when it simply ended.
    Ended(String),
}

pub const START: [u8; 4] = [0, 0, 0, 1];

/// The NAL units of an Annex-B byte stream (3- or 4-byte start codes),
/// without their start codes.
pub fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut start: Option<usize> = None;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(s) = start {
                let mut end = i;
                // The zero before a 4-byte start code belongs to the code.
                if end > s && data[end - 1] == 0 {
                    end -= 1;
                }
                out.push(&data[s..end]);
            }
            i += 3;
            start = Some(i);
            continue;
        }
        i += 1;
    }
    if let Some(s) = start {
        out.push(&data[s..]);
    }
    out.retain(|n| !n.is_empty());
    out
}

/// The NAL unit type (0..32) of a unit.
pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map(|b| b & 0x1F).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_three_and_four_byte_start_codes() {
        let data = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5];
        let units = nal_units(&data);
        assert_eq!(units, vec![&[0x67, 1, 2][..], &[0x68, 3], &[0x65, 4, 5]]);
        assert_eq!(units.iter().map(|u| nal_type(u)).collect::<Vec<_>>(), vec![7, 8, 5]);
        assert!(nal_units(&[1, 2, 3]).is_empty());
    }
}
