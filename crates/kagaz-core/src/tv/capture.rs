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

/// What can be shown: whole screens, single windows, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Screen,
    Window,
    Either,
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

    // 1 monitors, 2 windows; the cursor drawn into the picture (2) when
    // the desktop can.
    let types = match choice {
        Choice::Screen => 1,
        Choice::Window => 2,
        Choice::Either => 3,
    } & available;
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
        let mut s = Screen::from_node(Some(fd), node)?;
        s._portal = Some(portal);
        Ok(s)
    }

    /// Takes pictures from a PipeWire stream: over `fd` (from the portal),
    /// or this session's PipeWire when `None` (tests, with a stream made
    /// some other way).
    pub fn from_node(fd: Option<OwnedFd>, node: u32) -> Result<Screen, String> {
        let latest = Arc::new(Mutex::new(Latest::default()));
        let arrived = Arc::new(AtomicU64::new(0));
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let l = latest.clone();
        let count = arrived.clone();
        let thread = std::thread::Builder::new()
            .name("kagaz-screen".into())
            .spawn(move || {
                if let Err(e) = run_stream(fd, node, l, count, quit_rx, &ready_tx) {
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
        let l = self.latest.lock().unwrap();
        if l.seq != self.last_seq && l.seq > 0 {
            if let Some(layout) = l.layout {
                scale(&l.data, l.width, l.height, l.stride, layout, &mut self.scaled, width, height);
            }
            self.last_seq = l.seq;
        }
        drop(l);
        &self.scaled
    }
}

/// Scales with a 2x2 average at each target pixel (sharper than nearest
/// for text, cheap enough for 60 pictures a second), keeping the shape.
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
    let half_x = (sw / fw).max(1) / 2;
    let half_y = (sh / fh).max(1) / 2;
    for y in 0..fh {
        let sy0 = (y * sh / fh).min(sh - 1);
        let sy1 = (sy0 + half_y).min(sh - 1);
        let row0 = &src[sy0 * stride..];
        let row1 = &src[sy1 * stride..];
        let out = &mut dst[((oy + y) * dw + ox) * 4..((oy + y) * dw + ox + fw) * 4];
        for (x, px) in out.chunks_exact_mut(4).enumerate() {
            let sx0 = (x * sw / fw).min(sw - 1) * 4;
            let sx1 = ((x * sw / fw + half_x).min(sw - 1)) * 4;
            for (c, from) in [(0usize, r), (1, 1), (2, b)] {
                let sum = row0[sx0 + from] as u16 + row0[sx1 + from] as u16 + row1[sx0 + from] as u16 + row1[sx1 + from] as u16;
                px[c] = (sum / 4) as u8;
            }
            px[3] = 255;
        }
    }
}

fn run_stream(
    fd: Option<OwnedFd>,
    node: u32,
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
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 }
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
        let (stream,): (dbus::Path<'static>,) = s
            .method_call(&format!("{M}.Session"), "RecordMonitor", ("", props))
            .expect("the virtual monitor recorded");
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
        Some((Screen::from_node(None, id).expect("the stream opened"), c))
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
