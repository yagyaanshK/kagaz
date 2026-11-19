//! This computer's screen on Linux, for mirroring to a TV.
//!
//! The screen comes through the desktop's screen-sharing portal
//! (xdg-desktop-portal ScreenCast), which shows the desktop's own dialog
//! each time: the owner picks a screen or a window, and can refuse. GNOME,
//! KDE and Hyprland each answer through their own portal. The portal hands
//! over a PipeWire connection and a stream on it; Kagaz keeps the newest
//! picture and scales it to the TV's size when the TV is ready for one.
//! PipeWire itself is opened only then (`pw.rs`).
//!
//! Kept inside `tv/` for now (the owner's choice of 2026-11-18); written so
//! that remote connection's host can use it later.

use super::picture::{scale, Layout};
use super::pw;
use dbus::arg::{OwnedFd as DbusFd, PropMap, RefArg, Variant};
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use std::collections::HashMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A picture left in graphics memory (DMA-BUF), laid out row after row:
/// what the graphics chip's encoder takes without a copy.
#[derive(Debug)]
pub struct GpuPicture {
    /// Kagaz's own duplicate of the buffer's descriptor.
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
    /// How the memory is laid out (drm_fourcc.h); row after row so far.
    pub modifier: u64,
    /// PipeWire's name for the pixel layout ([`pw::BGRX`] ...).
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

/// The newest picture of the screen, as it came.
#[derive(Default)]
struct Latest {
    width: usize,
    height: usize,
    stride: usize,
    layout: Option<Layout>,
    data: Vec<u8>,
    /// Instead of `data`, when the pictures come in graphics memory.
    gpu: Option<Arc<GpuPicture>>,
    /// Counts pictures; 0 until the first arrives.
    seq: u64,
}

/// What can be shown: whole screens, single windows, or both; or a new
/// monitor of the given size that exists only for the TV (extending the
/// desktop onto it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Screen,
    Window,
    Either,
    Extend { width: u32, height: u32 },
}

/// A screen being captured. Dropping it ends the capture (and the
/// portal's session, which takes the desktop's "sharing" sign away).
pub struct Screen {
    latest: Arc<Mutex<Latest>>,
    stream: Option<pw::VideoStream>,
    /// Kept so the portal session lives as long as the capture.
    _portal: Option<Connection>,
    last_seq: u64,
    last_gpu_seq: u64,
    scaled: Vec<u8>,
    /// The picture taken out of `latest` for scaling, swapped in and out so
    /// the desktop's thread never waits for the scaling.
    taken: Latest,
    /// Pictures that arrived from the desktop.
    pub arrived: Arc<AtomicU64>,
}

impl Drop for Screen {
    fn drop(&mut self) {
        // The stream ends before the portal's session.
        self.stream.take();
    }
}

pub(crate) const PORTAL: &str = "org.freedesktop.portal.Desktop";
pub(crate) const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST: &str = "org.freedesktop.portal.ScreenCast";

pub(crate) fn v<T: RefArg + 'static>(x: T) -> Variant<Box<dyn RefArg>> {
    Variant(Box::new(x))
}

/// Calls a portal method that answers through a Request object, and waits
/// for the answer (the owner may take a while in the dialog).
pub(crate) fn portal_request(
    c: &Connection,
    method: &str,
    token: &str,
    call: impl FnOnce(&dbus::blocking::Proxy<'_, &Connection>, PropMap) -> Result<(), dbus::Error>,
    wait: Duration,
) -> Result<PropMap, String> {
    let sender = c.unique_name().to_string().trim_start_matches(':').replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let answer: Arc<Mutex<Option<(u32, PropMap)>>> = Arc::new(Mutex::new(None));
    let a = answer.clone();
    let rule = MatchRule::new_signal("org.freedesktop.portal.Request", "Response")
        .with_path(dbus::Path::from(path).into_static());
    let match_token = c
        .add_match(rule, move |(code, results): (u32, PropMap), _, _| {
            *a.lock().unwrap() = Some((code, results));
            false
        })
        .map_err(|e| format!("the screen-sharing portal could not be reached: {e}"))?;
    let mut options: PropMap = HashMap::new();
    options.insert("handle_token".into(), v(token.to_string()));
    let proxy = c.with_proxy(PORTAL, PORTAL_PATH, Duration::from_secs(30));
    call(&proxy, options).map_err(|e| format!("the screen-sharing portal refused {method}: {e}"))?;
    let end = Instant::now() + wait;
    let result = loop {
        if let Some(r) = answer.lock().unwrap().take() {
            break Ok(r);
        }
        if Instant::now() > end {
            break Err("the screen-sharing dialog was not answered".to_string());
        }
        let _ = c.process(Duration::from_millis(100));
    };
    let _ = c.remove_match(match_token);
    match result? {
        (0, results) => Ok(results),
        (1, _) => Err("screen sharing was cancelled".into()),
        _ => Err("the desktop did not allow screen sharing".into()),
    }
}

pub(crate) fn token() -> String {
    format!("kagaz{}", uuid::Uuid::new_v4().simple())
}

/// Asks the desktop for a screen or window, through its own dialog.
fn from_portal(choice: Choice) -> Result<(Connection, OwnedFd, u32), String> {
    let c = Connection::new_session().map_err(|e| format!("no desktop session bus: {e}"))?;
    let proxy = c.with_proxy(PORTAL, PORTAL_PATH, Duration::from_secs(10));
    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    let available: u32 = proxy.get(SCREENCAST, "AvailableSourceTypes").map_err(|_| {
        "this desktop has no screen-sharing portal (xdg-desktop-portal with a backend for this desktop)".to_string()
    })?;
    let cursors: u32 = proxy.get(SCREENCAST, "AvailableCursorModes").unwrap_or(1);

    let session_token = token();
    let created = portal_request(
        &c,
        "CreateSession",
        &token(),
        |p, mut o| {
            o.insert("session_handle_token".into(), v(session_token.clone()));
            p.method_call(SCREENCAST, "CreateSession", (o,))
                .map(|(_,): (dbus::Path<'static>,)| ())
        },
        Duration::from_secs(10),
    )?;
    let session: String = created
        .get("session_handle")
        .and_then(|x| x.0.as_str().map(str::to_string))
        .ok_or("the portal gave no session")?;
    let session = dbus::Path::from(session);

    // 1 monitors, 2 windows, 4 a new virtual monitor; the cursor drawn
    // into the picture (2) when the desktop can.
    let wanted = match choice {
        Choice::Screen => 1,
        Choice::Window => 2,
        Choice::Either => 3,
        Choice::Extend { .. } => 4,
    };
    if wanted & available == 0 {
        return Err(match choice {
            Choice::Extend { .. } => "this desktop cannot make a monitor for the TV (its screen-sharing portal has no virtual monitors)".into(),
            _ => "this desktop's screen-sharing portal does not offer that".into(),
        });
    }
    let types = wanted & available;
    let s1 = session.clone();
    portal_request(
        &c,
        "SelectSources",
        &token(),
        |p, mut o| {
            o.insert("types".into(), v(types.max(1)));
            o.insert("multiple".into(), v(false));
            if cursors & 2 != 0 {
                o.insert("cursor_mode".into(), v(2u32));
            }
            p.method_call(SCREENCAST, "SelectSources", (s1, o))
                .map(|(_,): (dbus::Path<'static>,)| ())
        },
        Duration::from_secs(10),
    )?;
    let s2 = session.clone();
    let started = portal_request(
        &c,
        "Start",
        &token(),
        |p, o| p.method_call(SCREENCAST, "Start", (s2, "", o)).map(|(_,): (dbus::Path<'static>,)| ()),
        Duration::from_secs(300),
    )?;
    // streams: a(ua{sv}); the first stream's node.
    let node = started
        .get("streams")
        .and_then(|s| s.0.as_iter())
        .and_then(|mut it| it.next())
        .and_then(|stream| stream.as_iter().and_then(|mut f| f.next()).and_then(|n| n.as_u64()))
        .ok_or("the portal gave no screen")? as u32;
    let (fd,): (DbusFd,) = proxy
        .method_call(SCREENCAST, "OpenPipeWireRemote", (session, PropMap::new()))
        .map_err(|e| format!("the portal gave no PipeWire connection: {e}"))?;
    // SAFETY: the descriptor was just received and is owned by nobody else.
    let fd = unsafe { OwnedFd::from_raw_fd(fd.into_fd()) };
    Ok((c, fd, node))
}

impl Screen {
    /// Asks the owner, through the desktop's dialog, which screen or window
    /// to share, and starts taking pictures of it. `gpu`: take them in
    /// graphics memory where the desktop can (for the graphics chip's
    /// encoder; the processor's still gets them, through a mapping).
    pub fn ask(choice: Choice, gpu: bool) -> Result<Screen, String> {
        let (portal, fd, node) = from_portal(choice)?;
        let size = match choice {
            Choice::Extend { width, height } => Some((width, height)),
            _ => None,
        };
        let mut s = Screen::start(Some(fd), node, size, gpu)?;
        s._portal = Some(portal);
        Ok(s)
    }

    /// Takes pictures from a PipeWire stream: over `fd` (from the portal),
    /// or this session's PipeWire when `None` (tests, with a stream made
    /// some other way).
    pub fn from_node(fd: Option<OwnedFd>, node: u32, gpu: bool) -> Result<Screen, String> {
        Screen::start(fd, node, None, gpu)
    }

    /// `size` fixes the picture size: a virtual monitor takes the size its
    /// stream asks for.
    fn start(fd: Option<OwnedFd>, node: u32, size: Option<(u32, u32)>, gpu: bool) -> Result<Screen, String> {
        let latest = Arc::new(Mutex::new(Latest::default()));
        let arrived = Arc::new(AtomicU64::new(0));
        let stream = run_stream(fd, node, size, gpu, latest.clone(), arrived.clone())?;
        Ok(Screen {
            latest,
            stream: Some(stream),
            _portal: None,
            last_seq: 0,
            last_gpu_seq: 0,
            scaled: Vec::new(),
            taken: Latest::default(),
            arrived,
        })
    }

    /// How pictures come from the desktop.
    pub fn route(&self) -> &'static str {
        match self.latest.lock().unwrap().gpu {
            Some(_) => "graphics memory",
            None => "shared memory",
        }
    }

    /// The newest picture when the pictures come in graphics memory: `None`
    /// when they do not, `Some(None)` when nothing new came since the last
    /// call.
    pub fn gpu_picture(&mut self) -> Option<Option<Arc<GpuPicture>>> {
        let l = self.latest.lock().unwrap();
        let g = l.gpu.clone()?;
        if l.seq == self.last_gpu_seq {
            return Some(None);
        }
        self.last_gpu_seq = l.seq;
        Some(Some(g))
    }

    /// Waits up to `wait` for the first picture; its size.
    pub fn first_picture(&self, wait: Duration) -> Option<(usize, usize)> {
        let end = Instant::now() + wait;
        while Instant::now() < end {
            {
                let l = self.latest.lock().unwrap();
                if l.seq > 0 {
                    return Some((l.width, l.height));
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// The newest picture scaled to `width` x `height` RGBA, with black
    /// bars where the shapes differ. Unchanged pictures are not redone.
    pub fn picture(&mut self, width: usize, height: usize) -> &[u8] {
        if self.scaled.len() != width * height * 4 {
            self.scaled = vec![0; width * height * 4];
            for px in self.scaled.chunks_exact_mut(4) {
                px[3] = 255;
            }
            self.last_seq = 0;
        }
        {
            // Swap the newest picture out (no copy), leaving the old buffer
            // for the desktop's thread to fill next.
            let mut l = self.latest.lock().unwrap();
            if l.seq == self.last_seq || l.seq == 0 {
                return &self.scaled;
            }
            if let Some(g) = l.gpu.clone() {
                self.last_seq = l.seq;
                drop(l);
                if let Err(e) = scale_mapped(&g, &mut self.scaled, width, height) {
                    eprintln!("kagaz: a picture in graphics memory could not be read: {e}");
                }
                return &self.scaled;
            }
            std::mem::swap(&mut self.taken.data, &mut l.data);
            self.taken.width = l.width;
            self.taken.height = l.height;
            self.taken.stride = l.stride;
            self.taken.layout = l.layout;
            self.last_seq = l.seq;
        }
        let t = &self.taken;
        if let Some(layout) = t.layout {
            if t.data.len() >= t.stride * t.height {
                scale(&t.data, t.width, t.height, t.stride, layout, &mut self.scaled, width, height);
            }
        }
        &self.scaled
    }
}

/// Reads a picture in graphics memory through a mapping (for the
/// processor's encoder) and scales it.
fn scale_mapped(g: &GpuPicture, dst: &mut [u8], dw: usize, dh: usize) -> Result<(), String> {
    read_mapped(g, |src, w, h, stride, layout| scale(src, w, h, stride, layout, dst, dw, dh))
}

/// Lends the pixels of a picture in graphics memory: (pixels, width,
/// height, stride, layout).
fn read_mapped<T>(g: &GpuPicture, read: impl FnOnce(&[u8], usize, usize, usize, Layout) -> T) -> Result<T, String> {
    use std::os::fd::AsRawFd;
    let layout = match g.format {
        pw::BGRX | pw::BGRA => Layout::Bgrx,
        pw::RGBX | pw::RGBA => Layout::Rgbx,
        _ => return Err("an unknown pixel layout".into()),
    };
    let (w, h) = (g.width as usize, g.height as usize);
    let stride = if g.stride == 0 { w * 4 } else { g.stride as usize };
    let len = g.offset as usize + stride * h;
    // DMA_BUF_IOCTL_SYNC (linux/dma-buf.h): reads start / end.
    const SYNC: libc::c_ulong = 0x4008_6200;
    const READ_START: u64 = 1;
    const READ_END: u64 = 1 | 4;
    // SAFETY: a read-only shared mapping of a buffer Kagaz holds a
    // descriptor of, unmapped before returning; the slice lives only
    // between the two.
    unsafe {
        let p = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, g.fd.as_raw_fd(), 0);
        if p == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error().to_string());
        }
        libc::ioctl(g.fd.as_raw_fd(), SYNC, &READ_START);
        let src = std::slice::from_raw_parts((p as *const u8).add(g.offset as usize), stride * h);
        let out = read(src, w, h, stride, layout);
        libc::ioctl(g.fd.as_raw_fd(), SYNC, &READ_END);
        libc::munmap(p, len);
        Ok(out)
    }
}

/// Opens the stream; PipeWire's own thread puts each picture in `latest`.
fn run_stream(
    fd: Option<OwnedFd>,
    node: u32,
    size: Option<(u32, u32)>,
    gpu: bool,
    latest: Arc<Mutex<Latest>>,
    arrived: Arc<AtomicU64>,
) -> Result<pw::VideoStream, String> {
    let settled = Arc::new(Mutex::new(None::<(Layout, pw::VideoFormat)>));
    let set = settled.clone();
    pw::VideoStream::open(
        fd,
        node,
        size,
        gpu,
        move |f| {
            let l = match f.format {
                pw::BGRX | pw::BGRA => Layout::Bgrx,
                pw::RGBX | pw::RGBA => Layout::Rgbx,
                _ => return,
            };
            *set.lock().unwrap() = Some((l, f));
        },
        move |frame| {
            let Some((l, f)) = *settled.lock().unwrap() else { return };
            let (w, h) = (f.width as usize, f.height as usize);
            let (bytes, offset, size, stride) = match frame {
                pw::Frame::Memory { bytes, offset, size, stride } => (bytes, offset, size, stride),
                pw::Frame::Gpu { fd, offset, stride } => {
                    let Ok(fd) = fd.try_clone_to_owned() else { return };
                    let g = GpuPicture {
                        fd,
                        offset,
                        stride,
                        modifier: f.modifier.unwrap_or(pw::MODIFIER_LINEAR),
                        format: f.format,
                        width: f.width,
                        height: f.height,
                    };
                    let mut t = latest.lock().unwrap();
                    t.width = w;
                    t.height = h;
                    t.layout = Some(l);
                    // The one it replaces is closed after the lock is let go.
                    let _old = t.gpu.replace(Arc::new(g));
                    t.seq += 1;
                    drop(t);
                    arrived.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            };
            let stride = match stride.max(0) as usize {
                0 => w * 4,
                s => s,
            };
            if size == 0 || bytes.len() < offset + stride * h {
                // An empty buffer: the picture did not change.
                return;
            }
            let mut t = latest.lock().unwrap();
            t.width = w;
            t.height = h;
            t.stride = stride;
            t.layout = Some(l);
            t.gpu = None;
            t.data.clear();
            t.data.extend_from_slice(&bytes[offset..offset + stride * h]);
            t.seq += 1;
            arrived.fetch_add(1, Ordering::Relaxed);
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sealed test desktop's screen, straight from its compositor (no
    /// dialog; only `tests/headless-screen.sh` sets this up). Returns the
    /// capture and the session to keep alive.
    fn headless_screen() -> Option<(Screen, Connection)> {
        headless(None, wants_gpu())
    }

    /// Whether the headless tests ask for graphics memory (the default) or
    /// shared memory (`KAGAZ_SHM=1`).
    fn wants_gpu() -> bool {
        std::env::var_os("KAGAZ_SHM").is_none()
    }

    /// `virtual_size`: a new monitor of that size instead of the existing one.
    fn headless(virtual_size: Option<(u32, u32)>, gpu: bool) -> Option<(Screen, Connection)> {
        if std::env::var_os("KAGAZ_HEADLESS").is_none() {
            eprintln!("skipped: run crates/kagaz-core/tests/headless-screen.sh");
            return None;
        }
        const M: &str = "org.gnome.Mutter.ScreenCast";
        let c = Connection::new_session().expect("the test desktop's bus");
        let sc = c.with_proxy(M, "/org/gnome/Mutter/ScreenCast", Duration::from_secs(5));
        let (session,): (dbus::Path<'static>,) =
            sc.method_call(M, "CreateSession", (PropMap::new(),)).expect("a screen-cast session");
        let s = c.with_proxy(M, session.clone(), Duration::from_secs(5));
        let mut props: PropMap = HashMap::new();
        props.insert("cursor-mode".into(), v(1u32));
        let (stream,): (dbus::Path<'static>,) = match virtual_size {
            None => s.method_call(&format!("{M}.Session"), "RecordMonitor", ("", props)),
            Some(_) => s.method_call(&format!("{M}.Session"), "RecordVirtual", (props,)),
        }
        .expect("recording started");
        let node: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
        let n = node.clone();
        let rule = MatchRule::new_signal(format!("{M}.Stream"), "PipeWireStreamAdded").with_path(stream);
        c.add_match(rule, move |(id,): (u32,), _, _| {
            *n.lock().unwrap() = Some(id);
            false
        })
        .unwrap();
        let _: () = s.method_call(&format!("{M}.Session"), "Start", ()).expect("started");
        let end = Instant::now() + Duration::from_secs(10);
        while node.lock().unwrap().is_none() && Instant::now() < end {
            c.process(Duration::from_millis(100)).unwrap();
        }
        let id = node.lock().unwrap().expect("a PipeWire stream from the compositor");
        Some((Screen::start(None, id, virtual_size, gpu).expect("the stream opened"), c))
    }

    #[test]
    #[ignore]
    fn headless_pictures_arrive_and_scale() {
        let Some((mut screen, _session)) = headless_screen() else { return };
        assert_eq!(screen.first_picture(Duration::from_secs(10)), Some((1280, 800)));
        let rgba = screen.picture(1280, 720).to_vec();
        // 1280x800 into 1280x720: bars at the sides, the desktop in between.
        let px = |x: usize, y: usize| &rgba[(y * 1280 + x) * 4..(y * 1280 + x) * 4 + 4];
        assert_eq!(px(0, 360), &[0, 0, 0, 255]);
        let middle: Vec<&[u8]> = (100..1180).step_by(40).map(|x| px(x, 360)).collect();
        assert!(middle.windows(2).any(|w| w[0] != w[1]), "the desktop is one flat colour");
        eprintln!("pictures so far: {}", screen.arrived.load(Ordering::Relaxed));
    }

    #[test]
    #[ignore]
    fn headless_new_monitor_takes_the_tvs_size() {
        let Some((screen, _session)) = headless(Some((1920, 1080)), wants_gpu()) else { return };
        assert_eq!(screen.first_picture(Duration::from_secs(10)), Some((1920, 1080)));
    }

    #[test]
    #[ignore]
    fn headless_desktop_to_a_stand_in_tv() {
        // The processor's encoder takes ordinary memory (graphics memory is
        // slow for it to read).
        let Some((mut screen, _session)) = headless(None, false) else { return };
        screen.first_picture(Duration::from_secs(10)).expect("a picture");
        let mut lines = Vec::new();
        let (report, es) = crate::tv::miracast::tests::session_with_stand_in_using(
            &mut screen,
            Duration::from_millis(2000),
            crate::tv::miracast::video::Engine::Software,
            |e| {
                if let crate::tv::miracast::Event::Progress { busy_ms, worst_ms, .. } = e {
                    lines.push(format!("{busy_ms:.1} ms a picture (worst {worst_ms:.1})"))
                }
            },
        );
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut pictures = 0;
        for nal in openh264::nal_units(&es) {
            if let Ok(Some(_)) = decoder.decode(nal) {
                pictures += 1;
            }
        }
        eprintln!("{}: sent {} pictures, {} decoded; {}", screen.route(), report.frames, pictures, lines.join("; "));
        assert!(pictures >= 100, "{pictures} pictures decoded");
    }

    /// Starts a square sliding across the test desktop for good: the stage
    /// changes on every frame the compositor draws.
    fn animate_test_desktop(c: &Connection) {
        let shell = c.with_proxy("org.gnome.Shell", "/org/gnome/Shell", Duration::from_secs(5));
        let script = "const Clutter = imports.gi.Clutter; \
            const a = new imports.gi.St.Widget({width: 200, height: 200, x: 0, y: 300, \
              style: 'background-color: red'}); \
            global.stage.add_child(a); \
            const t = new Clutter.Timeline({actor: a, duration: 2000, repeat_count: -1}); \
            globalThis.kagazFrames = 0; \
            t.connect('new-frame', () => { a.x = (t.get_elapsed_time() / 2000) * 1000; globalThis.kagazFrames++; }); \
            t.start(); globalThis.kagazTimeline = t; 'ok'";
        let (ok, answer): (bool, String) =
            shell.method_call("org.gnome.Shell", "Eval", (script,)).expect("the test shell takes a script");
        assert!(ok, "the animation did not start: {answer}");
    }

    /// Pictures a second while the whole stage changes each frame (60 is
    /// the most a 60 Hz monitor gives).
    #[test]
    #[ignore]
    fn headless_rate_while_animating() {
        let Some((screen, session)) = headless_screen() else { return };
        screen.first_picture(Duration::from_secs(10)).expect("a picture");
        animate_test_desktop(&session);
        std::thread::sleep(Duration::from_secs(1));
        let shell = session.with_proxy("org.gnome.Shell", "/org/gnome/Shell", Duration::from_secs(5));
        let drawn = || -> u64 {
            let (_, n): (bool, String) =
                shell.method_call("org.gnome.Shell", "Eval", ("globalThis.kagazFrames",)).expect("frames drawn");
            n.trim_matches('"').parse().unwrap_or(0)
        };
        let (mut rates, mut frames) = (Vec::new(), Vec::new());
        for _ in 0..5 {
            let (before, drawn_before) = (screen.arrived.load(Ordering::Relaxed), drawn());
            std::thread::sleep(Duration::from_secs(1));
            rates.push(screen.arrived.load(Ordering::Relaxed) - before);
            frames.push(drawn() - drawn_before);
        }
        eprintln!("pictures a second ({}): {rates:?}; the desktop drew {frames:?}", screen.route());
    }

    /// The test desktop's screen, kept in graphics memory, encoded by the
    /// graphics chip for the stand-in TV (skipped without VA-API).
    #[test]
    #[ignore]
    fn headless_desktop_to_a_stand_in_tv_on_the_chip() {
        use crate::tv::miracast::{video::Engine, Event};
        let Some((mut screen, session)) = headless_screen() else { return };
        screen.first_picture(Duration::from_secs(10)).expect("a picture");
        animate_test_desktop(&session);
        let mut lines = Vec::new();
        let (report, es) = crate::tv::miracast::tests::session_with_stand_in_using(
            &mut screen,
            Duration::from_millis(3000),
            Engine::Auto,
            |e| match e {
                Event::Encoder(s) => lines.push(s),
                Event::Progress { busy_ms, worst_ms, delivered_per_second, .. } => {
                    lines.push(format!("{busy_ms:.1} ms a picture (worst {worst_ms:.1}), {delivered_per_second} from the desktop"))
                }
                _ => {}
            },
        );
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let decoded = openh264::nal_units(&es).filter(|n| matches!(decoder.decode(n), Ok(Some(_)))).count();
        eprintln!("{} ({}): {}; sent {}, {decoded} decoded", screen.route(), lines.join("; "), report.frames, report.frames);
        assert!(decoded as u64 + 2 >= report.frames && decoded >= 100, "{decoded} of {} decoded", report.frames);
    }

    /// On the owner's real desktop, only when asked (`KAGAZ_REAL_SCREEN=1`,
    /// the desktop's dialog asks too): for each picture the desktop
    /// delivers, the time and the share of the screen that changed since
    /// the one before. Numbers only; no picture is kept or written.
    #[test]
    #[ignore]
    fn real_screen_changes() {
        if std::env::var_os("KAGAZ_REAL_SCREEN").is_none() {
            return;
        }
        let screen = Screen::ask(Choice::Screen, std::env::var_os("KAGAZ_SHM").is_none()).expect("screen shared");
        screen.first_picture(Duration::from_secs(10)).expect("a picture");
        let start = Instant::now();
        let mut last_seq = 0;
        let mut prev: Vec<u8> = Vec::new();
        let mut lines = Vec::new();
        while start.elapsed() < Duration::from_secs(30) {
            let (seq, sample) = {
                let l = screen.latest.lock().unwrap();
                if l.seq == last_seq {
                    drop(l);
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                // A grid of 160 x 100 points, one channel each.
                let grid = |data: &[u8], w: usize, h: usize, stride: usize| {
                    let mut s = Vec::with_capacity(16_000);
                    for gy in 0..100 {
                        for gx in 0..160 {
                            let (x, y) = (gx * w / 160, gy * h / 100);
                            s.push(data[y * stride + x * 4 + 1]);
                        }
                    }
                    s
                };
                let s = match &l.gpu {
                    Some(g) => read_mapped(g, |d, w, h, stride, _| grid(d, w, h, stride)).expect("mapped"),
                    None => grid(&l.data, l.width, l.height, l.stride),
                };
                (l.seq, s)
            };
            let changed = if prev.is_empty() {
                0.0
            } else {
                sample.iter().zip(&prev).filter(|(a, b)| a.abs_diff(**b) > 8).count() as f64 / 160.0
            };
            lines.push(format!("{:6} ms  #{seq:<5} changed {changed:5.1}%", start.elapsed().as_millis()));
            prev = sample;
            last_seq = seq;
        }
        for l in &lines {
            eprintln!("{l}");
        }
    }

    #[test]
    #[ignore]
    fn scale_timing() {
        let (sw, sh) = (1920usize, 1200usize);
        let src: Vec<u8> = (0..sw * sh * 4).map(|i| (i * 7 % 251) as u8).collect();
        let mut dst = vec![0u8; 1920 * 1080 * 4];
        let a = Instant::now();
        for _ in 0..20 {
            scale(&src, sw, sh, sw * 4, Layout::Bgrx, &mut dst, 1920, 1080);
        }
        eprintln!("scale 1920x1200 -> 1080p: {:.1} ms", a.elapsed().as_secs_f64() / 20.0 * 1e3);
    }


}
