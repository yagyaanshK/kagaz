//! Pictures of a screen, on any system: scaling to the TV's size with
//! black bars, and turning RGBA into NV12 for the encoders.

/// Four-byte pixel layouts screens come in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// Blue, green, red, then a byte unused or alpha.
    Bgrx,
    /// Red, green, blue, then a byte unused or alpha.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Rgbx,
}

/// Scales smoothly, keeping the shape: bilinear when shrinking a little or
/// growing (a 1920x1200 screen into 1080 lines), an average of the pixels
/// covered when shrinking by half or more.
#[allow(clippy::too_many_arguments)]
pub(crate) fn scale(src: &[u8], sw: usize, sh: usize, stride: usize, layout: Layout, dst: &mut [u8], dw: usize, dh: usize) {
    if sw == 0 || sh == 0 {
        return;
    }
    // Fit inside, centred.
    let (fw, fh) = if sw * dh > sh * dw { (dw, (sh * dw / sw).max(1)) } else { ((sw * dh / sh).max(1), dh) };
    let (ox, oy) = ((dw - fw) / 2, (dh - fh) / 2);
    let (r, b) = match layout {
        Layout::Bgrx => (2, 0),
        Layout::Rgbx => (0, 2),
    };
    // Each target column and row: the source positions around it, and the
    // weight of the second (0..256). Shrinking by half or more first
    // averages 2x2 blocks by spreading the taps.
    let taps = |n: usize, f: usize, s: usize| -> Vec<(usize, usize, u32)> {
        let step = s as f64 / f as f64;
        let spread = if step >= 2.0 { (step / 2.0).floor() } else { 0.0 };
        (0..f)
            .map(|i| {
                let c = ((i as f64 + 0.5) * step - 0.5 - spread / 2.0).max(0.0);
                let i0 = (c.floor() as usize).min(n - 1);
                let w = if spread > 0.0 { 128 } else { ((c - c.floor()) * 256.0) as u32 };
                let i1 = (i0 + 1 + spread as usize).min(n - 1);
                (i0, i1, w)
            })
            .collect()
    };
    let cols = taps(sw, fw, sw);
    let rows = taps(sh, fh, sh);
    let same = sw == fw && sh == fh;
    let band_rows = fh.div_ceil(SCALE_THREADS);
    let area = &mut dst[oy * dw * 4..(oy + fh) * dw * 4];
    std::thread::scope(|s| {
        for (band, chunk) in area.chunks_mut(band_rows * dw * 4).enumerate() {
            let (cols, rows) = (&cols, &rows);
            s.spawn(move || {
                for (k, line) in chunk.chunks_mut(dw * 4).enumerate() {
                    let y = band * band_rows + k;
                    let out = &mut line[ox * 4..(ox + fw) * 4];
                    if same {
                        // The same size: only the channels change places.
                        let row = &src[y * stride..y * stride + sw * 4];
                        for (px, s) in out.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                            px[0] = s[r];
                            px[1] = s[1];
                            px[2] = s[b];
                            px[3] = 255;
                        }
                        continue;
                    }
                    let (y0, y1, wy) = rows[y];
                    let row0 = &src[y0 * stride..];
                    let row1 = &src[y1 * stride..];
                    for (px, &(x0, x1, wx)) in out.chunks_exact_mut(4).zip(cols.iter()) {
                        let (a, c) = (x0 * 4, x1 * 4);
                        let mix = |from: usize| {
                            let top = row0[a + from] as u32 * (256 - wx) + row0[c + from] as u32 * wx;
                            let bottom = row1[a + from] as u32 * (256 - wx) + row1[c + from] as u32 * wx;
                            ((top * (256 - wy) + bottom * wy) >> 16) as u8
                        };
                        px[0] = mix(r);
                        px[1] = mix(1);
                        px[2] = mix(b);
                        px[3] = 255;
                    }
                }
            });
        }
    });
}

/// [`scale`] from RGBA, for tests elsewhere.
#[cfg(test)]
pub(crate) fn scale_rgba(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
    scale(src, sw, sh, sw * 4, Layout::Rgbx, dst, dw, dh);
}

/// Threads that share the scaling of each picture.
const SCALE_THREADS: usize = 6;

/// RGBA to NV12 (BT.601, video range, as OpenH264 does), on several threads.
pub fn rgba_to_nv12(rgba: &[u8], w: usize, h: usize, y: &mut [u8], y_pitch: usize, uv: &mut [u8], uv_pitch: usize) {
    const THREADS: usize = 6;
    let pairs = h / 2;
    let band = pairs.div_ceil(THREADS).max(1);
    std::thread::scope(|s| {
        let mut ys = y.chunks_mut(band * 2 * y_pitch);
        let mut uvs = uv.chunks_mut(band * uv_pitch);
        for b in 0..pairs.div_ceil(band) {
            let (Some(yb), Some(uvb)) = (ys.next(), uvs.next()) else { break };
            s.spawn(move || {
                for k in 0..band {
                    let pair = b * band + k;
                    if pair >= pairs {
                        break;
                    }
                    for dy in 0..2 {
                        let row = &rgba[(pair * 2 + dy) * w * 4..(pair * 2 + dy + 1) * w * 4];
                        let out = &mut yb[(k * 2 + dy) * y_pitch..(k * 2 + dy) * y_pitch + w];
                        for (o, p) in out.iter_mut().zip(row.chunks_exact(4)) {
                            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
                            *o = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
                        }
                    }
                    let top = &rgba[pair * 2 * w * 4..];
                    let bottom = &rgba[(pair * 2 + 1) * w * 4..];
                    let out = &mut uvb[k * uv_pitch..k * uv_pitch + w];
                    for x in (0..w).step_by(2) {
                        let i = x * 4;
                        let avg = |c: usize| {
                            (top[i + c] as i32 + top[i + 4 + c] as i32 + bottom[i + c] as i32 + bottom[i + 4 + c] as i32) / 4
                        };
                        let (r, g, b) = (avg(0), avg(1), avg(2));
                        out[x] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
                        out[x + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_of_white_and_black() {
        let (w, h) = (4, 4);
        let mut rgba = vec![255u8; w * h * 4];
        for px in rgba[..w * 2 * 4].chunks_exact_mut(4) {
            px[..3].copy_from_slice(&[0, 0, 0]);
        }
        let (mut y, mut uv) = (vec![0u8; w * h], vec![0u8; w * h / 2]);
        rgba_to_nv12(&rgba, w, h, &mut y, w, &mut uv, w);
        assert_eq!(y[0], 16);
        assert_eq!(y[w * 3], 235);
        assert!(uv.iter().all(|&c| (127..=129).contains(&c)));
    }


    #[test]
    fn scales_with_bars_and_colours_in_place() {
        // A 4x2 BGRx picture: left half red, right half blue.
        let mut src = Vec::new();
        for _ in 0..2 {
            for x in 0..4 {
                src.extend_from_slice(if x < 2 { &[0, 0, 255, 0] } else { &[255, 0, 0, 0] });
            }
        }
        // Into 4x4: the picture fills the middle two rows, black above and below.
        let mut dst = vec![0u8; 4 * 4 * 4];
        scale(&src, 4, 2, 16, Layout::Bgrx, &mut dst, 4, 4);
        let px = |x: usize, y: usize| &dst[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(px(0, 0), &[0, 0, 0, 0]);
        assert_eq!(px(0, 1), &[255, 0, 0, 255]);
        assert_eq!(px(3, 2), &[0, 0, 255, 255]);
        assert_eq!(px(3, 3), &[0, 0, 0, 0]);
    }
}
