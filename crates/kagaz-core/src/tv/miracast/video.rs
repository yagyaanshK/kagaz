//! Pictures for the TV: a generated test pattern (what every test sends,
//! never this computer's screen), and the H.264 encoder.

use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Level, Profile,
    RateControlMode, UsageType,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer};
use openh264::OpenH264API;

/// The test pattern: colour bars, a white square crossing them once every
/// four seconds, and the time since the start in big digits (for reading
/// the delay off the TV against a clock).
pub struct Pattern {
    pub width: usize,
    pub height: usize,
    rgba: Vec<u8>,
}

const BARS: [[u8; 3]; 7] = [
    [192, 192, 192],
    [192, 192, 0],
    [0, 192, 192],
    [0, 192, 0],
    [192, 0, 192],
    [192, 0, 0],
    [0, 0, 192],
];

/// Which of a seven-segment digit's segments are lit: a b c d e f g.
const SEGMENTS: [u8; 10] = [
    0b1111110, 0b0110000, 0b1101101, 0b1111001, 0b0110011, 0b1011011, 0b1011111, 0b1110000,
    0b1111111, 0b1111011,
];

impl Pattern {
    pub fn new(width: usize, height: usize) -> Pattern {
        Pattern { width, height, rgba: vec![255; width * height * 4] }
    }

    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, c: [u8; 3]) {
        for row in y.min(self.height)..(y + h).min(self.height) {
            let start = (row * self.width + x.min(self.width)) * 4;
            let end = (row * self.width + (x + w).min(self.width)) * 4;
            for px in self.rgba[start..end].chunks_exact_mut(4) {
                px[..3].copy_from_slice(&c);
            }
        }
    }

    fn digit(&mut self, x: usize, y: usize, size: usize, d: u8) {
        let t = (size / 6).max(2);
        let segs = SEGMENTS[d as usize % 10];
        let white = [255, 255, 255];
        let on = |bit: u8| segs & (1 << (6 - bit)) != 0;
        if on(0) { self.fill(x + t, y, size - 2 * t, t, white); }
        if on(1) { self.fill(x + size - t, y + t, t, size - t, white); }
        if on(2) { self.fill(x + size - t, y + size, t, size - t, white); }
        if on(3) { self.fill(x + t, y + 2 * size - t, size - 2 * t, t, white); }
        if on(4) { self.fill(x, y + size, t, size - t, white); }
        if on(5) { self.fill(x, y + t, t, size - t, white); }
        if on(6) { self.fill(x + t, y + size - t / 2, size - 2 * t, t, white); }
    }

    /// Draws the picture for `millis` since the start and returns it as
    /// RGBA.
    pub fn draw(&mut self, millis: u64) -> &[u8] {
        let (w, h) = (self.width, self.height);
        let bar_h = h * 2 / 3;
        for (i, c) in BARS.iter().enumerate() {
            let x0 = w * i / BARS.len();
            let x1 = w * (i + 1) / BARS.len();
            self.fill(x0, 0, x1 - x0, bar_h, *c);
        }
        self.fill(0, bar_h, w, h - bar_h, [16, 16, 16]);
        // The square: across the bars once every four seconds.
        let side = h / 8;
        let x = ((millis % 4000) as usize * (w - side)) / 4000;
        self.fill(x, bar_h / 2 - side / 2, side, side, [255, 255, 255]);
        // Seconds and milliseconds: SSS.mmm
        let size = (h - bar_h) / 3;
        let digits = [
            (millis / 100_000 % 10) as u8,
            (millis / 10_000 % 10) as u8,
            (millis / 1000 % 10) as u8,
            (millis / 100 % 10) as u8,
            (millis / 10 % 10) as u8,
            (millis % 10) as u8,
        ];
        let step = size * 3 / 2;
        let total = step * 6 + size / 2;
        let mut x = w.saturating_sub(total) / 2;
        let y = bar_h + (h - bar_h - 2 * size) / 2;
        for (i, d) in digits.iter().enumerate() {
            if i == 3 {
                self.fill(x, y + 2 * size - size / 6, size / 6, size / 6, [255, 255, 255]);
                x += size / 2;
            }
            self.digit(x, y, size, *d);
            x += step;
        }
        &self.rgba
    }
}

/// H.264, Constrained Baseline, as every Wi-Fi Display TV takes.
pub struct Video {
    encoder: Encoder,
    yuv: YUVBuffer,
    width: usize,
    height: usize,
}

/// One encoded picture.
pub struct Encoded {
    pub data: Vec<u8>,
    pub key: bool,
}

impl Video {
    pub fn new(width: usize, height: usize, fps: u32, bits_per_second: u32) -> Result<Video, String> {
        let level = match (height > 720, fps > 30) {
            (true, true) => Level::Level_4_2,
            (true, false) => Level::Level_4_0,
            (false, true) => Level::Level_3_2,
            (false, false) => Level::Level_3_1,
        };
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bits_per_second))
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .profile(Profile::Baseline)
            .level(level)
            // Over budget, a picture is skipped rather than the stream
            // falling behind.
            .skip_frames(true)
            .adaptive_quantization(false)
            .background_detection(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 2));
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| format!("the H.264 encoder did not start: {e}"))?;
        Ok(Video { encoder, yuv: YUVBuffer::new(width, height), width, height })
    }

    /// The next picture starts a new sequence (the TV asked for one, or
    /// lost its place).
    pub fn key_next(&mut self) {
        self.encoder.force_intra_frame();
    }

    pub fn encode(&mut self, rgba: &[u8]) -> Result<Encoded, String> {
        self.yuv.read_rgba8(RgbaSliceU8::new(rgba, (self.width, self.height)));
        let bits = self.encoder.encode(&self.yuv).map_err(|e| format!("encoding failed: {e}"))?;
        let key = matches!(bits.frame_type(), FrameType::IDR | FrameType::I);
        Ok(Encoded { data: bits.to_vec(), key })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_changes_over_time() {
        let mut p = Pattern::new(320, 180);
        let a = p.draw(0).to_vec();
        let b = p.draw(1234).to_vec();
        assert_eq!(a.len(), 320 * 180 * 4);
        assert_ne!(a, b);
    }

    #[test]
    fn encodes_a_key_frame_first() {
        let mut p = Pattern::new(320, 240);
        let mut v = Video::new(320, 240, 30, 1_000_000).unwrap();
        let first = v.encode(p.draw(0)).unwrap();
        assert!(first.key);
        // Annex B with a sequence parameter set at the start.
        assert_eq!(&first.data[..4], &[0, 0, 0, 1]);
        assert_eq!(first.data[4] & 0x1f, 7);
        let second = v.encode(p.draw(33)).unwrap();
        assert!(!second.key);
    }
}
