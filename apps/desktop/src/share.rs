//! Sharing this computer (docs/remote-connection.md, step 2): Kagaz's
//! window runs `kagaz-share` while sharing is on, and only then: off, as
//! it is until the person turns it on, nothing of it runs or listens
//! (one use of Kagaz must not cloud another). The program says what
//! happens in `@`-lines of JSON; this side keeps the state, tells the
//! pages, and opens the question and the sign as their own small
//! windows, on top of whatever is open.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

/// The person's choices, kept with Kagaz's other settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Share this computer: off until turned on.
    pub on: bool,
    pub port: u16,
    /// Where it listens: "all" (the local network and Tailscale),
    /// "tailscale" (only its Tailscale address), "local" (this computer).
    pub listen: String,
    /// A newly paired device is asked about each time (otherwise it
    /// connects with the PIN, without asking); each can have its own.
    pub ask: bool,
    /// After a session: "unused" (lock if nobody here used it), "always",
    /// "never".
    pub lock: String,
    /// Keep a record of sessions, on this computer only.
    pub record: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { on: false, port: 3390, listen: "all".into(), ask: true, lock: "unused".into(), record: true }
    }
}

fn settings_path() -> PathBuf {
    kagaz_core::remote::Devices::default_path().with_file_name("share.json")
}

fn load() -> Settings {
    std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// What the pages are told.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    /// This computer can share (Windows, with kagaz-share next to Kagaz).
    pub supported: bool,
    pub settings: Settings,
    pub running: bool,
    pub listening: Option<String>,
    pub name: String,
    /// Its addresses, for the device that pairs.
    pub addresses: Vec<(String, String)>,
    pub code: Option<String>,
    /// The words of a pairing waiting for the person here, and who.
    pub words: Option<(String, String)>,
    pub devices: Vec<Value>,
    pub pin: bool,
    /// The device connected now.
    pub session: Option<Value>,
    pub control: bool,
    pub asking: Vec<Value>,
    pub problem: Option<String>,
}

#[derive(Default)]
pub struct Share {
    child: Mutex<Option<(Child, ChildStdin)>>,
    status: Mutex<Status>,
}

fn log(line: &str) {
    kagaz_core::cameras::tapo::serve::log(&format!("share: {line}"));
}

/// The program, next to Kagaz (or named by KAGAZ_SHARE_BIN for testing).
fn program() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KAGAZ_SHARE_BIN") {
        return Some(p.into());
    }
    let here = std::env::current_exe().ok()?.parent()?.to_path_buf();
    ["kagaz-share.exe", "kagaz-share-windows-x64.exe", "kagaz-share"].iter().map(|n| here.join(n)).find(|p| p.is_file())
}

/// Sharing is offered where the screen can be shared: Windows for now
/// (and anywhere with the test card, for testing).
fn supported() -> bool {
    let test = std::env::var_os("KAGAZ_SHARE_PATTERN").is_some();
    (cfg!(windows) || test) && program().is_some()
}

/// This computer's address towards a network, found without sending
/// anything: the system says which of its addresses it would use.
fn address_towards(to: &str) -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(to).ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then(|| ip.to_string())
}

fn addresses() -> Vec<(String, String)> {
    let mut out = Vec::new();
    // Tailscale's own address answers through Tailscale's network only.
    let tailscale = address_towards("100.100.100.100:53").filter(|ip| ip.starts_with("100."));
    if let Some(local) = address_towards("192.0.2.1:9") {
        if Some(&local) != tailscale.as_ref() {
            out.push(("Local network".to_string(), local));
        }
    }
    if let Some(t) = tailscale {
        out.push(("Tailscale".to_string(), t));
    }
    out
}

impl Share {
    pub fn status(&self) -> Status {
        let mut s = self.status.lock().unwrap().clone();
        s.supported = supported();
        s.settings = load();
        s.addresses = if s.running { addresses() } else { Vec::new() };
        s
    }

    fn tell(&self, app: &AppHandle) {
        let _ = app.emit("share", self.status());
    }

    /// Starts the program with the settings, unless it runs.
    pub fn start(&self, app: &AppHandle) -> Result<(), String> {
        let mut child = self.child.lock().unwrap();
        if child.is_some() {
            return Ok(());
        }
        let program = program().ok_or("kagaz-share is not next to Kagaz")?;
        let settings = load();
        let mut cmd = Command::new(program);
        cmd.arg("--driven").args(["--port", &settings.port.to_string()]).args(["--lock", &settings.lock]);
        match settings.listen.as_str() {
            "local" => {
                cmd.arg("--local");
            }
            "tailscale" => {
                let ip = address_towards("100.100.100.100:53").filter(|ip| ip.starts_with("100.")).ok_or("Tailscale is not on here")?;
                cmd.args(["--bind", &ip]);
            }
            _ => {}
        }
        if !settings.record {
            cmd.arg("--no-record");
        }
        if std::env::var_os("KAGAZ_SHARE_PATTERN").is_some() {
            cmd.args(["--pattern", "--tone"]);
            if let Some(keep) = std::env::var_os("KAGAZ_SHARE_KEEP") {
                cmd.arg("--keep").arg(keep);
            }
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window of its own.
            cmd.creation_flags(0x0800_0000);
        }
        let mut c = cmd.spawn().map_err(|e| format!("kagaz-share did not start: {e}"))?;
        let stdin = c.stdin.take().ok_or("no way to kagaz-share")?;
        let stdout = c.stdout.take().ok_or("no word from kagaz-share")?;
        *child = Some((c, stdin));
        drop(child);
        {
            let mut st = self.status.lock().unwrap();
            *st = Status { running: true, ..Status::default() };
        }
        log("started");
        let app = app.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match line.strip_prefix('@').and_then(|j| serde_json::from_str::<Value>(j).ok()) {
                    Some(ev) => heard(&app, ev),
                    None => log(&line),
                }
            }
            let share = app.state::<Share>();
            let ended = share.child.lock().unwrap().take();
            if let Some((mut c, _)) = ended {
                let _ = c.wait();
            }
            {
                let mut st = share.status.lock().unwrap();
                let was = st.running;
                *st = Status::default();
                if was && load().on {
                    st.problem = Some("Sharing stopped unexpectedly. Turn it off and on to start it again.".into());
                }
            }
            close_window(&app, "share-sign");
            close_window(&app, "share-ask");
            log("stopped");
            share.tell(&app);
        });
        Ok(())
    }

    pub fn stop(&self) {
        if let Some((mut c, stdin)) = self.child.lock().unwrap().take() {
            // Its input closing ends it; then make sure.
            drop(stdin);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    pub fn send(&self, line: &str) -> Result<(), String> {
        let mut child = self.child.lock().unwrap();
        let (_, stdin) = child.as_mut().ok_or("sharing is off")?;
        writeln!(stdin, "{line}").and_then(|_| stdin.flush()).map_err(|e| e.to_string())
    }
}

fn close_window(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.destroy();
    }
}

/// A small window of its own, on top of everything: the question, or the
/// sign while a device is connected.
fn open_window(app: &AppHandle, label: &str, page: &str, size: (f64, f64), top: bool) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let mut b = tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App(page.into()))
        .title("Kagaz")
        .inner_size(size.0, size.1)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(top)
        .focused(!top);
    if top {
        b = b.decorations(false);
    }
    let Ok(w) = b.build() else { return };
    // The sign at the top middle of the main screen, the question at the
    // bottom right.
    if let Ok(Some(m)) = w.primary_monitor() {
        let (ms, mp, scale) = (m.size(), m.position(), m.scale_factor());
        let (w_px, h_px) = ((size.0 * scale) as i32, (size.1 * scale) as i32);
        let (x, y) = if top {
            (mp.x + (ms.width as i32 - w_px) / 2, mp.y + (10.0 * scale) as i32)
        } else {
            (mp.x + ms.width as i32 - w_px - (16.0 * scale) as i32, mp.y + ms.height as i32 - h_px - (64.0 * scale) as i32)
        };
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// One event from the program.
fn heard(app: &AppHandle, ev: Value) {
    let share = app.state::<Share>();
    let kind = ev["kind"].as_str().unwrap_or("").to_string();
    {
        let mut st = share.status.lock().unwrap();
        match kind.as_str() {
            "listening" => {
                st.listening = ev["address"].as_str().map(String::from);
                st.name = ev["name"].as_str().unwrap_or("").to_string();
                st.problem = None;
            }
            "devices" => {
                st.devices = ev["devices"].as_array().cloned().unwrap_or_default();
                st.pin = ev["pin"].as_bool().unwrap_or(false);
            }
            "code" => {
                st.code = ev["code"].as_str().map(String::from);
                st.words = None;
            }
            "words" => {
                st.words = Some((ev["words"].as_str().unwrap_or("").into(), ev["name"].as_str().unwrap_or("").into()));
                st.code = None;
            }
            "paired" | "not-paired" => {
                st.words = None;
                st.code = None;
            }
            "ask" => st.asking.push(ev.clone()),
            "asked" => {
                let id = ev["id"].as_str().unwrap_or("");
                st.asking.retain(|a| a["id"].as_str() != Some(id));
            }
            "connected" => {
                st.control = ev["control"].as_bool().unwrap_or(false);
                st.session = Some(ev.clone());
            }
            "mode" => st.control = ev["control"].as_bool().unwrap_or(false),
            "left" => {
                st.session = None;
                st.control = false;
            }
            _ => {}
        }
    }
    match kind.as_str() {
        "ask" => open_window(app, "share-ask", "share-ask.html", (380.0, 250.0), false),
        "asked" => {
            if share.status.lock().unwrap().asking.is_empty() {
                close_window(app, "share-ask");
            }
        }
        "connected" => open_window(app, "share-sign", "share-sign.html", (560.0, 52.0), true),
        "left" => close_window(app, "share-sign"),
        "paired" => {
            let _ = share.send("devices");
        }
        _ => {}
    }
    share.tell(app);
}

/// At Kagaz's start: sharing goes on again only if the person left it on.
pub fn resume(app: &AppHandle) {
    if load().on && supported() {
        if let Err(e) = app.state::<Share>().start(app) {
            log(&e);
        }
    }
}

#[tauri::command]
pub fn share_status(app: AppHandle) -> Status {
    app.state::<Share>().status()
}

#[tauri::command]
pub fn share_settings(app: AppHandle, settings: Settings) -> Result<Status, String> {
    let before = load();
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    let share = app.state::<Share>();
    if !settings.on {
        share.stop();
        share.status.lock().unwrap().problem = None;
    } else if !supported() {
        return Err("This computer cannot share its screen yet: Kagaz shares Windows computers for now.".into());
    } else {
        let restart = before.port != settings.port || before.listen != settings.listen || before.record != settings.record;
        if restart {
            share.stop();
        }
        if before.lock != settings.lock {
            let _ = share.send(&format!("lock {}", settings.lock));
        }
        share.start(&app)?;
    }
    share.tell(&app);
    Ok(share.status())
}

/// A line to the program, from the pages: only the commands it has.
#[tauri::command]
pub fn share_send(app: AppHandle, line: String) -> Result<(), String> {
    let first = line.split_whitespace().next().unwrap_or("");
    const ALLOWED: [&str; 12] = ["pair", "stop", "confirm", "cancel", "pin", "allow", "forget", "answer", "watch", "control", "end", "devices"];
    if !ALLOWED.contains(&first) || line.contains('\n') {
        return Err("not a sharing command".into());
    }
    app.state::<Share>().send(&line)
}

/// The record of sessions, newest first.
#[tauri::command]
pub fn share_sessions() -> Vec<Value> {
    let keep = std::env::var_os("KAGAZ_SHARE_KEEP").map(PathBuf::from).unwrap_or_else(|| {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_default();
        base.join("kagaz").join("share")
    });
    let text = std::fs::read_to_string(keep.join("sessions.jsonl")).unwrap_or_default();
    let mut list: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    list.reverse();
    list.truncate(100);
    list
}

/// Kagaz is closing: sharing stops with it.
pub fn shutdown(app: &AppHandle) {
    app.state::<Share>().stop();
}
