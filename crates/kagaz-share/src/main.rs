//! Sharing this computer: a Remote Desktop server that shows the screen
//! of the person signed in, who goes on using it, to a Kagaz elsewhere
//! (docs/remote-connection.md, step 2). It runs only while sharing is
//! turned on; until Kagaz starts it, nothing of it runs or listens.
//!
//!   kagaz-share [--port 3390] [--local | --bind IP] [--name NAME] [--pin 123456]
//!               [--driven] [--lock never|always|unused] [--no-record]
//!               [--watch] [--rate 30] [--capture dxgi|gdi] [--ask-on-input]
//!               [--no-sound] [--no-clipboard] [--no-files]
//!               [--pattern [--size 1280x720] [--card-rate 5] [--tone] [--answer control|watch|deny]]
//!               [--keep DIR]
//!
//! `--port` is the port it listens on (3390 unless said: Windows' own
//! Remote Desktop keeps 3389). `--local` listens on this computer only,
//! `--bind` on one address of it (its Tailscale address, say).
//! `--driven`: Kagaz's window runs it, and reads `@`-lines of JSON from
//! its output (who connected, a question, a pairing's code and words).
//! `--lock`: lock Windows when a session ends (never unless said), or
//! only if nobody here used it meanwhile. Each session is written to
//! `sessions.jsonl` in its folder unless `--no-record`.
//! `--name` is what paired devices call it (the computer's name unless
//! said). `--pin` sets this computer's PIN. `--watch` lets every device
//! only watch.
//!
//! Only paired devices connect (paired.rs): with the right PIN a device
//! allowed to connect without asking comes straight in, otherwise the
//! person here is asked: in Windows' own small question when this runs on
//! its own, on the output (`ask <id> "<name>"`) with `--ask-on-input`, as
//! Kagaz's window runs it. Pairing (pairing.rs, remote/link.rs) goes over
//! the same port.
//!
//! On its input, a line at a time (from Kagaz's window, or typed):
//! `pair` shows a code to pair a device with; `confirm [what it may do]`
//! or `cancel` once the words show; `pin <digits>` sets the PIN; `list`;
//! `allow <id> <what it may do>`; `forget <id>`; `answer <id>
//! control|watch|deny`; during a session `watch`, `control`, `end`;
//! `devices`; `lock never|always|unused`. What a device may do is any of `control
//! clipboard files sound unattended`.
//!
//! `--pattern` shows the test card instead of the screen, on any system,
//! and says each key and mouse event as `heard …`: for the tests. With it
//! `--tone` sends a quiet made tone as the sound, `--answer` answers every
//! question the same (control unless said), and what the viewer copies or
//! sends is said by its length or size, never its content.
//!
//! This computer's sound goes to the viewer (`--no-sound` keeps it
//! here), the clipboard goes both ways (`--no-clipboard`), and files the
//! viewer sends land in a folder of their own under the system's
//! temporary folder and on the clipboard, to paste anywhere
//! (`--no-files` refuses them), as far as the device may. Watching only
//! takes nothing from the viewer.
//!
//! Its certificate and the paired devices are kept in `--keep` (Kagaz's
//! folder for them unless said).
//!
//! It says what it does on its output, one line each: `listening
//! <address>`, `fingerprint <sha-256 of its certificate>`, `pictures from
//! …`, who connected and left, and every five seconds the pictures'
//! rate, size and time to encode. Never a picture.

// The same card as the test servers and the viewer's tests draw.
#[path = "../../kagaz-core/src/remote/pattern.rs"]
#[allow(dead_code)]
mod pattern;
// Pairing and signing in, as the viewer builds them too.
#[path = "../../kagaz-core/src/remote/link.rs"]
#[allow(dead_code)]
mod link;
mod clip;
mod paired;
mod pairing;
mod picture;
mod sound;
#[cfg(windows)]
mod win;
#[cfg(windows)]
mod win_ask;
#[cfg(windows)]
mod win_clip;
#[cfg(windows)]
mod win_sound;

use ironrdp_server::tokio_rustls::{rustls, TlsAcceptor};
use ironrdp_server::{
    DesktopSize, DisplayUpdate, GfxServerFactory, KeyboardEvent, MouseEvent, PixelFormat,
    RdpServer, RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler,
};
use picture::{Latest, Source};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::num::{NonZeroU16, NonZeroUsize};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Lines for the program that runs this one (Kagaz's window, with
/// `--driven`): `@` and JSON, one event each.
static DRIVEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn event(value: serde_json::Value) {
    if DRIVEN.load(Ordering::Relaxed) {
        say(&format!("@{value}"));
    }
}

/// How often the person here has used the mouse and keys (not counting
/// what came from a viewer).
pub fn local_input() -> u64 {
    #[cfg(windows)]
    {
        win::local_input()
    }
    #[cfg(not(windows))]
    {
        0
    }
}

pub fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Where the mouse and keys go: this computer, or the output (the test
/// card); only while the device connected may use them.
#[derive(Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
enum To {
    Here(Latest),
    Said,
}

#[derive(Clone)]
struct Input {
    to: To,
    allowed: paired::Allowed,
}

impl Input {
    fn may(&self) -> bool {
        self.allowed.control.load(Ordering::Acquire)
    }
}

impl RdpServerInputHandler for Input {
    fn keyboard(&mut self, event: KeyboardEvent) {
        if !self.may() {
            return;
        }
        match &self.to {
            #[cfg(windows)]
            To::Here(_) => win::key(event),
            #[cfg(not(windows))]
            To::Here(_) => {}
            To::Said => say(&format!("heard {event:?}")),
        }
    }
    fn mouse(&mut self, event: MouseEvent) {
        if !self.may() {
            return;
        }
        match &self.to {
            #[cfg(windows)]
            To::Here(latest) => {
                let size = *latest.size.lock().unwrap();
                win::mouse(event, size)
            }
            #[cfg(not(windows))]
            To::Here(_) => {}
            To::Said => say(&format!("heard {event:?}")),
        }
    }
}

/// The screen as the session sees it: its size, and plain pictures for a
/// viewer that has not opened the graphics pipeline.
#[derive(Clone)]
struct Display {
    latest: Latest,
}

/// One viewer's stream of updates. The library starts a new one each
/// time the session is set up again (after a change of size too), so it
/// starts from the size the viewer was told, and says only real changes.
struct Updates {
    latest: Latest,
    size: (u16, u16),
    sent: u64,
    began: Instant,
    pointer_told: bool,
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for Updates {
    async fn next_update(&mut self) -> anyhow::Result<Option<DisplayUpdate>> {
        loop {
            if !self.pointer_told {
                // The viewer's own pointer is a plain arrow; this
                // computer's pointer is drawn into the pictures, so that
                // the person at it is seen pointing too.
                self.pointer_told = true;
                return Ok(Some(DisplayUpdate::DefaultPointer));
            }
            let size = *self.latest.size.lock().unwrap();
            if size != self.size && size.0 > 0 {
                self.size = size;
                self.sent = 0;
                return Ok(Some(DisplayUpdate::Resize(DesktopSize { width: size.0, height: size.1 })));
            }
            // Plain pictures only when the viewer has not taken the
            // graphics pipeline after a moment.
            let plain = !self.latest.pipeline.load(Ordering::Acquire) && self.began.elapsed() > Duration::from_secs(3);
            if plain {
                if let Some((number, frame)) = self.latest.get() {
                    if number != self.sent && (frame.width, frame.height) == self.size {
                        self.sent = number;
                        return Ok(Some(DisplayUpdate::Bitmap(ironrdp_server::BitmapUpdate {
                            x: 0,
                            y: 0,
                            width: NonZeroU16::new(frame.width).expect("a size is not zero"),
                            height: NonZeroU16::new(frame.height).expect("a size is not zero"),
                            format: PixelFormat::BgrX32,
                            data: frame.bgra.clone().into(),
                            stride: NonZeroUsize::new(frame.width as usize * 4).expect("a size is not zero"),
                        })));
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for Display {
    async fn size(&mut self) -> DesktopSize {
        // The first picture says the size; wait a little for it.
        for _ in 0..50 {
            let (width, height) = *self.latest.size.lock().unwrap();
            if width > 0 {
                return DesktopSize { width, height };
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        DesktopSize { width: 1280, height: 720 }
    }

    async fn request_initial_size(&mut self, _viewer: DesktopSize) -> DesktopSize {
        // This computer's screen keeps its size: the person at it is
        // using it. The viewer fits it to its window.
        self.size().await
    }

    fn request_layout(&mut self, _layout: ironrdp_displaycontrol::pdu::DisplayControlMonitorLayout) {}

    async fn updates(&mut self) -> anyhow::Result<Box<dyn RdpServerDisplayUpdates>> {
        let told = self.size().await;
        Ok(Box::new(Updates {
            latest: self.latest.clone(),
            size: (told.width, told.height),
            sent: 0,
            began: Instant::now(),
            pointer_told: false,
        }))
    }
}

/// Files received more than a day ago are let go: pasting copied them
/// where they were wanted.
fn tidy(received: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(received) else { return };
    let day = Duration::from_secs(24 * 60 * 60);
    for e in entries.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Kagaz's folder for what sharing keeps.
fn keep_dir(args: &[String]) -> PathBuf {
    if let Some(dir) = value(args, "--keep") {
        return dir.into();
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("kagaz").join("share")
}

/// The certificate and its key, made the first time and kept after.
fn certificate(dir: &std::path::Path) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let (cert_at, key_at) = (dir.join("certificate.der"), dir.join("key.der"));
    if let (Ok(cert), Ok(key)) = (std::fs::read(&cert_at), std::fs::read(&key_at)) {
        return Ok((cert, key));
    }
    std::fs::create_dir_all(dir)?;
    let made = rcgen::generate_simple_self_signed(vec!["kagaz-share".to_string()])?;
    let (cert, key) = (made.cert.der().to_vec(), made.signing_key.serialize_der());
    std::fs::write(&cert_at, &cert)?;
    std::fs::write(&key_at, &key)?;
    Ok((cert, key))
}

/// This computer's name, as paired devices see it.
fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "This computer".into())
}

/// What is typed on the input, a line at a time.
/// What the session going on now needs from the commands.
struct Now {
    allowed: paired::Allowed,
    current: Arc<std::sync::Mutex<Option<paired::Current>>>,
    quit: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<ironrdp_server::ServerEvent>>>,
    /// After a session: lock Windows "never", "always", or "unused" (if
    /// nobody here used it meanwhile).
    lock: std::sync::Mutex<String>,
}

fn devices_event(paired: &paired::Paired) {
    let store = paired.store.lock().unwrap();
    let list: Vec<serde_json::Value> = store
        .devices
        .iter()
        .map(|d| serde_json::json!({ "id": d.id, "name": d.name, "may": d.may, "since": d.paired, "seen": d.seen }))
        .collect();
    event(serde_json::json!({ "kind": "devices", "devices": list, "pin": store.pin.is_some() }));
}

fn commands(pairing: Arc<pairing::Pairing>, lines: Arc<paired::Lines>, now: Arc<Now>) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let words: Vec<&str> = line.split_whitespace().collect();
            let paired = &pairing.paired;
            match words.as_slice() {
                ["pair"] => {
                    pairing.start();
                }
                ["stop"] => pairing.stop(),
                ["confirm", may @ ..] => pairing.decide(Some(paired::May::from_words(may))),
                ["cancel"] => pairing.decide(None),
                ["pin", pin] => {
                    say(if paired.set_pin(pin) { "the PIN is set" } else { "a PIN is at least 4 digits" });
                    devices_event(paired);
                }
                ["devices"] => devices_event(paired),
                // The person here, during a session: let it only watch, give
                // it control back (as far as it may), or end it.
                ["watch"] => {
                    now.allowed.control.store(false, Ordering::Release);
                    now.allowed.files.store(false, Ordering::Release);
                    event(serde_json::json!({ "kind": "mode", "control": false }));
                    say("watching only now");
                }
                ["control"] => {
                    let may = now.current.lock().unwrap().as_ref().map(|c| c.may);
                    if let Some(may) = may.filter(|m| m.control) {
                        now.allowed.control.store(true, Ordering::Release);
                        now.allowed.files.store(may.files, Ordering::Release);
                        event(serde_json::json!({ "kind": "mode", "control": true }));
                        say("it may use the mouse and keys again");
                    }
                }
                ["end"] => {
                    if let Some(quit) = now.quit.lock().unwrap().as_ref() {
                        let _ = quit.send(ironrdp_server::ServerEvent::Quit("ended here".into()));
                    }
                }
                ["lock", how @ ("never" | "always" | "unused")] => *now.lock.lock().unwrap() = how.to_string(),
                ["list"] => {
                    let store = paired.store.lock().unwrap();
                    if store.devices.is_empty() {
                        say("no devices paired");
                    }
                    for d in &store.devices {
                        say(&format!("device {} {:?}: {}{}", d.id, d.name, d.may.words(), d.seen.map_or(String::new(), |s| format!(", last seen {s}"))));
                    }
                    say(if store.pin.is_some() { "a PIN is set" } else { "no PIN set yet: pin <digits>" });
                }
                ["allow", id, may @ ..] => {
                    say(if paired.change(id, paired::May::from_words(may)) { "changed" } else { "no such device" });
                    devices_event(paired);
                }
                ["forget", id] => {
                    say(if paired.forget(id) { "forgotten" } else { "no such device" });
                    devices_event(paired);
                }
                ["answer", id, how] => lines.answer(
                    id,
                    match *how {
                        "control" => paired::Answer::Control,
                        "watch" => paired::Answer::Watch,
                        _ => paired::Answer::Deny,
                    },
                ),
                [] => {}
                _ => say("pair, confirm, cancel, pin <digits>, list, allow <id> …, forget <id>, answer <id> control|watch|deny"),
            }
        }
        // Kagaz's window that ran it is gone: sharing stops with it, never
        // left running unseen.
        if DRIVEN.load(Ordering::Relaxed) {
            say("Kagaz closed: sharing stops");
            std::process::exit(0);
        }
    });
}

/// A new connection: a pairing (it starts with Kagaz's mark) or a Remote
/// Desktop session (which never does).
async fn route(stream: tokio::net::TcpStream, sessions: tokio::sync::mpsc::Sender<tokio::net::TcpStream>, pairing: Arc<pairing::Pairing>) {
    let mut first = [0u8; 4];
    let began = Instant::now();
    loop {
        match stream.peek(&mut first).await {
            Ok(n) if n >= 4 => break,
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if began.elapsed() > Duration::from_secs(5) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if &first == link::MAGIC {
        let Ok(mut s) = stream.into_std() else { return };
        let _ = s.set_nonblocking(false);
        std::thread::spawn(move || {
            use std::io::Read;
            let mut mark = [0u8; 4];
            if s.read_exact(&mut mark).is_ok() {
                pairing.handle(s);
            }
        });
    } else {
        let _ = sessions.send(stream).await;
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        say("kagaz-share [--port 3390] [--local | --bind IP] [--name NAME] [--driven] [--lock never|always|unused] [--no-record] [--pin 123456] [--watch] [--rate 30] [--capture dxgi|gdi] [--ask-on-input] [--no-sound] [--no-clipboard] [--no-files] [--pattern [--size 1280x720] [--card-rate 5] [--tone] [--answer control|watch|deny]] [--keep DIR]");
        return Ok(());
    }
    #[cfg(windows)]
    win::dpi_aware();
    let port: u16 = value(&args, "--port").map(|p| p.parse()).transpose()?.unwrap_or(3390);
    let local = args.iter().any(|a| a == "--local");
    let bind: Option<std::net::IpAddr> = value(&args, "--bind").map(|b| b.parse()).transpose()?;
    if args.iter().any(|a| a == "--driven") {
        DRIVEN.store(true, Ordering::Relaxed);
    }
    let rate: u32 = value(&args, "--rate").map(|p| p.parse()).transpose()?.unwrap_or(30).clamp(1, 60);
    let pattern = args.iter().any(|a| a == "--pattern");
    let watch = args.iter().any(|a| a == "--watch");
    let name = value(&args, "--name").unwrap_or_else(computer_name);
    let keep = keep_dir(&args);
    let (der, key) = certificate(&keep)?;
    let paired = Arc::new(paired::Paired::load(keep.join("paired.json")));
    if let Some(pin) = value(&args, "--pin") {
        anyhow::ensure!(paired.set_pin(&pin), "a PIN is at least 4 digits");
    }
    let allowed = paired::Allowed::default();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(der.clone())],
            rustls::pki_types::PrivateKeyDer::Pkcs8(key.into()),
        )?;
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let host: std::net::IpAddr = match bind {
        Some(ip) => ip,
        None if local => std::net::Ipv4Addr::LOCALHOST.into(),
        None => std::net::Ipv4Addr::UNSPECIFIED.into(),
    };
    let listener = std::net::TcpListener::bind((host, port))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;

    let latest = Latest::default();
    if pattern {
        let size = value(&args, "--size").unwrap_or_else(|| "1280x720".into());
        let (w, h) = size.split_once('x').ok_or_else(|| anyhow::anyhow!("a size is a width and a height, as 1280x720"))?;
        let (width, height): (u16, u16) = (w.parse()?, h.parse()?);
        anyhow::ensure!(width >= 16 && height >= 16, "a size of at least 16x16");
        let card_rate: u32 = value(&args, "--card-rate").map(|p| p.parse()).transpose()?.unwrap_or(5);
        picture::take(
            move || -> anyhow::Result<Box<dyn Source>> {
                Ok(Box::new(picture::Card { width, height, tick: 0, last: None, began: Instant::now(), rate: card_rate }))
            },
            latest.clone(),
            rate,
        );
    } else {
        #[cfg(windows)]
        {
            let how = value(&args, "--capture");
            picture::take(move || win::open(how.as_deref()), latest.clone(), rate);
        }
        #[cfg(not(windows))]
        anyhow::bail!("sharing this computer's screen comes to this system later; --pattern shows the test card");
    }
    // The sound: this computer's, or the made tone.
    let with_sound = !args.iter().any(|a| a == "--no-sound") && (!pattern || args.iter().any(|a| a == "--tone"));
    let speaker = if !with_sound {
        None
    } else if pattern {
        Some(sound::Speaker::new(|| Ok(Box::new(sound::Tone::new()) as Box<dyn sound::Sound>), 48000, allowed.sound.clone()))
    } else {
        #[cfg(windows)]
        {
            Some(sound::Speaker::new(
                || Ok(Box::new(win_sound::open()?) as Box<dyn sound::Sound>),
                win_sound::native_rate(),
                allowed.sound.clone(),
            ))
        }
        #[cfg(not(windows))]
        None
    };
    // The clipboard and files.
    let received = std::env::temp_dir().join("kagaz").join("received");
    tidy(&received);
    let clipboard = if args.iter().any(|a| a == "--no-clipboard") {
        None
    } else {
        let board: Option<Arc<dyn clip::Board>> = if pattern {
            Some(Arc::new(clip::Said::default()))
        } else {
            #[cfg(windows)]
            {
                match win_clip::WinBoard::start() {
                    Ok(b) => Some(Arc::new(b)),
                    Err(e) => {
                        say(&format!("no clipboard: {e:#}"));
                        None
                    }
                }
            }
            #[cfg(not(windows))]
            None
        };
        board.map(|b| clip::Clip::new(b, received.clone(), allowed.clone(), args.iter().any(|a| a == "--no-files")))
    };
    let pipeline = picture::Pipeline::default();
    picture::pump(latest.clone(), pipeline.clone(), rate);

    let input = Input { to: if pattern { To::Said } else { To::Here(latest.clone()) }, allowed: allowed.clone() };
    let fingerprint: String = link::hex(&Sha256::digest(&der));
    // Who answers when a device asks to connect.
    let lines = Arc::new(paired::Lines::default());
    let asker: Arc<dyn paired::Asker> = if args.iter().any(|a| a == "--ask-on-input" || a == "--driven") {
        lines.clone()
    } else if pattern {
        Arc::new(paired::Always(match value(&args, "--answer").as_deref() {
            Some("watch") => paired::Answer::Watch,
            Some("deny") => paired::Answer::Deny,
            _ => paired::Answer::Control,
        }))
    } else {
        #[cfg(windows)]
        {
            Arc::new(win_ask::Window)
        }
        #[cfg(not(windows))]
        {
            lines.clone()
        }
    };
    let current = Arc::new(std::sync::Mutex::new(None));
    let gate = paired::Gate { paired: paired.clone(), asker, allowed: allowed.clone(), watch_only: watch, current: current.clone() };
    let now = Arc::new(Now {
        allowed: allowed.clone(),
        current: current.clone(),
        quit: std::sync::Mutex::new(None),
        lock: std::sync::Mutex::new(value(&args, "--lock").unwrap_or_else(|| "never".into())),
    });
    let record = !args.iter().any(|a| a == "--no-record");
    let sessions_file = keep.join("sessions.jsonl");
    let pairing = Arc::new(pairing::Pairing::new(paired.clone(), name.clone(), fingerprint.clone(), addr.port()));
    commands(pairing.clone(), lines, now.clone());
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let viewers = latest.viewers.clone();
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let mut server = RdpServer::builder()
            .with_addr(addr)
            // TLS, with each sign-in checked against the paired devices
            // (the library checks only one fixed account under NLA).
            .with_tls(acceptor)
            .with_input_handler(input)
            .with_display_handler(Display { latest: latest.clone() })
            .with_gfx_factory(Some(Box::new(pipeline) as Box<dyn GfxServerFactory>))
            .with_sound_factory(speaker.map(|s| Box::new(s) as Box<dyn ironrdp_server::SoundServerFactory>))
            .with_cliprdr_factory(clipboard.map(|c| Box::new(c) as Box<dyn ironrdp_server::CliprdrServerFactory>))
            .with_honor_client_desktop_size(false)
            .with_credential_validator(Some(Arc::new(gate)))
            .build();
        *now.quit.lock().unwrap() = Some(server.event_sender().clone());
        event(serde_json::json!({ "kind": "listening", "address": addr.to_string(), "port": addr.port(), "name": name, "fingerprint": fingerprint }));
        devices_event(&paired);
        say(&format!("listening {addr} as {name:?}"));
        say(&format!("fingerprint {fingerprint}"));
        {
            let store = paired.store.lock().unwrap();
            say(&format!("{} devices paired; {}", store.devices.len(), if store.pin.is_some() { "a PIN is set" } else { "no PIN set yet (pin <digits>)" }));
        }
        say("type pair to pair a device");
        if watch {
            say("watch only: the mouse and keys stay with this computer");
        }
        // Connections are sorted as they come; sessions run one at a time.
        let (to_sessions, mut sessions) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { continue };
                tokio::spawn(route(stream, to_sessions.clone(), pairing.clone()));
            }
        });
        while let Some(stream) = sessions.recv().await {
            say("a viewer connected");
            allowed.set(paired::May { control: false, clipboard: false, files: false, sound: false, unattended: false });
            // Pictures are taken from now until it leaves, so that the
            // screen's size is known when the session is set up.
            viewers.fetch_add(1, Ordering::AcqRel);
            let began = Instant::now();
            match server.run_connection(stream).await {
                Ok(()) => say(&format!("the viewer left after {} s", began.elapsed().as_secs())),
                Err(e) => say(&format!("the viewer left after {} s: {e:#}", began.elapsed().as_secs())),
            }
            viewers.fetch_sub(1, Ordering::AcqRel);
            allowed.set(paired::May { control: false, clipboard: false, files: false, sound: false, unattended: false });
            // Who it was: the record, the sign, and locking up after.
            if let Some(was) = current.lock().unwrap().take() {
                let secs = was.began.elapsed().as_secs();
                let used_here = local_input() != was.local_input;
                event(serde_json::json!({ "kind": "left", "id": was.id, "name": was.name, "seconds": secs }));
                if record {
                    let line = serde_json::json!({ "at": paired::now(), "id": was.id, "name": was.name, "seconds": secs });
                    use std::io::Write as _;
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&sessions_file) {
                        let _ = writeln!(f, "{line}");
                    }
                }
                let how = now.lock.lock().unwrap().clone();
                if how == "always" || (how == "unused" && !used_here) {
                    say("locking this computer: the session is over");
                    #[cfg(windows)]
                    win::lock();
                }
            }
        }
        Ok(())
    })
}
