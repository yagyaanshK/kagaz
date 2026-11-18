//! Pictures for the TV: a generated test pattern (what every test sends,
//! never this computer's screen), and the H.264 encoder.

use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Level, Profile,
    QpRange, RateControlMode, UsageType,
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

/// The quantiser ceiling: text stays crisp.
const QUALITY: u8 = 26;

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
        Video::tuned(width, height, fps, bits_per_second, QUALITY)
    }

    /// With a ceiling on the quantiser (lower is sharper and larger).
    pub fn tuned(width: usize, height: usize, fps: u32, bits_per_second: u32, max_qp: u8) -> Result<Video, String> {
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
            .intra_frame_period(IntraFramePeriod::from_num_frames(fps * 2))
            .qp(QpRange::new(10, max_qp))
            .complexity(Complexity::Medium);
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

#[cfg(test)]
mod quality {
    use super::*;

    /// A desktop-like picture: white, with rows of small dark "letters"
    /// (1 to 2 pixel strokes) and a coloured bar, changing a little each time.
    pub fn desktop(w: usize, h: usize, t: usize) -> Vec<u8> {
        let mut p = vec![255u8; w * h * 4];
        let mut seed = 12345u32;
        for row in (40..h - 20).step_by(22) {
            for col in (20..w - 20).step_by(9) {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                if seed >> 28 == 0 {
                    continue;
                }
                for dy in 0..12 {
                    for dx in 0..6 {
                        let on = (seed >> ((dy * 6 + dx) % 31)) & 1 == 1 && (dx == 0 || dy % 5 == 0 || dx == 5);
                        if on {
                            let i = ((row + dy) * w + col + dx + t % 3) * 4;
                            p[i..i + 3].copy_from_slice(&[30, 30, 30]);
                        }
                    }
                }
            }
        }
        for y in 0..30 {
            for x in 0..w {
                let i = (y * w + x) * 4;
                p[i..i + 3].copy_from_slice(&[47, 111, 78]);
            }
        }
        p
    }

    /// PSNR of the luma, decoded against the source.
    pub fn psnr(src: &[u8], w: usize, h: usize, yuv: &openh264::decoder::DecodedYUV) -> f64 {
        use openh264::formats::YUVSource;
        let (ys, _, _) = yuv.strides();
        let mut err = 0f64;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let l = 16.0 + (65.481 * src[i] as f64 + 128.553 * src[i + 1] as f64 + 24.966 * src[i + 2] as f64) / 255.0;
                let d = l - yuv.y()[y * ys + x] as f64;
                err += d * d;
            }
        }
        let mse = err / (w * h) as f64;
        10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
    }

    #[test]
    #[ignore]
    fn measure() {
        for (w, h, fps, bits, qp) in [
            (1280, 720, 60, 8_000_000, 51),
            (1280, 720, 60, 8_000_000, 26),
            (1920, 1080, 30, 12_000_000, 51),
            (1920, 1080, 30, 12_000_000, 30),
            (1920, 1080, 30, 12_000_000, 26),
            (1920, 1080, 30, 12_000_000, 22),
        ] {
            let mut v = Video::tuned(w, h, fps, bits, qp).unwrap();
            let mut d = openh264::decoder::Decoder::new().unwrap();
            let mut bytes = 0;
            let mut last = 0.0;
            for t in 0..60 {
                let src = desktop(w, h, t);
                let e = v.encode(&src).unwrap();
                bytes += e.data.len();
                let mut got = None;
                for nal in openh264::nal_units(&e.data) {
                    if let Ok(Some(y)) = d.decode(nal) {
                        got = Some(psnr(&src, w, h, &y));
                    }
                }
                if let Some(p) = got { last = p; }
            }
            eprintln!("qp<={qp} {w}x{h}@{fps} target {} Mbit/s: {:.1} dB, {:.1} Mbit/s", bits / 1_000_000, last, bytes as f64 * 8.0 / 1e6 * fps as f64 / 60.0);
        }
    }
}
