//! Each session with a computer or phone in a window of its own, on a
//! computer: Connect opens a window for the device (session.html: the
//! viewer and nothing else of Kagaz), several can be open at once, and
//! Kagaz's main window stays free for everything else. Closing a
//! session's window ends that session, after a question; closing the
//! main window ends them all after one question, or, where the person
//! chose so (remote.toml, `windows.keep_open`), leaves them open with the
//! main window out of sight until it is asked for again.
//!
//! A phone has one window: there a session stays inside Kagaz.

use kagaz_core::remote::store::Devices;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

pub const MAIN: &str = "main";
const PREFIX: &str = "session-";

/// A session's window: which, and whose screen it shows.
#[derive(Debug, Clone, Serialize)]
pub struct Shown {
    pub label: String,
    pub device: String,
    pub name: String,
}

#[derive(Default)]
pub struct SessionWindows {
    list: Mutex<Vec<Shown>>,
    next: AtomicU64,
    /// The main window was closed with sessions left open: it is out of
    /// sight, and Kagaz ends with the last session's window.
    main_hidden: AtomicBool,
}

impl SessionWindows {
    fn now(&self) -> Vec<Shown> {
        self.list.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

pub fn is_session(label: &str) -> bool {
    label.starts_with(PREFIX)
}

/// Every window hears which sessions have one (the device list says so).
fn tell(app: &AppHandle) {
    let _ = app.emit("remote-windows", app.state::<SessionWindows>().now());
}

#[cfg(desktop)]
fn bring(window: &tauri::WebviewWindow) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

#[tauri::command]
pub fn remote_windows(state: tauri::State<'_, SessionWindows>) -> Vec<Shown> {
    state.now()
}

/// A window for a session with `device` (its id, name or the start of
/// its name). Where it has one already, that one comes to the front,
/// unless another is asked for (`again`): the device decides what it
/// makes of a second connection. Says which window.
#[tauri::command]
pub async fn remote_window_open(app: AppHandle, device: String, again: bool) -> Result<String, String> {
    #[cfg(desktop)]
    {
        let found = Devices::load(&Devices::default_path()).and_then(|d| d.find(&device).cloned()).map_err(|e| e.to_string())?;
        let state = app.state::<SessionWindows>();
        if !again {
            let open = state.now().into_iter().find(|w| w.device == found.id);
            if let Some(window) = open.as_ref().and_then(|w| app.get_webview_window(&w.label)) {
                bring(&window);
                return Ok(window.label().to_string());
            }
        }
        let label = format!("{PREFIX}{}", state.next.fetch_add(1, Ordering::SeqCst) + 1);
        // The page is told whose screen it is before it starts.
        let about = serde_json::json!({ "device": found.id, "label": label });
        tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App("session.html".into()))
            .title(format!("{} · Kagaz", found.name))
            .inner_size(1120.0, 720.0)
            .min_inner_size(480.0, 360.0)
            .initialization_script(format!("window.__KAGAZ_SESSION__ = {about};"))
            .build()
            .map_err(|e| e.to_string())?;
        if let Ok(mut list) = state.list.lock() {
            list.push(Shown { label: label.clone(), device: found.id.clone(), name: found.name.clone() });
        }
        tell(&app);
        Ok(label)
    }
    #[cfg(not(desktop))]
    {
        let _ = (app, device, again, Devices::default_path);
        Err("a phone shows a session inside Kagaz".into())
    }
}

/// Kagaz's main window to the front (from a session's window: its Kagaz
/// button, or Answer on a call at the door).
#[tauri::command]
pub fn remote_main_show(app: AppHandle) {
    #[cfg(desktop)]
    if let Some(window) = app.get_webview_window(MAIN) {
        app.state::<SessionWindows>().main_hidden.store(false, Ordering::Release);
        bring(&window);
    }
    #[cfg(not(desktop))]
    let _ = app;
}

/// The person said yes to closing Kagaz with sessions open: every
/// session's window goes, then the main one, and Kagaz with it.
#[tauri::command]
pub fn remote_close_all(app: AppHandle) {
    for shown in app.state::<SessionWindows>().now() {
        if let Some(window) = app.get_webview_window(&shown.label) {
            let _ = window.destroy();
        }
    }
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.destroy();
    }
}

/// A window is being closed, or is gone (main.rs passes every window's
/// events here).
pub fn on_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    #[cfg(desktop)]
    {
        let app = window.app_handle();
        let label = window.label();
        match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                // A session shown in this very window (a session's own
                // window; or the main one, where sessions have none):
                // its page asks, and closes the window on a yes.
                if super::remote_view::open_in(app, label) {
                    api.prevent_close();
                    let _ = app.emit_to(label, "remote-close-asked", ());
                } else if label == MAIN && super::share::keeps_running() {
                    // Sharing this computer is on: Kagaz waits in the tray,
                    // sessions and all, and sharing goes on.
                    api.prevent_close();
                    let _ = window.hide();
                } else if label == MAIN {
                    let state = app.state::<SessionWindows>();
                    let others = state.now();
                    if others.is_empty() {
                        return;
                    }
                    api.prevent_close();
                    let keep = Devices::load(&Devices::default_path()).map(|d| d.windows.keep_open).unwrap_or(false);
                    if keep {
                        state.main_hidden.store(true, Ordering::Release);
                        let _ = window.hide();
                    } else {
                        let names: Vec<String> = others.into_iter().map(|w| w.name).collect();
                        let _ = app.emit_to(MAIN, "main-close-asked", names);
                    }
                }
            }
            tauri::WindowEvent::Destroyed if is_session(label) => {
                let state = app.state::<SessionWindows>();
                let left = {
                    let Ok(mut list) = state.list.lock() else { return };
                    list.retain(|w| w.label != label);
                    list.len()
                };
                super::remote_view::end_in(app, label);
                tell(app);
                if left == 0 && state.main_hidden.load(Ordering::Acquire) && !super::share::keeps_running() {
                    app.exit(0);
                }
            }
            _ => {}
        }
    }
    #[cfg(not(desktop))]
    let _ = (window, event);
}
