//! The doorbell in the window: one thread listens for the door station's
//! call, the window shows the ring, and the person answers, opens the door
//! or hangs up from it. Everything the thread learns goes out on the
//! `doorbell-state` event; the window never polls.

use kagaz_core::intercom::incoming::{Incoming, Listener};
use kagaz_core::intercom::stations::{self, Station, Stations};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Something the person did in the window, for the listener thread.
#[derive(Debug, Clone, Copy)]
enum Command {
    Answer,
    Decline,
    OpenDoor,
    Hangup,
}

/// One past ring, newest first in the status.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub at: i64,
    pub caller: String,
    pub from: String,
    /// "answered", "door opened", "declined", "missed".
    pub outcome: String,
}

/// Where the doorbell stands, as the window shows it.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Status {
    pub listening: bool,
    pub port: u16,
    /// The `ip:port` callers reach us at (what to put in the station's Direct IP list).
    pub local: String,
    /// Why we are not listening, or the last trouble.
    pub message: String,
    /// "idle", "ringing" or "in_call".
    pub phase: String,
    pub caller: String,
    pub caller_uri: String,
    /// The calling station's address.
    pub from: String,
    pub has_video: bool,
    /// When the ring / call began (unix seconds).
    pub since: i64,
    pub door_opened_at: Option<i64>,
    pub recent: Vec<Event>,
}

pub struct DoorbellState {
    status: Mutex<Status>,
    commands: Mutex<Option<mpsc::Sender<Command>>>,
    stop: Mutex<Option<Arc<AtomicBool>>>,
}

impl DoorbellState {
    pub fn new() -> DoorbellState {
        DoorbellState {
            status: Mutex::new(Status {
                phase: "idle".into(),
                ..Default::default()
            }),
            commands: Mutex::new(None),
            stop: Mutex::new(None),
        }
    }
}

const RECENT_KEPT: usize = 30;
/// How long we let a call ring before giving it up as missed.
const RING_FOR: Duration = Duration::from_secs(120);

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn update(app: &AppHandle, f: impl FnOnce(&mut Status)) {
    let state = app.state::<DoorbellState>();
    let snapshot = {
        let mut guard = state.status.lock().unwrap();
        f(&mut guard);
        guard.clone()
    };
    let _ = app.emit("doorbell-state", &snapshot);
}

/// Bring the window forward for a ring.
#[cfg(desktop)]
fn attention(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.request_user_attention(Some(tauri::UserAttentionType::Critical));
        let _ = win.set_focus();
    }
}

/// A phone shows the app's own screen; the ring sound plays in the page.
#[cfg(mobile)]
fn attention(_app: &AppHandle) {}

/// Start (or restart) listening on `port`. Safe to call again: the old
/// thread stops first.
pub fn start(app: &AppHandle, port: u16) {
    let state = app.state::<DoorbellState>();
    if let Some(old) = state.stop.lock().unwrap().take() {
        old.store(true, Ordering::SeqCst);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    *state.stop.lock().unwrap() = Some(stop.clone());
    *state.commands.lock().unwrap() = Some(tx);
    let app = app.clone();
    std::thread::Builder::new()
        .name("doorbell".into())
        .spawn(move || run(app, port, rx, stop))
        .expect("spawn the doorbell thread");
}

fn run(app: AppHandle, port: u16, rx: mpsc::Receiver<Command>, stop: Arc<AtomicBool>) {
    // The old thread may still hold the port for a moment.
    let mut listener = None;
    for _ in 0..20 {
        match Listener::bind(port) {
            Ok(l) => {
                listener = Some(l);
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    let listener = match listener {
        Some(l) => l,
        None => {
            update(&app, |s| {
                s.listening = false;
                s.port = port;
                s.phase = "idle".into();
                s.message = format!(
                    "Cannot listen on UDP port {port}: something else on this computer has it (another SIP program, or `kagaz intercom listen`)."
                );
            });
            return;
        }
    };
    let local = listener.local().to_string();
    update(&app, |s| {
        s.listening = true;
        s.port = port;
        s.local = local;
        s.message.clear();
        s.phase = "idle".into();
    });

    while !stop.load(Ordering::SeqCst) {
        // Stray clicks from before this call mean nothing now.
        while rx.try_recv().is_ok() {}
        let call = match listener.next_call_within(Duration::from_millis(300)) {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(e) => {
                update(&app, |s| {
                    s.listening = false;
                    s.message = format!("Stopped listening: {e}");
                });
                return;
            }
        };
        let outcome = handle_call(&app, &call, &rx, &stop);
        let event = Event {
            at: now(),
            caller: call.caller(),
            from: call.remote().ip().to_string(),
            outcome: outcome.to_string(),
        };
        update(&app, |s| {
            s.phase = "idle".into();
            s.caller.clear();
            s.caller_uri.clear();
            s.from.clear();
            s.has_video = false;
            s.since = 0;
            s.door_opened_at = None;
            s.recent.insert(0, event);
            s.recent.truncate(RECENT_KEPT);
        });
    }
    update(&app, |s| {
        s.listening = false;
        s.phase = "idle".into();
    });
}

/// Ring, then do what the window says, until the call is over. Returns
/// how it went, for the log.
fn handle_call(
    app: &AppHandle,
    call: &Incoming,
    rx: &mpsc::Receiver<Command>,
    stop: &AtomicBool,
) -> &'static str {
    let _ = call.ringing();
    let media = call.media();
    update(app, |s| {
        s.phase = "ringing".into();
        s.caller = call.caller();
        s.caller_uri = call.caller_uri();
        s.from = call.remote().ip().to_string();
        s.has_video = media.has_video();
        s.since = now();
        s.door_opened_at = None;
        s.message.clear();
    });
    attention(app);

    let started = Instant::now();
    let decision = loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Answer) => break Some(true),
            Ok(Command::Decline) => break Some(false),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break None,
        }
        match call.poll_end(Duration::from_millis(50)) {
            Ok(true) => break None, // the caller gave up
            Ok(false) => {}
            Err(_) => break None,
        }
        if stop.load(Ordering::SeqCst) || started.elapsed() > RING_FOR {
            break None;
        }
    };

    match decision {
        None => "missed",
        Some(false) => {
            let _ = call.decline();
            "declined"
        }
        Some(true) => match call.answer() {
            Err(e) => {
                update(app, |s| s.message = format!("Could not answer: {e}"));
                "missed"
            }
            Ok(mut active) => {
                let mut outcome = "answered";
                update(app, |s| {
                    s.phase = "in_call".into();
                    s.since = now();
                });
                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(Command::OpenDoor) => match active.open_door() {
                            Ok(()) => {
                                outcome = "door opened";
                                update(app, |s| s.door_opened_at = Some(now()));
                            }
                            Err(e) => {
                                update(app, |s| s.message = format!("Could not open the door: {e}"))
                            }
                        },
                        Ok(Command::Hangup) => {
                            let _ = active.hangup();
                            break;
                        }
                        Ok(_) => {}
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => {
                            let _ = active.hangup();
                            break;
                        }
                    }
                    match active.poll_end(Duration::from_millis(50)) {
                        Ok(true) => break,
                        Ok(false) => {}
                        Err(_) => break,
                    }
                    if stop.load(Ordering::SeqCst) {
                        let _ = active.hangup();
                        break;
                    }
                }
                outcome
            }
        },
    }
}

fn send(state: &tauri::State<'_, DoorbellState>, cmd: Command) -> Result<(), String> {
    let guard = state.commands.lock().unwrap();
    match guard.as_ref() {
        Some(tx) => tx
            .send(cmd)
            .map_err(|_| "the doorbell is not listening".to_string()),
        None => Err("the doorbell is not listening".to_string()),
    }
}

// ---------- commands ----------

#[tauri::command]
pub fn doorbell_status(state: tauri::State<'_, DoorbellState>) -> Status {
    state.status.lock().unwrap().clone()
}

#[tauri::command]
pub fn doorbell_answer(state: tauri::State<'_, DoorbellState>) -> Result<(), String> {
    send(&state, Command::Answer)
}

#[tauri::command]
pub fn doorbell_decline(state: tauri::State<'_, DoorbellState>) -> Result<(), String> {
    send(&state, Command::Decline)
}

#[tauri::command]
pub fn doorbell_open_door(state: tauri::State<'_, DoorbellState>) -> Result<(), String> {
    send(&state, Command::OpenDoor)
}

#[tauri::command]
pub fn doorbell_hangup(state: tauri::State<'_, DoorbellState>) -> Result<(), String> {
    send(&state, Command::Hangup)
}

/// Listen on another port (saved), restarting the listener.
#[tauri::command]
pub fn doorbell_listen(app: AppHandle, port: u16) -> Result<(), String> {
    let mut list = Stations::load();
    list.listen_port = port;
    list.save().map_err(|e| e.to_string())?;
    start(&app, port);
    Ok(())
}

#[tauri::command]
pub fn doorbell_stations() -> Stations {
    Stations::load()
}

#[tauri::command]
pub fn doorbell_station_save(name: String, address: String, port: u16) -> Result<Stations, String> {
    let address = address.trim().to_string();
    if address.parse::<std::net::Ipv4Addr>().is_err() {
        return Err(format!(
            "\"{address}\" is not an IPv4 address (like 192.168.1.20)"
        ));
    }
    let name = name.trim();
    let name = if name.is_empty() {
        "Door station"
    } else {
        name
    };
    let mut list = Stations::load();
    list.put(Station {
        name: name.to_string(),
        address,
        port: if port == 0 { 5060 } else { port },
    });
    list.save().map_err(|e| e.to_string())?;
    Ok(list)
}

#[tauri::command]
pub fn doorbell_station_remove(address: String) -> Result<Stations, String> {
    let mut list = Stations::load();
    list.remove(&address);
    list.save().map_err(|e| e.to_string())?;
    Ok(list)
}

/// Ask one station whether it answers SIP right now.
#[tauri::command]
pub async fn doorbell_probe(address: String, port: u16) -> Result<Option<stations::Found>, String> {
    super::blocking(move || {
        stations::probe(&address, port, Duration::from_millis(1500)).map_err(|e| e.to_string())
    })
    .await
}

/// Find every SIP endpoint on the local networks.
#[tauri::command]
pub async fn doorbell_discover(port: u16) -> Result<Vec<stations::Found>, String> {
    super::blocking(move || {
        stations::discover(port, Duration::from_millis(2500)).map_err(|e| e.to_string())
    })
    .await
}
