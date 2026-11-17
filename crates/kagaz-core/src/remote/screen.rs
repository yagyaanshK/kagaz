//! The remote screen as Kagaz holds it, shared between the session that
//! draws it and whoever shows it (the window, the command line).
//!
//! The session writes areas as they arrive. Whoever shows it takes what
//! changed when it is ready for more, and gets the pixels as they are at
//! that moment: a viewer that falls behind skips the pictures in between
//! instead of queueing them up, which is what keeps a slow link from
//! lagging further and further behind the device.
//!
//! Pixels are RGBA, four bytes each, row by row, always opaque.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// An area of the screen, in the device's own pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect { x, y, width, height }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    fn right(&self) -> u32 {
        self.x as u32 + self.width as u32
    }

    fn bottom(&self) -> u32 {
        self.y as u32 + self.height as u32
    }

    pub fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        !self.is_empty()
            && !o.is_empty()
            && (self.x as u32) < o.right()
            && (o.x as u32) < self.right()
            && (self.y as u32) < o.bottom()
            && (o.y as u32) < self.bottom()
    }

    pub fn contains(&self, o: &Rect) -> bool {
        self.x <= o.x && self.y <= o.y && self.right() >= o.right() && self.bottom() >= o.bottom()
    }

    /// The smallest area holding both.
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect {
            x,
            y,
            width: (self.right().max(o.right()) - x as u32) as u16,
            height: (self.bottom().max(o.bottom()) - y as u32) as u16,
        }
    }

    /// The part of it on a screen of this size.
    pub fn within(&self, width: u16, height: u16) -> Rect {
        let x = self.x.min(width);
        let y = self.y.min(height);
        Rect {
            x,
            y,
            width: (self.right().min(width as u32) - x as u32) as u16,
            height: (self.bottom().min(height as u32) - y as u32) as u16,
        }
    }
}

/// The shape of the device's pointer, drawn here so that it moves at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    pub width: u16,
    pub height: u16,
    /// The point of it that does the pointing.
    pub hot_x: u16,
    pub hot_y: u16,
    /// RGBA; transparent where the pointer is not.
    pub rgba: Vec<u8>,
}

/// News from the device other than its picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Text was copied on the device.
    Clipboard(String),
    Pointer(Pointer),
    Bell,
    /// The session is over, and why.
    Ended(String),
}

/// One step in bringing a shown copy of the screen up to date; the steps
/// of one [`Taken`] are applied in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// The screen is this size now; a picture of all of it follows.
    Size { width: u16, height: u16 },
    Picture { rect: Rect, rgba: Vec<u8> },
    /// What is shown at `from` goes to `rect` (scrolling, a dragged window).
    Move { from_x: u16, from_y: u16, rect: Rect },
}

#[derive(Debug, Default)]
pub struct Taken {
    pub pieces: Vec<Piece>,
    pub events: Vec<Event>,
}

impl Taken {
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty() && self.events.is_empty()
    }
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Dirty(Rect),
    Move { from_x: u16, from_y: u16, rect: Rect },
}

/// More changed areas than this are merged, the least wasteful pair first.
const MAX_AREAS: usize = 24;
/// More steps than this and the moves are given up for plain pictures.
const MAX_OPS: usize = 64;

#[derive(Default)]
struct Inner {
    width: u16,
    height: u16,
    rgba: Vec<u8>,
    /// The size is news to whoever takes next.
    resized: bool,
    ops: Vec<Op>,
    events: Vec<Event>,
    /// There is something to take, and it is not half an update.
    ready: bool,
}

impl Inner {
    fn dirty(&mut self, r: Rect) {
        if r.is_empty() || self.resized {
            return;
        }
        // Only the areas after the last move may be merged: an area before
        // it is painted before the move, and the move may paint over it.
        let start = self
            .ops
            .iter()
            .rposition(|op| matches!(op, Op::Move { .. }))
            .map_or(0, |i| i + 1);
        let covered = self.ops[start..]
            .iter()
            .any(|op| matches!(op, Op::Dirty(d) if d.contains(&r)));
        if covered {
            return;
        }
        let mut i = start;
        while i < self.ops.len() {
            match self.ops[i] {
                Op::Dirty(d) if r.contains(&d) => {
                    self.ops.remove(i);
                }
                _ => i += 1,
            }
        }
        self.ops.push(Op::Dirty(r));
        if self.ops.len() > MAX_OPS {
            for op in &mut self.ops {
                if let Op::Move { rect, .. } = *op {
                    *op = Op::Dirty(rect);
                }
            }
            merge(&mut self.ops, 0);
        } else {
            merge(&mut self.ops, start);
        }
    }

    fn pixels(&self, r: Rect) -> Vec<u8> {
        let stride = self.width as usize * 4;
        let mut out = Vec::with_capacity(r.area() as usize * 4);
        for row in r.y as usize..r.bottom() as usize {
            let at = row * stride + r.x as usize * 4;
            out.extend_from_slice(&self.rgba[at..at + r.width as usize * 4]);
        }
        out
    }
}

/// Merge the changed areas from `start` on until there are few enough,
/// each time the two whose union adds the least that did not change.
fn merge(ops: &mut Vec<Op>, start: usize) {
    while ops.len() - start > MAX_AREAS {
        let mut best: Option<(usize, usize, i64)> = None;
        for i in start..ops.len() {
            for j in i + 1..ops.len() {
                let (Op::Dirty(a), Op::Dirty(b)) = (ops[i], ops[j]) else { continue };
                let waste = a.union(&b).area() as i64 - a.area() as i64 - b.area() as i64;
                if best.is_none_or(|(_, _, w)| waste < w) {
                    best = Some((i, j, waste));
                }
            }
        }
        let Some((i, j, _)) = best else { return };
        let (Op::Dirty(a), Op::Dirty(b)) = (ops[i], ops[j]) else { return };
        ops[i] = Op::Dirty(a.union(&b));
        ops.remove(j);
    }
}

#[derive(Default)]
pub struct Screen {
    inner: Mutex<Inner>,
    changed: Condvar,
}

impl Screen {
    pub fn new() -> Screen {
        Screen::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn size(&self) -> (u16, u16) {
        let s = self.lock();
        (s.width, s.height)
    }

    // ---- the session's side ----

    /// The device's screen is this size (now, or from now on): black until
    /// it is drawn.
    pub fn resize(&self, width: u16, height: u16) {
        let mut s = self.lock();
        s.width = width;
        s.height = height;
        s.rgba = [0, 0, 0, 255].repeat(width as usize * height as usize);
        s.ops.clear();
        s.resized = true;
    }

    /// Pixels for an area (`rect.width * rect.height * 4` bytes of RGBA).
    /// What falls outside the screen is left out.
    pub fn put(&self, rect: Rect, rgba: &[u8]) {
        let mut s = self.lock();
        let c = rect.within(s.width, s.height);
        if c.is_empty() || rgba.len() < rect.area() as usize * 4 {
            return;
        }
        let stride = s.width as usize * 4;
        let from_stride = rect.width as usize * 4;
        let skip = (c.x - rect.x) as usize * 4;
        for row in 0..c.height as usize {
            let from = (row + (c.y - rect.y) as usize) * from_stride + skip;
            let to = (c.y as usize + row) * stride + c.x as usize * 4;
            s.rgba[to..to + c.width as usize * 4].copy_from_slice(&rgba[from..from + c.width as usize * 4]);
        }
        s.dirty(c);
    }

    /// One colour over an area.
    pub fn fill(&self, rect: Rect, rgb: [u8; 3]) {
        let mut s = self.lock();
        let c = rect.within(s.width, s.height);
        if c.is_empty() {
            return;
        }
        let stride = s.width as usize * 4;
        let px = [rgb[0], rgb[1], rgb[2], 255];
        for row in c.y as usize..c.bottom() as usize {
            let at = row * stride + c.x as usize * 4;
            for p in s.rgba[at..at + c.width as usize * 4].as_chunks_mut::<4>().0 {
                *p = px;
            }
        }
        s.dirty(c);
    }

    /// What is at (`from_x`, `from_y`) goes to `rect`.
    pub fn copy(&self, from_x: u16, from_y: u16, rect: Rect) {
        let mut s = self.lock();
        // Keep only the part whose source and target are both on screen.
        let to = rect.within(s.width, s.height);
        let from = Rect::new(from_x, from_y, to.width, to.height).within(s.width, s.height);
        let rect = Rect::new(to.x, to.y, from.width, from.height);
        if rect.is_empty() {
            return;
        }
        let from = Rect::new(from_x, from_y, rect.width, rect.height);
        let pixels = s.pixels(from);
        let stride = s.width as usize * 4;
        let row_len = rect.width as usize * 4;
        for row in 0..rect.height as usize {
            let to = (rect.y as usize + row) * stride + rect.x as usize * 4;
            s.rgba[to..to + row_len].copy_from_slice(&pixels[row * row_len..(row + 1) * row_len]);
        }
        if s.resized {
            return;
        }
        // A move stands only while what it copies is what is shown: a
        // pending picture over its source would be painted first, with the
        // pixels of later.
        let stale = s
            .ops
            .iter()
            .any(|op| matches!(op, Op::Dirty(d) if d.intersects(&from)));
        if stale {
            s.dirty(rect);
        } else {
            s.ops.push(Op::Move { from_x, from_y, rect });
            if s.ops.len() > MAX_OPS {
                s.dirty(rect);
            }
        }
    }

    /// A whole update is in: whoever shows the screen may take it.
    pub fn commit(&self) {
        let mut s = self.lock();
        if s.resized || !s.ops.is_empty() || !s.events.is_empty() {
            s.ready = true;
            self.changed.notify_all();
        }
    }

    pub fn post(&self, event: Event) {
        let mut s = self.lock();
        s.events.push(event);
        s.ready = true;
        self.changed.notify_all();
    }

    // ---- the side that shows it ----

    /// Wait until there is something to take; false when the time ran out.
    pub fn wait(&self, timeout: Duration) -> bool {
        let s = self.lock();
        let (s, _) = self
            .changed
            .wait_timeout_while(s, timeout, |s| !s.ready)
            .unwrap_or_else(|e| e.into_inner());
        s.ready
    }

    /// Everything that changed since the last take, with the pixels as
    /// they are now.
    pub fn take(&self) -> Taken {
        let mut s = self.lock();
        s.ready = false;
        let mut pieces = Vec::new();
        if s.resized {
            s.resized = false;
            s.ops.clear();
            pieces.push(Piece::Size {
                width: s.width,
                height: s.height,
            });
            let all = Rect::new(0, 0, s.width, s.height);
            if !all.is_empty() {
                pieces.push(Piece::Picture {
                    rect: all,
                    rgba: s.rgba.clone(),
                });
            }
        } else {
            for op in std::mem::take(&mut s.ops) {
                pieces.push(match op {
                    Op::Dirty(rect) => Piece::Picture {
                        rect,
                        rgba: s.pixels(rect),
                    },
                    Op::Move { from_x, from_y, rect } => Piece::Move { from_x, from_y, rect },
                });
            }
        }
        Taken {
            pieces,
            events: std::mem::take(&mut s.events),
        }
    }

    /// The whole screen as it is now: its size and its pixels.
    pub fn snapshot(&self) -> (u16, u16, Vec<u8>) {
        let s = self.lock();
        (s.width, s.height, s.rgba.clone())
    }
}

/// A copy of the screen kept up to date from what is taken, the way the
/// window's page does it. For tests: it must always equal the screen.
#[cfg(test)]
pub(crate) struct Shown {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

#[cfg(test)]
impl Shown {
    pub fn new() -> Shown {
        Shown {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        }
    }

    pub fn apply(&mut self, taken: &Taken) {
        for piece in &taken.pieces {
            match piece {
                Piece::Size { width, height } => {
                    self.width = *width;
                    self.height = *height;
                    self.rgba = vec![0; *width as usize * *height as usize * 4];
                }
                Piece::Picture { rect, rgba } => self.paint(*rect, rgba),
                Piece::Move { from_x, from_y, rect } => {
                    let stride = self.width as usize * 4;
                    let len = rect.width as usize * 4;
                    let mut pixels = Vec::new();
                    for row in 0..rect.height as usize {
                        let at = (*from_y as usize + row) * stride + *from_x as usize * 4;
                        pixels.extend_from_slice(&self.rgba[at..at + len]);
                    }
                    self.paint(*rect, &pixels);
                }
            }
        }
    }

    fn paint(&mut self, rect: Rect, rgba: &[u8]) {
        let stride = self.width as usize * 4;
        let len = rect.width as usize * 4;
        for row in 0..rect.height as usize {
            let at = (rect.y as usize + row) * stride + rect.x as usize * 4;
            self.rgba[at..at + len].copy_from_slice(&rgba[row * len..(row + 1) * len]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(rect: Rect, seed: u8) -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..rect.height {
            for x in 0..rect.width {
                out.extend_from_slice(&[(x as u8).wrapping_add(seed), (y as u8) ^ seed, seed, 255]);
            }
        }
        out
    }

    #[test]
    fn rect_arithmetic() {
        let a = Rect::new(10, 10, 20, 20);
        assert!(a.intersects(&Rect::new(29, 29, 5, 5)));
        assert!(!a.intersects(&Rect::new(30, 10, 5, 5)));
        assert!(a.contains(&Rect::new(12, 12, 18, 18)));
        assert!(!a.contains(&Rect::new(12, 12, 19, 18)));
        assert_eq!(a.union(&Rect::new(0, 25, 5, 10)), Rect::new(0, 10, 30, 25));
        assert_eq!(Rect::new(90, 95, 20, 20).within(100, 100), Rect::new(90, 95, 10, 5));
        assert!(Rect::new(120, 0, 5, 5).within(100, 100).is_empty());
    }

    #[test]
    fn the_first_take_is_the_size_and_all_of_it() {
        let screen = Screen::new();
        screen.resize(8, 4);
        screen.put(Rect::new(1, 1, 2, 2), &pattern(Rect::new(0, 0, 2, 2), 9));
        screen.commit();
        assert!(screen.wait(Duration::from_millis(10)));
        let taken = screen.take();
        assert_eq!(taken.pieces.len(), 2);
        assert_eq!(taken.pieces[0], Piece::Size { width: 8, height: 4 });
        let mut shown = Shown::new();
        shown.apply(&taken);
        assert_eq!(shown.rgba, screen.snapshot().2);
        assert!(!screen.wait(Duration::from_millis(1)));
        assert!(screen.take().is_empty());
    }

    #[test]
    fn put_keeps_what_is_on_screen() {
        let screen = Screen::new();
        screen.resize(10, 10);
        screen.take();
        // Half off the right and the bottom edge.
        let rect = Rect::new(7, 8, 6, 6);
        screen.put(rect, &pattern(rect, 3));
        let taken = screen.take();
        assert_eq!(taken.pieces.len(), 1);
        let Piece::Picture { rect: got, rgba } = &taken.pieces[0] else { panic!() };
        assert_eq!(*got, Rect::new(7, 8, 3, 2));
        // Its first pixel is the pattern's first pixel.
        assert_eq!(&rgba[..4], &pattern(rect, 3)[..4]);
    }

    /// Whatever is drawn and however rarely it is taken, a shown copy
    /// brought up to date from the takes equals the screen.
    #[test]
    fn a_shown_copy_always_ends_up_equal() {
        let screen = Screen::new();
        screen.resize(200, 120);
        let mut shown = Shown::new();
        shown.apply(&screen.take());
        // A fixed pseudo-random run of puts, fills and copies.
        let mut n: u32 = 12345;
        let mut next = move |below: u32| {
            n = n.wrapping_mul(1664525).wrapping_add(1013904223);
            (n >> 8) % below
        };
        for step in 0..3000u32 {
            let rect = Rect::new(next(190) as u16, next(110) as u16, 1 + next(60) as u16, 1 + next(40) as u16);
            match next(4) {
                0 => screen.put(rect, &pattern(rect, step as u8)),
                1 => screen.fill(rect, [step as u8, 7, 200]),
                _ => screen.copy(next(190) as u16, next(110) as u16, rect),
            }
            // Take now and then: often, and after long stretches.
            if next(100) < 8 || step % 700 == 699 {
                shown.apply(&screen.take());
                assert_eq!(shown.rgba, screen.snapshot().2, "after step {step}");
            }
        }
        shown.apply(&screen.take());
        assert_eq!(shown.rgba, screen.snapshot().2);
    }

    #[test]
    fn a_scroll_is_a_move_until_its_source_is_stale() {
        let screen = Screen::new();
        screen.resize(100, 100);
        screen.take();
        screen.copy(0, 10, Rect::new(0, 0, 100, 90));
        let taken = screen.take();
        assert!(matches!(taken.pieces[..], [Piece::Move { from_x: 0, from_y: 10, .. }]));
        // A picture waiting over the source: the move becomes a picture.
        screen.fill(Rect::new(0, 20, 10, 10), [1, 2, 3]);
        screen.copy(0, 10, Rect::new(0, 0, 100, 90));
        let taken = screen.take();
        assert!(taken.pieces.iter().all(|p| matches!(p, Piece::Picture { .. })));
    }

    #[test]
    fn many_small_changes_stay_few_pieces() {
        let screen = Screen::new();
        screen.resize(640, 480);
        screen.take();
        for i in 0..400u16 {
            screen.fill(Rect::new((i * 37) % 600, (i * 53) % 440, 8, 8), [i as u8, 0, 0]);
        }
        let taken = screen.take();
        assert!(taken.pieces.len() <= MAX_AREAS, "{} pieces", taken.pieces.len());
    }

    #[test]
    fn events_wake_the_taker() {
        let screen = Screen::new();
        screen.post(Event::Clipboard("hello".into()));
        assert!(screen.wait(Duration::from_millis(1)));
        assert_eq!(screen.take().events, vec![Event::Clipboard("hello".into())]);
    }
}
