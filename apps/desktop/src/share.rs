//! Sharing this computer (docs/remote-connection.md, step 2): Kagaz's
//! window runs `kagaz-share` while sharing is on, and only then: off, as
//! it is until the person turns it on, nothing of it runs or listens
//! (one use of Kagaz must not cloud another). The program says what
//! happens in `@`-lines of JSON; this side keeps the state, tells the
//! pages, and opens the question and the sign as their own small
//! windows, on top of whatever is open.
//!
//! While sharing is on, Kagaz waits in the tray when its window is closed
//! (Open Kagaz, Stop sharing, Quit), and can start with Windows straight
//! into the tray (off unless chosen). With Kagaz's helper service
//! installed (chosen under "Sign-in screen and administrator prompts",
//! with an administrator's password; win_helper.rs in kagaz-share), the
//! service runs the share instead, also before anyone signs in, and this
//! window steers it over a connection on this computer.

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
    /// Start Kagaz, in the tray, when the person signs in to Windows.
    pub startup: bool,
    /// The sign-in screen and administrator prompts: "person" (someone at
    /// the computer answers them), "rdp" (Windows' own Remote Desktop), or
    /// "service" (Kagaz's helper service).
    pub prompts: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            on: false,
            port: 3390,
            listen: "all".into(),
            ask: true,
            lock: "unused".into(),
            record: true,
            startup: false,
            prompts: "person".into(),
        }
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
    /// Windows (the tray, starting with Windows, the helper are offered).
    pub windows: bool,
    /// Kagaz's helper service is installed, and this window steers it.
    pub helper: bool,
    pub steering: bool,
    /// Windows' own Remote Desktop is turned on here (None: not known).
    pub rdp: Option<bool>,
}

/// How Kagaz reaches the share: the program it started, or the helper's.
enum Link {
    Child(Child, ChildStdin),
    Helper(std::net::TcpStream),
}

#[derive(Default)]
pub struct Share {
    child: Mutex<Option<Link>>,
    status: Mutex<Status>,
    /// Looking for the helper again after losing it.
    seeking: std::sync::atomic::AtomicBool,
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

/// The helper's folder (`C:\ProgramData\Kagaz`), or the one named by
/// KAGAZ_SHARE_HELPER for testing (with a `kagaz-share --system --home`
/// there standing in for the service).
fn helper_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KAGAZ_SHARE_HELPER") {
        return Some(dir.into());
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        Some(base.join("Kagaz"))
    }
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
fn quiet(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(program);
    c.creation_flags(0x0800_0000);
    c
}

/// Kagaz's helper service is installed here.
fn helper_installed() -> bool {
    if std::env::var_os("KAGAZ_SHARE_HELPER").is_some() {
        return true;
    }
    #[cfg(windows)]
    {
        quiet("sc.exe").args(["query", "KagazHelper"]).output().is_ok_and(|o| o.status.success())
    }
    #[cfg(not(windows))]
    false
}

/// Windows' own Remote Desktop is on (it lets connections in).
fn rdp_on() -> Option<bool> {
    #[cfg(windows)]
    {
        let out = quiet("reg.exe")
            .args(["query", r"HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server", "/v", "fDenyTSConnections"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let v = text.split_whitespace().last()?;
        Some(v == "0x0")
    }
    #[cfg(not(windows))]
    None
}

/// Start with Windows: Kagaz in the tray at sign-in, while sharing is on
/// and the person chose it; otherwise nothing of it is left.
fn startup_apply(settings: &Settings) {
    #[cfg(windows)]
    {
        const RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        if settings.on && settings.startup {
            if let Ok(me) = std::env::current_exe() {
                let line = format!("\"{}\" --tray", me.to_string_lossy());
                let _ = quiet("reg.exe").args(["add", RUN, "/v", "Kagaz", "/t", "REG_SZ", "/d", &line, "/f"]).output();
            }
        } else {
            let _ = quiet("reg.exe").args(["delete", RUN, "/v", "Kagaz", "/f"]).output();
        }
    }
    #[cfg(not(windows))]
    let _ = settings;
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
        s.windows = cfg!(windows) || std::env::var_os("KAGAZ_SHARE_PATTERN").is_some();
        s.helper = helper_installed();
        s.steering = matches!(*self.child.lock().unwrap(), Some(Link::Helper(_)));
        s.rdp = rdp_on();
        s
    }

    fn tell(&self, app: &AppHandle) {
        let _ = app.emit("share", self.status());
    }

    /// Steers the helper's share: its token and port are in its folder,
    /// readable by the person who installed it. The settings go first.
    fn steer(&self, app: &AppHandle) -> Result<(), String> {
        let mut link = self.child.lock().unwrap();
        if link.is_some() {
            return Ok(());
        }
        let home = helper_home().ok_or("no helper here")?;
        let told: Value = std::fs::read_to_string(home.join("control").join("control.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .ok_or("Kagaz's helper is not running")?;
        let port = told["port"].as_u64().ok_or("Kagaz's helper is not running")? as u16;
        let token = told["token"].as_str().unwrap_or("");
        let mut stream = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), std::time::Duration::from_secs(2))
            .map_err(|_| "Kagaz's helper is not running".to_string())?;
        let settings = serde_json::to_string(&load()).map_err(|e| e.to_string())?;
        writeln!(stream, "{token}").and_then(|_| writeln!(stream, "settings {settings}")).map_err(|e| e.to_string())?;
        let reading = stream.try_clone().map_err(|e| e.to_string())?;
        *link = Some(Link::Helper(stream));
        drop(link);
        *self.status.lock().unwrap() = Status::default();
        log("steering the helper");
        let app = app.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(reading).lines() {
                let Ok(line) = line else { break };
                if let Some(ev) = line.strip_prefix('@').and_then(|j| serde_json::from_str::<Value>(j).ok()) {
                    heard(&app, ev);
                }
            }
            let share = app.state::<Share>();
            let was = matches!(*share.child.lock().unwrap(), Some(Link::Helper(_)));
            if was {
                share.child.lock().unwrap().take();
            }
            *share.status.lock().unwrap() = Status::default();
            close_window(&app, "share-sign");
            close_window(&app, "share-ask");
            log("the helper let go");
            share.tell(&app);
            // It starts again by itself after a change of settings: found
            // again when it does.
            if was {
                share.seek(&app);
            }
        });
        Ok(())
    }

    /// Looks for the helper every two seconds until it answers (or is no
    /// longer installed).
    fn seek(&self, app: &AppHandle) {
        if self.seeking.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        let app = app.clone();
        std::thread::spawn(move || {
            let share = app.state::<Share>();
            for _ in 0..900 {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if !helper_installed() || share.child.lock().unwrap().is_some() {
                    break;
                }
                if share.steer(&app).is_ok() {
                    share.tell(&app);
                    break;
                }
            }
            share.seeking.store(false, std::sync::atomic::Ordering::Release);
        });
    }

    /// Starts the program with the settings, unless it runs; with the
    /// helper installed, steers the helper's instead.
    pub fn start(&self, app: &AppHandle) -> Result<(), String> {
        if helper_installed() {
            return match self.steer(app) {
                Ok(()) => Ok(()),
                Err(e) => {
                    self.seek(app);
                    Err(e)
                }
            };
        }
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
        *child = Some(Link::Child(c, stdin));
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
            let ended = {
                let mut link = share.child.lock().unwrap();
                if matches!(*link, Some(Link::Child(..))) {
                    link.take()
                } else {
                    None
                }
            };
            if let Some(Link::Child(mut c, _)) = ended {
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

    /// Stops the program Kagaz started, or lets go of the helper's (which
    /// goes on as its settings say).
    pub fn stop(&self) {
        let link = self.child.lock().unwrap().take();
        match link {
            Some(Link::Child(mut c, stdin)) => {
                // Its input closing ends it; then make sure.
                drop(stdin);
                std::thread::sleep(std::time::Duration::from_millis(300));
                let _ = c.kill();
                let _ = c.wait();
            }
            Some(Link::Helper(s)) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            None => {}
        }
    }

    pub fn send(&self, line: &str) -> Result<(), String> {
        let mut child = self.child.lock().unwrap();
        match child.as_mut().ok_or("sharing is off")? {
            Link::Child(_, stdin) => writeln!(stdin, "{line}").and_then(|_| stdin.flush()).map_err(|e| e.to_string()),
            Link::Helper(s) => writeln!(s, "{line}").and_then(|_| s.flush()).map_err(|e| e.to_string()),
        }
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
                st.running = true;
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
            // The helper's share, turned off: nothing listens.
            "off" => {
                st.running = false;
                st.listening = None;
                st.session = None;
            }
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
        // The helper's share asks this window to lock: Windows locks only
        // from the person's own desktop.
        "lock" => {
            #[cfg(windows)]
            let _ = quiet("rundll32.exe").arg("user32.dll,LockWorkStation").spawn();
        }
        _ => {}
    }
    share.tell(app);
}

// ---- the tray ----

/// Kagaz stays running in the tray (sharing is on here).
pub fn keeps_running() -> bool {
    load().on && supported()
}

const TRAY: &str = "share";

/// The tray icon while sharing is on, and only then.
fn tray(app: &AppHandle) {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let want = keeps_running();
    let have = app.tray_by_id(TRAY).is_some();
    if want == have {
        return;
    }
    if !want {
        let _ = app.remove_tray_by_id(TRAY);
        return;
    }
    let made = (|| -> tauri::Result<()> {
        let open = MenuItem::with_id(app, "share-open", "Open Kagaz", true, None::<&str>)?;
        let stop = MenuItem::with_id(app, "share-stop", "Stop sharing", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "share-quit", "Quit Kagaz", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &stop, &quit])?;
        let mut b = TrayIconBuilder::with_id(TRAY).menu(&menu).tooltip("Kagaz: sharing this computer").show_menu_on_left_click(false);
        if let Some(icon) = app.default_window_icon() {
            b = b.icon(icon.clone());
        }
        b.on_menu_event(|app, ev| match ev.id().as_ref() {
            "share-open" => show_main(app),
            "share-stop" => {
                let mut s = load();
                s.on = false;
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = share_settings(app, s).await;
                });
            }
            "share-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|icon, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(icon.app_handle());
            }
        })
        .build(app)?;
        Ok(())
    })();
    if let Err(e) = made {
        log(&format!("no tray icon: {e}"));
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// At Kagaz's start: sharing goes on again only if the person left it on.
/// Started with Windows (`--tray`), Kagaz waits in the tray.
pub fn resume(app: &AppHandle) {
    let settings = load();
    if settings.on && supported() {
        startup_apply(&settings);
        if let Err(e) = app.state::<Share>().start(app) {
            log(&e);
        }
        tray(app);
        if std::env::args().any(|a| a == "--tray") {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.hide();
            }
        }
    }
}

#[tauri::command]
pub fn share_status(app: AppHandle) -> Status {
    app.state::<Share>().status()
}

/// Installs or removes Kagaz's helper service (Windows' administrator
/// question first); Kagaz's own share is stopped while it does.
async fn helper_change(app: &AppHandle, install: bool) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("Kagaz's helper service is for Windows.".into());
    }
    let program = program().ok_or("kagaz-share is not next to Kagaz")?;
    let keep = share_folder();
    let settings = settings_path();
    let share = app.state::<Share>();
    share.stop();
    let ended = tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = Command::new(program);
        cmd.arg("helper");
        if install {
            cmd.arg("install").arg("--from").arg(&keep).arg("--settings").arg(&settings);
        } else {
            cmd.arg("remove").arg("--to").arg(&keep);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.status().map(|s| s.code())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("kagaz-share did not start: {e}"))?;
    match ended {
        Some(0) => Ok(()),
        Some(1223) => Err("Windows' administrator question was not answered yes: nothing changed.".into()),
        _ => Err(if install {
            "Kagaz's helper could not be installed.".into()
        } else {
            "Kagaz's helper could not be removed completely.".into()
        }),
    }
}

#[tauri::command]
pub async fn share_settings(app: AppHandle, settings: Settings) -> Result<Status, String> {
    let before = load();
    // The helper, installed or removed as the choice for the sign-in
    // screen asks; on a no, the choice stays as it was.
    let mut settings = settings;
    let installed = helper_installed() && std::env::var_os("KAGAZ_SHARE_HELPER").is_none();
    if settings.prompts == "service" && !installed && cfg!(windows) {
        if let Err(e) = helper_change(&app, true).await {
            settings.prompts = before.prompts.clone();
            save(&settings)?;
            let share = app.state::<Share>();
            if settings.on {
                let _ = share.start(&app);
            }
            share.tell(&app);
            return Err(e);
        }
    } else if settings.prompts != "service" && installed {
        if let Err(e) = helper_change(&app, false).await {
            settings.prompts = "service".into();
            save(&settings)?;
            let share = app.state::<Share>();
            let _ = share.start(&app);
            share.tell(&app);
            return Err(e);
        }
    }
    apply(&app, before, settings)
}

fn save(settings: &Settings) -> Result<(), String> {
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

fn apply(app: &AppHandle, before: Settings, settings: Settings) -> Result<Status, String> {
    let app = app.clone();
    save(&settings)?;
    startup_apply(&settings);
    let share = app.state::<Share>();
    if helper_installed() {
        // The helper's share takes the settings and starts again with them
        // where needed; it goes on while Kagaz is closed.
        if share.child.lock().unwrap().is_none() {
            let _ = share.start(&app);
        } else {
            let line = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
            share.send(&format!("settings {line}"))?;
        }
        share.status.lock().unwrap().problem = None;
    } else if !settings.on {
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
    tray(&app);
    share.tell(&app);
    Ok(share.status())
}

/// A line to the program, from the pages: only the commands it has.
#[tauri::command]
pub fn share_send(app: AppHandle, line: String) -> Result<(), String> {
    let first = line.split_whitespace().next().unwrap_or("");
    // Windows' own page for Remote Desktop, from the choice for the
    // sign-in screen.
    if line == "open-rdp-settings" {
        #[cfg(windows)]
        {
            let _ = quiet("explorer.exe").arg("ms-settings:remotedesktop").spawn();
            return Ok(());
        }
        #[cfg(not(windows))]
        return Err("Windows' own Remote Desktop is on Windows".into());
    }
    const ALLOWED: [&str; 12] = ["pair", "stop", "confirm", "cancel", "pin", "allow", "forget", "answer", "watch", "control", "end", "devices"];
    if !ALLOWED.contains(&first) || line.contains('\n') {
        return Err("not a sharing command".into());
    }
    app.state::<Share>().send(&line)
}

/// kagaz-share's folder for what sharing keeps (the same as it finds).
fn share_folder() -> PathBuf {
    std::env::var_os("KAGAZ_SHARE_KEEP").map(PathBuf::from).unwrap_or_else(|| {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_default();
        base.join("kagaz").join("share")
    })
}

/// The record of sessions, newest first (the helper's, where it runs).
#[tauri::command]
pub fn share_sessions() -> Vec<Value> {
    let at = match helper_home().filter(|_| helper_installed()) {
        Some(home) => home.join("control").join("sessions.jsonl"),
        None => share_folder().join("sessions.jsonl"),
    };
    let text = std::fs::read_to_string(at).unwrap_or_default();
    let mut list: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    list.reverse();
    list.truncate(100);
    list
}

/// Kagaz is closing: sharing stops with it (the helper's goes on).
pub fn shutdown(app: &AppHandle) {
    app.state::<Share>().stop();
}
