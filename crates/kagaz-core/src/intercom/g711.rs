//! G.711, the telephone audio every door station speaks: 8 kHz, one byte
//! per sample, μ-law (PCMU, payload 0) or A-law (PCMA, payload 8).

const BIAS: i32 = 0x84;
const CLIP: i32 = 32635;

pub fn ulaw_encode(sample: i16) -> u8 {
    let mut s = sample as i32;
    let sign = if s < 0 {
        s = -s;
        0x80
    } else {
        0
    };
    s = s.min(CLIP) + BIAS;
    // Position of the highest set bit above bit 7, capped at 7.
    let mut exponent = 7;
    while exponent > 0 && (s & (0x4000 >> (7 - exponent))) == 0 {
        exponent -= 1;
    }
    let mantissa = (s >> (exponent + 3)) & 0x0F;
    !(sign | (exponent << 4) | mantissa) as u8
}

pub fn ulaw_decode(byte: u8) -> i16 {
    let u = !byte as i32;
    let sign = u & 0x80;
    let exponent = (u >> 4) & 0x07;
    let mantissa = u & 0x0F;
    let s = (((mantissa << 3) + BIAS) << exponent) - BIAS;
    (if sign != 0 { -s } else { s }) as i16
}

pub fn alaw_encode(sample: i16) -> u8 {
    let mut s = sample as i32;
    let sign = if s >= 0 { 0x80 } else { 0 };
    if s < 0 {
        s = -s - 1;
    }
    s = s.min(32767);
    let byte = if s < 256 {
        (s >> 4) as u8
    } else {
        let mut exponent = 7;
        while exponent > 1 && (s & (0x4000 >> (7 - exponent))) == 0 {
            exponent -= 1;
        }
        let mantissa = (s >> (exponent + 3)) & 0x0F;
        ((exponent << 4) | mantissa) as u8
    };
    (byte | sign) ^ 0x55
}

pub fn alaw_decode(byte: u8) -> i16 {
    let a = (byte ^ 0x55) as i32;
    let sign = a & 0x80;
    let exponent = (a >> 4) & 0x07;
    let mantissa = a & 0x0F;
    let s = if exponent == 0 {
        (mantissa << 4) + 8
    } else {
        ((mantissa << 4) + 0x108) << (exponent - 1)
    };
    (if sign != 0 { s } else { -s }) as i16
}

/// The G.711 law of an RTP payload type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Law {
    Mu,
    A,
}

impl Law {
    pub fn payload_type(self) -> u8 {
        match self {
            Law::Mu => 0,
            Law::A => 8,
        }
    }

    pub fn from_payload_type(pt: u8) -> Option<Law> {
        match pt {
            0 => Some(Law::Mu),
            8 => Some(Law::A),
            _ => None,
        }
    }

    pub fn encode(self, pcm: &[i16]) -> Vec<u8> {
        match self {
            Law::Mu => pcm.iter().map(|&s| ulaw_encode(s)).collect(),
            Law::A => pcm.iter().map(|&s| alaw_encode(s)).collect(),
        }
    }

    pub fn decode(self, bytes: &[u8]) -> Vec<i16> {
        match self {
            Law::Mu => bytes.iter().map(|&b| ulaw_decode(b)).collect(),
            Law::A => bytes.iter().map(|&b| alaw_decode(b)).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mu_law_round_trips_within_its_step() {
        for s in (-32000i32..32000).step_by(97) {
            let back = ulaw_decode(ulaw_encode(s as i16)) as i32;
            let step = (s.abs() / 16).max(8);
            assert!((back - s).abs() <= step, "{s} came back as {back}");
        }
        // Known codes: silence and full scale.
        assert_eq!(ulaw_encode(0), 0xFF);
        assert_eq!(ulaw_decode(0xFF), 0);
        assert_eq!(ulaw_encode(32767), 0x80);
    }

    #[test]
    fn a_law_round_trips_within_its_step() {
        for s in (-32000i32..32000).step_by(97) {
            let back = alaw_decode(alaw_encode(s as i16)) as i32;
            let step = (s.abs() / 16).max(16);
            assert!((back - s).abs() <= step, "{s} came back as {back}");
        }
        assert_eq!(alaw_encode(0), 0xD5);
    }

    #[test]
    fn payload_types() {
        assert_eq!(Law::from_payload_type(0), Some(Law::Mu));
        assert_eq!(Law::from_payload_type(8), Some(Law::A));
        assert_eq!(Law::from_payload_type(18), None);
        assert_eq!(Law::A.decode(&Law::A.encode(&[1000, -1000])).len(), 2);
    }
}
