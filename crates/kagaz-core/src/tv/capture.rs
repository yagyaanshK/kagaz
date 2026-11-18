//! This computer's screen on Linux, for mirroring to a TV.
//!
//! The screen comes through the desktop's screen-sharing portal
//! (xdg-desktop-portal ScreenCast), which shows the desktop's own dialog
//! each time: the owner picks a screen or a window, and can refuse. GNOME,
//! KDE and Hyprland each answer through their own portal. The portal hands
//! over a PipeWire connection and a stream on it; Kagaz keeps the newest
//! picture and scales it to the TV's size when the TV is ready for one.
//!
//! Kept inside `tv/` for now (the owner's choice of 2026-11-18); written so
//! that remote connection's host can use it later.

use dbus::arg::{OwnedFd as DbusFd, PropMap, RefArg, Variant};
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use pipewire as pw;
use pw::spa;
use std::collections::HashMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The pixel layouts Kagaz takes from PipeWire: four bytes a pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    Bgrx,
    Rgbx,
}

/// The newest picture of the screen, as it came.
#[derive(Default)]
struct Latest {
    width: usize,
    height: usize,
    stride: usize,
    layout: Option<Layout>,
    data: Vec<u8>,
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
    quit: Option<pw::channel::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Kept so the portal session lives as long as the capture.
    _portal: Option<Connection>,
    last_seq: u64,
    scaled: Vec<u8>,
    /// The picture taken out of `latest` for scaling, swapped in and out so
    /// the desktop's thread never waits for the scaling.
    taken: Latest,
    /// Pictures that arrived from the desktop.
    pub arrived: Arc<AtomicU64>,
}

impl Drop for Screen {
    fn drop(&mut self) {
        if let Some(q) = self.quit.take() {
            let _ = q.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST: &str = "org.freedesktop.portal.ScreenCast";

fn v<T: RefArg + 'static>(x: T) -> Variant<Box<dyn RefArg>> {
    Variant(Box::new(x))
}

/// Calls a portal method that answers through a Request object, and waits
/// for the answer (the owner may take a while in the dialog).
fn portal_request(
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

fn token() -> String {
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
    /// to share, and starts taking pictures of it.
    pub fn ask(choice: Choice) -> Result<Screen, String> {
        let (portal, fd, node) = from_portal(choice)?;
        let size = match choice {
            Choice::Extend { width, height } => Some((width, height)),
            _ => None,
        };
        let mut s = Screen::start(Some(fd), node, size)?;
        s._portal = Some(portal);
        Ok(s)
    }

    /// Takes pictures from a PipeWire stream: over `fd` (from the portal),
    /// or this session's PipeWire when `None` (tests, with a stream made
    /// some other way).
    pub fn from_node(fd: Option<OwnedFd>, node: u32) -> Result<Screen, String> {
        Screen::start(fd, node, None)
    }

    /// `size` fixes the picture size: a virtual monitor takes the size its
    /// stream asks for.
    fn start(fd: Option<OwnedFd>, node: u32, size: Option<(u32, u32)>) -> Result<Screen, String> {
        let latest = Arc::new(Mutex::new(Latest::default()));
        let arrived = Arc::new(AtomicU64::new(0));
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let l = latest.clone();
        let count = arrived.clone();
        let thread = std::thread::Builder::new()
            .name("kagaz-screen".into())
            .spawn(move || {
                if let Err(e) = run_stream(fd, node, size, l, count, quit_rx, &ready_tx) {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("the screen stream did not start".into()),
        }
        Ok(Screen {
            latest,
            quit: Some(quit_tx),
            thread: Some(thread),
            _portal: None,
            last_seq: 0,
            scaled: Vec::new(),
            taken: Latest::default(),
            arrived,
        })
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

/// Scales smoothly, keeping the shape: bilinear when shrinking a little or
/// growing (a 1920x1200 screen into 1080 lines), an average of the pixels
/// covered when shrinking by half or more.
#[allow(clippy::too_many_arguments)]
fn scale(src: &[u8], sw: usize, sh: usize, stride: usize, layout: Layout, dst: &mut [u8], dw: usize, dh: usize) {
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

/// Threads that share the scaling of each picture.
const SCALE_THREADS: usize = 6;

fn run_stream(
    fd: Option<OwnedFd>,
    node: u32,
    size: Option<(u32, u32)>,
    latest: Arc<Mutex<Latest>>,
    arrived: Arc<AtomicU64>,
    quit: pw::channel::Receiver<()>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    pw::init();
    let err = |e: pw::Error| format!("PipeWire: {e}");
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(err)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(err)?;
    let core = match fd {
        Some(fd) => context.connect_fd_rc(fd, None),
        None => context.connect_rc(None),
    }
    .map_err(|e| format!("PipeWire could not be reached: {e}"))?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "kagaz-screen",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(err)?;

    let layout = Arc::new(Mutex::new(None::<(Layout, usize, usize)>));
    let lay = layout.clone();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |_, _, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::video::VideoInfoRaw::default();
            if info.parse(param).is_err() {
                return;
            }
            let l = match info.format() {
                spa::param::video::VideoFormat::BGRx | spa::param::video::VideoFormat::BGRA => Layout::Bgrx,
                spa::param::video::VideoFormat::RGBx | spa::param::video::VideoFormat::RGBA => Layout::Rgbx,
                _ => return,
            };
            *lay.lock().unwrap() = Some((l, info.size().width as usize, info.size().height as usize));
        })
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let Some((l, w, h)) = *layout.lock().unwrap() else { return };
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let (offset, size, stride) = {
                let c = d.chunk();
                (c.offset() as usize, c.size() as usize, c.stride().max(0) as usize)
            };
            let Some(bytes) = d.data() else { return };
            let stride = if stride == 0 { w * 4 } else { stride };
            if size == 0 || bytes.len() < offset + stride * h {
                // An empty buffer: the picture did not change.
                return;
            }
            let mut t = latest.lock().unwrap();
            t.width = w;
            t.height = h;
            t.stride = stride;
            t.layout = Some(l);
            t.data.clear();
            t.data.extend_from_slice(&bytes[offset..offset + stride * h]);
            t.seq += 1;
            arrived.fetch_add(1, Ordering::Relaxed);
        })
        .register()
        .map_err(err)?;

    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat;
    use spa::utils::{Fraction, Rectangle};
    let (want, min, max) = match size {
        // A virtual monitor: exactly this size.
        Some((w, h)) => (Rectangle { width: w, height: h }, Rectangle { width: w, height: h }, Rectangle { width: w, height: h }),
        None => (
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 },
        ),
    };
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::RGBx,
            VideoFormat::BGRA,
            VideoFormat::RGBA
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            want,
            min,
            max
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 60, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 1000, denom: 1 }
        ),
    );
    let bytes: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map_err(|_| "PipeWire format could not be written".to_string())?
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&bytes).ok_or("PipeWire format could not be read back")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(err)?;

    let ml = mainloop.clone();
    let _quit = quit.attach(mainloop.loop_(), move |_| ml.quit());
    let _ = ready.send(Ok(()));
    mainloop.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sealed test desktop's screen, straight from its compositor (no
    /// dialog; only `tests/headless-screen.sh` sets this up). Returns the
    /// capture and the session to keep alive.
    fn headless_screen() -> Option<(Screen, Connection)> {
        headless(None)
    }

    /// `virtual_size`: a new monitor of that size instead of the existing one.
    fn headless(virtual_size: Option<(u32, u32)>) -> Option<(Screen, Connection)> {
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
        Some((Screen::start(None, id, virtual_size).expect("the stream opened"), c))
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
        let Some((screen, _session)) = headless(Some((1920, 1080))) else { return };
        assert_eq!(screen.first_picture(Duration::from_secs(10)), Some((1920, 1080)));
    }

    #[test]
    #[ignore]
    fn headless_desktop_to_a_stand_in_tv() {
        let Some((mut screen, _session)) = headless_screen() else { return };
        screen.first_picture(Duration::from_secs(10)).expect("a picture");
        let (report, es) =
            crate::tv::miracast::tests::session_with_stand_in_from(&mut screen, Duration::from_millis(2000));
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut pictures = 0;
        for nal in openh264::nal_units(&es) {
            if let Ok(Some(_)) = decoder.decode(nal) {
                pictures += 1;
            }
        }
        eprintln!("sent {} pictures, {} decoded", report.frames, pictures);
        assert!(pictures >= 100, "{pictures} pictures decoded");
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
        let screen = Screen::ask(Choice::Screen).expect("screen shared");
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
                let mut s = Vec::with_capacity(16_000);
                for gy in 0..100 {
                    for gx in 0..160 {
                        let (x, y) = (gx * l.width / 160, gy * l.height / 100);
                        s.push(l.data[y * l.stride + x * 4 + 1]);
                    }
                }
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
