//! A session's keyboard shortcuts on Linux, through the desktop's
//! GlobalShortcuts portal (GNOME 48 and later, KDE Plasma): the desktop
//! asks the owner once to allow them and lets them change the keys in its
//! own settings. They are bound only while a session runs.
//!
//!   Ctrl+Alt+H  hide everything on the TV, or show it again
//!   Ctrl+Alt+S  the sound on the TV, or back on this computer

use super::capture::{portal_request, token, v, PORTAL, PORTAL_PATH};
use dbus::arg::PropMap;
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";

/// The shortcuts: (id, what it does, the keys asked for).
pub const KEYS: [(&str, &str, &str); 2] = [
    ("all", "Hide everything on the TV, or show it again", "CTRL+ALT+H"),
    ("sound", "Sound on the TV, or back on this computer", "CTRL+ALT+S"),
];

/// Bound while it lives.
pub struct Keys {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Keys {
    /// Binds the shortcuts; `pressed` hears a shortcut's id (on a thread
    /// of its own). `said` hears why they could not be bound.
    pub fn start(pressed: impl Fn(&str) + Send + 'static, said: impl Fn(String) + Send + 'static) -> Keys {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("kagaz-tv-keys".into())
            .spawn(move || {
                if let Err(e) = run(&pressed, &s) {
                    said(e);
                }
            })
            .ok();
        Keys { stop, thread }
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(pressed: &dyn Fn(&str), stop: &AtomicBool) -> Result<(), String> {
    let c = Connection::new_session().map_err(|e| format!("no desktop session bus: {e}"))?;
    {
        use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
        let proxy = c.with_proxy(PORTAL, PORTAL_PATH, Duration::from_secs(5));
        let _: u32 = proxy
            .get(SHORTCUTS, "version")
            .map_err(|_| "this desktop has no shortcuts portal (GNOME 48 or later, or KDE Plasma)".to_string())?;
    }
    let session_token = token();
    let created = portal_request(
        &c,
        "CreateSession",
        &token(),
        |p, mut o| {
            o.insert("session_handle_token".into(), v(session_token.clone()));
            p.method_call(SHORTCUTS, "CreateSession", (o,)).map(|(_,): (dbus::Path<'static>,)| ())
        },
        Duration::from_secs(10),
    )?;
    let session: String = created
        .get("session_handle")
        .and_then(|x| x.0.as_str().map(str::to_string))
        .ok_or("the shortcuts portal gave no session")?;
    let session = dbus::Path::from(session);
    let wanted: Vec<(String, PropMap)> = KEYS
        .iter()
        .map(|(id, what, keys)| {
            let mut m: PropMap = HashMap::new();
            m.insert("description".into(), v(what.to_string()));
            m.insert("preferred_trigger".into(), v(keys.to_string()));
            (id.to_string(), m)
        })
        .collect();
    let s1 = session.clone();
    // The desktop may ask the owner first; there is time to answer.
    portal_request(
        &c,
        "BindShortcuts",
        &token(),
        |p, o| p.method_call(SHORTCUTS, "BindShortcuts", (s1, wanted, "", o)).map(|(_,): (dbus::Path<'static>,)| ()),
        Duration::from_secs(120),
    )
    .map_err(|_| "the shortcuts were not allowed".to_string())?;
    let heard: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let h = heard.clone();
    let mine = session.clone();
    let rule = MatchRule::new_signal(SHORTCUTS, "Activated");
    c.add_match(rule, move |(s, id, _, _): (dbus::Path<'static>, String, u64, PropMap), _, _| {
        if s == mine {
            h.lock().unwrap().push(id);
        }
        true
    })
    .map_err(|e| format!("the shortcuts could not be heard: {e}"))?;
    while !stop.load(Ordering::SeqCst) {
        let _ = c.process(Duration::from_millis(100));
        let ids: Vec<String> = std::mem::take(&mut *heard.lock().unwrap());
        for id in ids {
            pressed(&id);
        }
    }
    let proxy = c.with_proxy(PORTAL, session, Duration::from_secs(2));
    let _: Result<(), _> = proxy.method_call("org.freedesktop.portal.Session", "Close", ());
    Ok(())
}
