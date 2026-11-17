//! A generated test card, so that a session and the viewer can be tried and
//! tested without anyone's real screen: flat colours, fine stripes, a
//! smooth gradient, noise, blocks of few colours and a part that moves
//! with every tick. Each kind of area is one that some way of packing a
//! picture is made for.

/// How far the scrolling band moves to the left each tick, in pixels.
pub const SCROLL_STEP: u32 = 8;

/// The rows of the band that scrolls: the first, and the one after the last.
pub fn scroll_rows(height: u16) -> (u16, u16) {
    let h = height as u32;
    ((4 * h).div_ceil(6) as u16, (5 * h).div_ceil(6) as u16)
}

/// The colour of one pixel of the card at `tick`.
pub fn pixel(x: u32, y: u32, width: u32, height: u32, tick: u32) -> [u8; 3] {
    let band = y * 6 / height.max(1);
    match band {
        // Eight flat bars.
        0 => {
            const BARS: [[u8; 3]; 8] = [
                [235, 235, 235],
                [235, 235, 16],
                [16, 235, 235],
                [16, 235, 16],
                [235, 16, 235],
                [235, 16, 16],
                [16, 16, 235],
                [16, 16, 16],
            ];
            BARS[(x * 8 / width.max(1)) as usize % 8]
        }
        // Two colours: stripes one pixel wide, then a checkerboard.
        1 => {
            let on = if x < width / 2 { x.is_multiple_of(2) } else { (x / 4 + y / 4).is_multiple_of(2) };
            if on {
                [250, 250, 250]
            } else {
                [20, 30, 60]
            }
        }
        // Left: a smooth gradient. Right: noise that nothing can pack.
        2 | 3 => {
            if x < width / 2 {
                [
                    (x * 510 / width.max(1)) as u8,
                    ((y - height / 3) * 765 / height.max(1)) as u8,
                    ((x + y) % 256) as u8,
                ]
            } else {
                let mut n = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
                n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
                [n as u8, (n >> 8) as u8, (n >> 16) as u8]
            }
        }
        // Cells of eight pixels, in turns of a dozen colours and of a
        // hundred, that scroll to the left one cell each tick.
        4 => {
            let x = x + tick * SCROLL_STEP;
            let cell = x / 8 + (y / 8) * 31;
            let colours = if (x / 64).is_multiple_of(2) { 12 } else { 100 };
            let c = cell.wrapping_mul(2_654_435_761) >> 8;
            let c = c % colours;
            [(c * 21 % 256) as u8, (c * 67 % 256) as u8, (255 - c * 2) as u8]
        }
        // A bar that crosses the screen, one step each tick, over the
        // tick itself written in squares.
        _ => {
            let top = height * 5 / 6;
            let bar = (tick * 8) % width.max(1);
            if x >= bar && x < bar + 12 && y < top + (height - top) / 2 {
                return [255, 255, 255];
            }
            if y >= top + (height - top) / 2 {
                let square = x / 8;
                if square < 16 && x % 8 < 7 {
                    return if tick >> (15 - square) & 1 == 1 { [240, 240, 240] } else { [70, 70, 70] };
                }
            }
            [40, 44, 52]
        }
    }
}

/// The whole card at `tick`, as RGBA.
pub fn card(width: u16, height: u16, tick: u32) -> Vec<u8> {
    let (w, h) = (width as u32, height as u32);
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            let p = pixel(x, y, w, h, tick);
            out.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_has_its_size_and_is_opaque() {
        let c = card(33, 21, 0);
        assert_eq!(c.len(), 33 * 21 * 4);
        assert!(c.chunks(4).all(|p| p[3] == 255));
        // Not one size crashes it.
        for (w, h) in [(0, 0), (1, 1), (2, 7), (5, 1)] {
            assert_eq!(card(w, h, 9).len(), w as usize * h as usize * 4);
        }
    }

    #[test]
    fn only_the_two_lowest_bands_move() {
        let (w, h) = (320u16, 240u16);
        let (a, b) = (card(w, h, 4), card(w, h, 5));
        let moved: Vec<usize> = a
            .chunks(4)
            .zip(b.chunks(4))
            .enumerate()
            .filter(|(_, (p, q))| p != q)
            .map(|(i, _)| i / w as usize)
            .collect();
        assert!(!moved.is_empty());
        assert!(moved.iter().all(|row| *row >= 160), "a row above the two lowest bands moved");
    }

    #[test]
    fn the_scrolling_band_is_the_last_tick_moved_left() {
        for (w, h) in [(320u16, 240u16), (333, 77), (64, 13)] {
            let (a, b) = (card(w, h, 4), card(w, h, 5));
            let (top, bottom) = scroll_rows(h);
            assert!(top < bottom);
            let step = SCROLL_STEP as usize;
            for y in top as usize..bottom as usize {
                let row = |c: &[u8]| c[y * w as usize * 4..(y + 1) * w as usize * 4].to_vec();
                assert_eq!(row(&b)[..(w as usize - step) * 4], row(&a)[step * 4..], "row {y} of {w}x{h}");
            }
            // And the rows next to it are not part of it.
            let band = |y: u32| y * 6 / h as u32;
            assert_eq!(band(top as u32), 4);
            assert_eq!(band(bottom as u32 - 1), 4);
            assert!(top == 0 || band(top as u32 - 1) == 3);
        }
    }
}
