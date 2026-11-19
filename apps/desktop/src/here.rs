//! This computer as a screen: while turned on (the TVs page, off until
//! then), Kagaz lists this computer on the network as a Google Cast
//! screen and as a Miracast one; each sender (Kagaz on a phone or another
//! computer over Cast, Windows' own Cast over Miracast) is asked about
//! first in a small window on top (Allow or Decline, no answer in 30
//! seconds is Decline); its picture opens in a window of its own (the
//! page decodes it, as the cameras' player does) and its sound plays on
//! this computer's speakers.
//!
//! News for the pages comes as "here" events (the state) and "here-line"
//! (a line of what happens, for the picture's window).

use kagaz_core::tv::cast::stand_in::{self, Screen, Speaker, Who};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager};

const SETTING: &str = "screen-here";
const PORT: u16 = 8009;
const ASK: &str = "here-ask";
const VIEW: &str = "here-view";

/// How it stands, for the pages.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub on: bool,
    /// The name senders list this computer under.
    pub name: String,
    /// Why it could not be turned on.
    pub problem: Option<String>,
    /// Who asks now.
    pub asking: Option<String>,
    /// Whose screen shows now.
    pub showing: Option<String>,
}

struct Running {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

/// A sender's screen shows now (one at a time, whichever way it came).
static BUSY: AtomicBool = AtomicBool::new(false);

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static STATUS: Mutex<Option<Status>> = Mutex::new(None);
/// The answer to the question on screen: None while asked.
static ANSWER: (Mutex<Option<bool>>, Condvar) = (Mutex::new(None), Condvar::new());
/// The picture window's channel, once it asked for the pictures.
static PICTURES: Mutex<Option<Channel<InvokeResponseBody>>> = Mutex::new(None);
/// This screen's Stop.
static END_SESSION: AtomicBool = AtomicBool::new(false);

fn status() -> Status {
    STATUS.lock().unwrap().clone().unwrap_or_else(|| Status { name: name(), ..Default::default() })
}

fn tell(app: &AppHandle, change: impl FnOnce(&mut Status)) {
    let s = {
        let mut st = STATUS.lock().unwrap();
        let s = st.get_or_insert_with(|| Status { name: name(), ..Default::default() });
        change(s);
        s.clone()
    };
    let _ = app.emit("here", s);
}

/// The name this computer goes by on the network.
fn name() -> String {
    kagaz_core::tv::cast::channel::device_name()
}

#[tauri::command]
pub fn here_status() -> Status {
    status()
}

/// Turns it on or off (and remembers which).
#[tauri::command]
pub fn here_set(app: AppHandle, on: bool) -> Status {
    let _ = kagaz_core::paths::set_setting(SETTING, on.then_some("on"));
    if on {
        start(&app);
    } else {
        stop(&app);
    }
    status()
}

/// The question's answer, from its window.
#[tauri::command]
pub fn here_answer(allow: bool) {
    let (m, c) = &ANSWER;
    *m.lock().unwrap() = Some(allow);
    c.notify_all();
}

/// The picture window asks for the pictures.
#[tauri::command]
pub fn here_watch(channel: Channel<InvokeResponseBody>) {
    *PICTURES.lock().unwrap() = Some(channel);
}

/// The picture window's Stop: this sender's session ends, the computer
/// stays listed.
#[tauri::command]
pub fn here_stop() {
    END_SESSION.store(true, Ordering::SeqCst);
}

/// At Kagaz's start: on again only if it was left on.
pub fn resume(app: &AppHandle) {
    if kagaz_core::paths::setting(SETTING).as_deref() == Some("on") {
        start(app);
    }
}

fn start(app: &AppHandle) {
    let mut running = RUNNING.lock().unwrap();
    if running.is_some() {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();
    // Google Cast (Kagaz's senders).
    let (s, a) = (stop.clone(), app.clone());
    threads.extend(
        std::thread::Builder::new()
            .name("kagaz-here-cast".into())
            .spawn(move || {
                let n = name();
                let listener = match std::net::TcpListener::bind(("0.0.0.0", PORT)) {
                    Ok(l) => l,
                    Err(e) => {
                        tell(&a, |st| {
                            st.on = false;
                            st.problem = Some(format!("Another program uses port {PORT} (Google Cast's): {e}"));
                        });
                        return;
                    }
                };
                let announced = match stand_in::announce(&n, "Kagaz", PORT) {
                    Ok((announced, _)) => announced,
                    Err(e) => {
                        tell(&a, |st| {
                            st.on = false;
                            st.problem = Some(format!("This computer could not be listed on the network: {e}"));
                        });
                        return;
                    }
                };
                tell(&a, |st| {
                    st.on = true;
                    st.name = n;
                    st.problem = None;
                });
                let mut screen = Here::new(&a);
                if let Err(e) = stand_in::receive(listener, None, &s, &mut screen) {
                    tell(&a, |st| st.problem = Some(e));
                }
                drop(announced);
                tell(&a, |st| {
                    st.on = false;
                    st.asking = None;
                    st.showing = None;
                });
            })
            .ok(),
    );
    // Miracast over the network (Windows' own Cast). Without it, Cast
    // alone goes on.
    let (s, a) = (stop.clone(), app.clone());
    threads.extend(
        std::thread::Builder::new()
            .name("kagaz-here-miracast".into())
            .spawn(move || {
                use kagaz_core::tv::miracast::{mice, stand_in as wfd};
                let Ok(listener) = std::net::TcpListener::bind(("0.0.0.0", mice::PORT)) else {
                    eprintln!("here: port {} is taken; Miracast senders cannot reach this screen", mice::PORT);
                    return;
                };
                let Ok((announced, _)) = wfd::announce(&name()) else { return };
                let mut screen = Here::new(&a);
                if let Err(e) = wfd::receive(listener, &s, &mut screen) {
                    eprintln!("here: Miracast: {e}");
                }
                drop(announced);
            })
            .ok(),
    );
    *running = Some(Running { stop, threads });
}

fn stop(app: &AppHandle) {
    let r = RUNNING.lock().unwrap().take();
    if let Some(r) = r {
        r.stop.store(true, Ordering::SeqCst);
        here_answer(false);
        for t in r.threads {
            let _ = t.join();
        }
    }
    close(app, ASK);
    close(app, VIEW);
    tell(app, |st| {
        st.on = false;
        st.asking = None;
        st.showing = None;
    });
}

fn close(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.destroy();
    }
}

/// A small window on top: the question at the bottom right, the picture
/// in the middle of the screen at a good size.
fn open(app: &AppHandle, label: &str, page: &str, size: (f64, f64), ask: bool) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let mut b = tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App(page.into()))
        .title(if ask { "Kagaz: a screen to show here" } else { "Kagaz" })
        .inner_size(size.0, size.1)
        .resizable(!ask)
        .focused(true);
    if ask {
        b = b.always_on_top(true).skip_taskbar(true);
    }
    let Ok(w) = b.build() else { return };
    // Closing the picture's window is Stop.
    if !ask {
        w.on_window_event(|e| {
            if matches!(e, tauri::WindowEvent::Destroyed) {
                here_stop();
            }
        });
    }
    if ask {
        if let Ok(Some(m)) = w.primary_monitor() {
            let (ms, mp, scale) = (m.size(), m.position(), m.scale_factor());
            let (w_px, h_px) = ((size.0 * scale) as i32, (size.1 * scale) as i32);
            let _ = w.set_position(tauri::PhysicalPosition::new(
                mp.x + ms.width as i32 - w_px - (16.0 * scale) as i32,
                mp.y + ms.height as i32 - h_px - (64.0 * scale) as i32,
            ));
        }
    }
}

/// The app's screen: asks, shows, plays.
struct Here {
    app: AppHandle,
    speaker: Speaker,
    who: Option<String>,
    started: Instant,
    lines: u64,
}

impl Here {
    fn new(app: &AppHandle) -> Here {
        Here { app: app.clone(), speaker: Speaker::default(), who: None, started: Instant::now(), lines: 0 }
    }
}

/// How a sender is named to the person: its name, a phone as "A phone",
/// one that gives none by its address.
fn label(who: &Who) -> String {
    if who.name.is_empty() || who.name == "Kagaz" {
        format!("A device at {}", who.address)
    } else if who.name == "a phone" {
        "A phone".into()
    } else {
        who.name.clone()
    }
}

impl Screen for Here {
    fn ask(&mut self, who: &Who) -> bool {
        // One at a time: another device's screen shows already.
        if BUSY.load(Ordering::SeqCst) {
            return false;
        }
        let name = label(who);
        {
            *ANSWER.0.lock().unwrap() = None;
        }
        END_SESSION.store(false, Ordering::SeqCst);
        tell(&self.app, |st| st.asking = Some(name.clone()));
        let a = self.app.clone();
        let _ = self.app.run_on_main_thread(move || open(&a, ASK, "here-ask.html", (380.0, 170.0), true));
        let (m, c) = &ANSWER;
        let end = Instant::now() + Duration::from_secs(30);
        let mut answer = m.lock().unwrap();
        while answer.is_none() && Instant::now() < end {
            let (g, _) = c.wait_timeout(answer, Duration::from_millis(250)).unwrap();
            answer = g;
        }
        let allow = answer.take().unwrap_or(false);
        drop(answer);
        let a = self.app.clone();
        let _ = self.app.run_on_main_thread(move || close(&a, ASK));
        tell(&self.app, |st| st.asking = None);
        if allow && BUSY.swap(true, Ordering::SeqCst) {
            return false;
        }
        if allow {
            self.who = Some(name.clone());
            self.started = Instant::now();
            *PICTURES.lock().unwrap() = None;
            tell(&self.app, |st| st.showing = Some(name));
            let a = self.app.clone();
            let _ = self.app.run_on_main_thread(move || open(&a, VIEW, "here-view.html", (1120.0, 680.0), false));
        }
        allow
    }

    fn picture(&mut self, data: Vec<u8>, key: bool) {
        let timestamp = (self.started.elapsed().as_micros() as u64 * 9 / 100) as u32;
        let p = PICTURES.lock().unwrap();
        if let Some(ch) = p.as_ref() {
            let _ = super::media::send(ch, &kagaz_core::media::Media::Video { data, key, timestamp });
        }
    }

    fn sound(&mut self, packet: Vec<u8>, aac: bool) {
        self.speaker.play(&packet, aac);
    }

    fn pcm(&mut self, samples: Vec<i16>) {
        self.speaker.play_pcm(&samples);
    }

    fn say(&mut self, line: String) {
        eprintln!("here: {line}");
        self.lines += 1;
        let _ = self.app.emit("here-line", line);
    }

    fn ended(&mut self) {
        self.speaker.close();
        if self.who.take().is_some() {
            BUSY.store(false, Ordering::SeqCst);
            if let Some(ch) = PICTURES.lock().unwrap().take() {
                let _ = super::media::send(&ch, &kagaz_core::media::Media::Ended("The device stopped showing its screen.".into()));
            }
            let a = self.app.clone();
            let _ = self.app.run_on_main_thread(move || close(&a, VIEW));
        }
        tell(&self.app, |st| {
            st.asking = None;
            st.showing = None;
        });
    }

    fn keep(&mut self) -> bool {
        !END_SESSION.swap(false, Ordering::SeqCst)
    }
}
