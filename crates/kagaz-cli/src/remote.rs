//! `kagaz remote`: the computers and phones you added, every way to reach
//! each and how it does now, a session with one told in numbers, clipboard
//! text sent there, and the system's own ssh and scp over whichever path
//! answers. The same list as the window's Computers & phones.

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use kagaz_core::remote::tailscale::{self, Tailscale};
use kagaz_core::remote::{
    self, Answer, Device, Devices, Event, Input, LocateError, OpenError, PathKind, PathReport, PathTo, Piece, Session,
    SignIn, Speaks, Trouble, Trust, Way,
};
use std::time::{Duration, Instant};

#[derive(Subcommand)]
pub enum RemoteAction {
    /// The computers and phones you added, and whether each is within reach now
    List {
        /// List them without trying to reach them
        #[arg(long)]
        no_check: bool,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Remember a computer or phone by its address or name
    Add {
        /// Its address or name: 192.0.2.20, study-laptop.local, a Tailscale address or name
        host: String,
        /// What to call it; without it, its own name, else the address
        #[arg(long)]
        name: Option<String>,
        /// The account to sign in with
        #[arg(long)]
        user: Option<String>,
        /// It speaks Remote Desktop on this port (without these three, Kagaz finds out)
        #[arg(long, num_args = 0..=1, default_missing_value = "3389")]
        rdp: Option<u16>,
        /// It speaks VNC on this port
        #[arg(long, num_args = 0..=1, default_missing_value = "5900")]
        vnc: Option<u16>,
        /// It speaks SSH on this port
        #[arg(long, num_args = 0..=1, default_missing_value = "22")]
        ssh: Option<u16>,
        /// It is a phone that also shares its hotspot: reach it at the gateway then
        #[arg(long)]
        hotspot: bool,
    },
    /// Computers and phones on this network that show their screen, and your Tailscale devices
    Nearby {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Every way to a device, and how each does now
    Paths {
        /// The device's name, or the start of it
        device: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Add or remove a way to reach a device
    Path {
        device: String,
        #[command(subcommand)]
        change: PathChange,
    },
    /// Give a device another name
    Rename { device: String, name: String },
    /// Stop remembering a device
    Forget { device: String },
    /// Open the Kagaz window on a device's screen
    Open { device: String },
    /// Open a session with a device and tell it in numbers; no picture is shown or kept
    Try {
        device: String,
        /// How long to watch it
        #[arg(long, default_value_t = 5)]
        seconds: u64,
        /// Sharper (0) to smoother (10); without it Kagaz follows the link
        #[arg(long)]
        picture: Option<u8>,
    },
    /// Send text to a device's clipboard (the text, or what is piped in)
    Clip {
        device: String,
        text: Option<String>,
    },
    /// Serve a moving test card over VNC, to try the viewer without a real screen
    Pattern {
        #[arg(long, default_value_t = 5901)]
        port: u16,
        /// Width and height, as 1280x720
        #[arg(long, default_value = "1280x720")]
        size: String,
        /// Ask viewers for this VNC password
        #[arg(long)]
        password: Option<String>,
        /// How many times a second the card moves; 0 keeps it still
        #[arg(long, default_value_t = 30)]
        rate: u32,
        /// Let other devices on the network connect, not only this computer
        #[arg(long)]
        open: bool,
    },
    /// The system's ssh to the device, over whichever path answers
    Ssh {
        device: String,
        /// A command to run there instead of a shell (after `--`)
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Copy a file with the system's scp; a side that starts with a colon is on the device
    Copy {
        device: String,
        /// For example `report.pdf` here, or `:Documents/report.pdf` there
        from: String,
        to: String,
    },
}

#[derive(Subcommand)]
pub enum PathChange {
    /// One more address or name for the device
    Add { host: String },
    /// The device is a phone that shares its hotspot: also try the gateway
    Hotspot,
    /// Take an address or name away (`hotspot` for the gateway)
    Remove { host: String },
}

const QUICK: Duration = Duration::from_millis(2500);

fn list_path() -> std::path::PathBuf {
    Devices::default_path()
}

fn system_word(system: &str) -> &str {
    match system {
        "windows" => "Windows",
        "linux" => "Linux",
        "android" => "Android",
        "macos" => "macOS",
        "ios" => "iPhone",
        other => other,
    }
}

/// "Windows · Remote Desktop, SSH".
fn what(d: &Device) -> String {
    let ways: Vec<String> = d.ways.iter().map(Way::words).collect();
    [system_word(&d.system).to_string(), ways.join(", ")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn trouble_words(t: &Trouble) -> String {
    t.to_string()
}

fn report_json(d: &Device, r: &PathReport) -> serde_json::Value {
    let p = &d.paths[r.path];
    let mut v = serde_json::json!({
        "path": p.kind.label(),
        "host": p.host,
        "speaks": r.way.speaks.label(),
        "port": r.way.port,
        "answers": r.outcome.is_ok(),
    });
    match &r.outcome {
        Ok(a) => {
            v["address"] = a.addr.ip().to_string().into();
            v["millis"] = a.millis.into();
        }
        Err(t) => v["trouble"] = trouble_words(t).into(),
    }
    v
}

pub fn run(action: Option<RemoteAction>) -> Result<()> {
    match action.unwrap_or(RemoteAction::List {
        no_check: false,
        json: false,
    }) {
        RemoteAction::List { no_check, json } => list(no_check, json),
        RemoteAction::Add {
            host,
            name,
            user,
            rdp,
            vnc,
            ssh,
            hotspot,
        } => add(&host, name, user, [(Speaks::Rdp, rdp), (Speaks::Vnc, vnc), (Speaks::Ssh, ssh)], hotspot),
        RemoteAction::Nearby { json } => nearby(json),
        RemoteAction::Paths { device, json } => paths(&device, json),
        RemoteAction::Path { device, change } => {
            let ((), saved) = Devices::update(&list_path(), |d| match &change {
                PathChange::Add { host } => d.add_path(&device, PathTo::new(host)),
                PathChange::Hotspot => d.add_path(&device, PathTo::gateway()),
                PathChange::Remove { host } => d.remove_path(&device, host),
            })?;
            let d = saved.find(&device)?;
            println!("{} is now reached by:", d.name);
            for p in &d.paths {
                println!("  {}", p.words());
            }
            Ok(())
        }
        RemoteAction::Rename { device, name } => {
            let ((), saved) = Devices::update(&list_path(), |d| d.rename(&device, &name))?;
            let _ = saved;
            println!("Renamed to {}.", name.trim());
            Ok(())
        }
        RemoteAction::Forget { device } => {
            let (gone, _) = Devices::update(&list_path(), |d| d.forget(&device))?;
            println!("Forgot {}.", gone.name);
            Ok(())
        }
        RemoteAction::Open { device } => open_window(&device),
        RemoteAction::Try { device, seconds, picture } => try_session(&device, seconds, picture),
        RemoteAction::Clip { device, text } => clip(&device, text),
        RemoteAction::Pattern {
            port,
            size,
            password,
            rate,
            open,
        } => pattern(port, &size, password, rate, open),
        RemoteAction::Ssh { device, command } => ssh(&device, &command),
        RemoteAction::Copy { device, from, to } => copy(&device, &from, &to),
    }
}

fn list(no_check: bool, json: bool) -> Result<()> {
    let devices = Devices::load(&list_path())?;
    // Every device's paths at once: a device that is off costs one timeout
    // for the whole list, not one each.
    let checks: Vec<Option<Vec<PathReport>>> = if no_check {
        devices.devices.iter().map(|_| None).collect()
    } else {
        std::thread::scope(|s| {
            let jobs: Vec<_> = devices
                .devices
                .iter()
                .map(|d| s.spawn(move || remote::check(d, QUICK)))
                .collect();
            jobs.into_iter().map(|j| j.join().ok()).collect()
        })
    };
    let tailscale = if no_check { Tailscale::NotInstalled } else { tailscale::status() };
    // What was learnt goes back in the list, in one short step.
    if !no_check {
        let at = remote::store::now();
        Devices::update(&list_path(), |all| {
            for (d, reports) in devices.devices.iter().zip(&checks) {
                if let Some(a) = reports.as_deref().and_then(|r| remote::best(d, r)) {
                    all.note_reached(&d.id, a.path, at);
                    all.note_identity(&d.id, &a.seen.identity);
                }
            }
            Ok(())
        })?;
    }
    if json {
        let out: Vec<serde_json::Value> = devices
            .devices
            .iter()
            .zip(&checks)
            .map(|(d, reports)| {
                let mut v = serde_json::json!({
                    "id": d.id, "name": d.name, "system": d.system, "shape": d.shape,
                    "ways": d.ways.iter().map(Way::words).collect::<Vec<_>>(),
                });
                if let Some(reports) = reports {
                    let answer = remote::best(d, reports);
                    v["within_reach"] = answer.is_some().into();
                    v["paths"] = reports.iter().map(|r| report_json(d, r)).collect::<Vec<_>>().into();
                    if answer.is_none() {
                        v["why"] = remote::why_not(d, reports, &tailscale).into();
                    }
                }
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if devices.devices.is_empty() {
        println!("No computers or phones added yet.");
        println!("Add one with `kagaz remote add <address or name>`; `kagaz remote nearby` shows what is around.");
        return Ok(());
    }
    for (d, reports) in devices.devices.iter().zip(&checks) {
        println!("{}  ({})", d.name, what(d));
        match reports {
            None => {}
            Some(reports) => match remote::best(d, reports) {
                Some(a) => println!("  within reach: {}, {} ms", d.paths[a.path].words(), a.millis),
                None => println!("  out of reach: {}", remote::why_not(d, reports, &tailscale)),
            },
        }
    }
    Ok(())
}

fn add(host: &str, name: Option<String>, user: Option<String>, asked: [(Speaks, Option<u16>); 3], hotspot: bool) -> Result<()> {
    let host = host.trim();
    if host.is_empty() {
        bail!("an address or a name is needed");
    }
    let asked: Vec<Way> = asked
        .iter()
        .filter_map(|(speaks, port)| port.map(|port| Way { speaks: *speaks, port }))
        .collect();
    if asked.is_empty() {
        println!("Asking {host} what it speaks…");
    }
    let found = remote::describe(host, &asked, &tailscale::status(), QUICK);
    if found.ways.is_empty() {
        bail!(
            "nothing answered at {} for Remote Desktop, VNC or SSH. Check that it is on and that remote access is switched on there; \
             to add it anyway, say what it speaks: --rdp, --vnc or --ssh",
            found.host
        );
    }
    if asked.is_empty() {
        for (way, banner) in found.ways.iter().zip(&found.banners) {
            println!("  {}{}", way.words(), if banner.is_empty() { String::new() } else { format!(" ({banner})") });
        }
    }
    let mut paths = vec![PathTo::new(&found.host)];
    if hotspot {
        paths.push(PathTo::gateway());
    }
    let device = Device {
        name: name.unwrap_or(found.name),
        system: found.system,
        ways: found.ways,
        paths,
        identity: found.identity,
        user,
        ..Default::default()
    };
    let (id, saved) = Devices::update(&list_path(), |d| d.add(device))?;
    let d = saved.find(&id)?;
    println!("Added {} ({}).", d.name, what(d));
    Ok(())
}

fn nearby(json: bool) -> Result<()> {
    let saved = Devices::load(&list_path())?;
    let known = |host: &str| saved.devices.iter().find(|d| d.paths.iter().any(|p| p.host.eq_ignore_ascii_case(host)));
    if !json {
        println!("Looking over this network and in Tailscale…");
    }
    let (found, tailscale) = std::thread::scope(|s| {
        let search = s.spawn(|| remote::search(Duration::from_millis(400)));
        let tailscale = tailscale::status();
        (search.join().unwrap_or_default(), tailscale)
    });
    if json {
        let out = serde_json::json!({
            "network": found.iter().map(|f| serde_json::json!({
                "address": f.ip.to_string(), "speaks": f.way.speaks.label(), "port": f.way.port,
                "name": f.seen.identity.name, "saved_as": known(&f.ip.to_string()).map(|d| d.name.clone()),
            })).collect::<Vec<_>>(),
            "tailscale": match &tailscale {
                Tailscale::NotInstalled => serde_json::json!("not installed"),
                Tailscale::Off => serde_json::json!("off"),
                Tailscale::On { peers } => peers.iter().map(|p| serde_json::json!({
                    "name": p.name, "address": p.address, "os": p.os, "online": p.online,
                    "saved_as": known(&p.address).map(|d| d.name.clone()),
                })).collect::<Vec<_>>().into(),
            },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("On this network:");
    if found.is_empty() {
        println!("  nothing that shows its screen (Remote Desktop or VNC).");
    }
    for f in &found {
        let name = f.seen.identity.name.clone().unwrap_or_default();
        let mark = known(&f.ip.to_string()).map(|d| format!("  [added as {}]", d.name)).unwrap_or_default();
        println!("  {:<15} {:<16} {}{}", f.ip, f.way.words(), name, mark);
    }
    match &tailscale {
        Tailscale::NotInstalled => println!("Tailscale is not on this computer."),
        Tailscale::Off => println!("Tailscale is turned off here."),
        Tailscale::On { peers } => {
            println!("In your Tailscale:");
            if peers.is_empty() {
                println!("  no other devices.");
            }
            for p in peers {
                let mark = known(&p.address).map(|d| format!("  [added as {}]", d.name)).unwrap_or_default();
                println!("  {:<15} {:<22} {:<8} {}{}", p.address, p.name, system_word(&p.os.to_ascii_lowercase()), if p.online { "online" } else { "offline" }, mark);
            }
        }
    }
    println!("Add one with `kagaz remote add <address>`.");
    Ok(())
}

fn paths(which: &str, json: bool) -> Result<()> {
    let devices = Devices::load(&list_path())?;
    let d = devices.find(which)?;
    let reports = remote::check(d, QUICK);
    if json {
        let out: Vec<serde_json::Value> = reports.iter().map(|r| report_json(d, r)).collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("{}  ({})", d.name, what(d));
    for r in &reports {
        let p = &d.paths[r.path];
        let how = match &r.outcome {
            Ok(a) => {
                let at = if p.kind == PathKind::Gateway || a.addr.ip().to_string() != p.host {
                    format!(" at {}", a.addr.ip())
                } else {
                    String::new()
                };
                format!("answers{at}, {} ms", a.millis)
            }
            Err(t) => trouble_words(t),
        };
        println!("  {:<34} {:<16} {}", p.words(), r.way.words(), how);
    }
    if reports.iter().all(|r| r.outcome.is_err()) {
        println!("Out of reach: {}.", remote::why_not(d, &reports, &tailscale::status()));
    }
    Ok(())
}

/// Reach the device for SSH. A device added without SSH is asked on the
/// usual port once, and SSH is remembered when it answers.
fn locate_ssh(which: &str) -> Result<(Device, Answer)> {
    let list = list_path();
    let device = Devices::load(&list)?.find(which)?.clone();
    if device.way(Speaks::Ssh).is_none() {
        let mut trial = device.clone();
        trial.ways = vec![Way::new(Speaks::Ssh)];
        if remote::reach(&trial, Speaks::Ssh, QUICK).is_err() {
            bail!(
                "{} does not answer for SSH on port 22 over any of its paths. An SSH server has to be running there \
                 (Windows: the optional feature \"OpenSSH Server\"; Linux: the openssh-server package)",
                device.name
            );
        }
        Devices::update(&list, |d| {
            let id = d.find(&device.id)?.id.clone();
            if let Some(dev) = d.devices.iter_mut().find(|x| x.id == id) {
                dev.ways.push(Way::new(Speaks::Ssh));
            }
            Ok(())
        })?;
    }
    match remote::locate(&list, &device.id, Speaks::Ssh, QUICK) {
        Ok(found) => Ok(found),
        Err(LocateError::NotReached(n)) => Err(anyhow!("{} is out of reach: {}", device.name, n.why)),
        Err(e) => Err(e.into()),
    }
}

/// Reach a device the way that shows its screen, and open a session
/// there; a password is asked for when the device wants one.
fn open_session(which: &str, picture: Option<u8>) -> Result<(Device, Answer, Session)> {
    let list = list_path();
    let device = Devices::load(&list)?.find(which)?.clone();
    let Some(way) = remote::session::way_for(&device) else {
        bail!("{} has no way in that shows its screen (Remote Desktop or VNC)", device.name);
    };
    let (device, answer) = match remote::locate(&list, &device.id, way.speaks, QUICK) {
        Ok(found) => found,
        Err(LocateError::NotReached(n)) => bail!("{} is out of reach: {}", device.name, n.why),
        Err(e) => return Err(e.into()),
    };
    let mut settings = device.settings.clone();
    if picture.is_some() {
        settings.picture = picture.map(|p| p.min(10));
    }
    let mut sign_in = SignIn {
        user: device.user.clone(),
        password: None,
    };
    let mut trust = Trust {
        known: device.identity.certificate.clone(),
        accept_new: false,
    };
    loop {
        match remote::session::open(answer.addr, answer.way, &sign_in, &settings, &trust, None, Duration::from_secs(10)) {
            Ok(session) => {
                // Who signed in and the certificate it showed are the
                // device's from now on.
                let (user, seen, accepted) = (sign_in.user.clone(), session.info.identity.clone(), trust.accept_new);
                let _ = Devices::update(&list, |d| {
                    if let Some(seen) = &seen {
                        if accepted {
                            d.trust_identity(&device.id, seen);
                        } else {
                            d.note_identity(&device.id, seen);
                        }
                    }
                    if let Some(dev) = d.devices.iter_mut().find(|x| x.id == device.id) {
                        if user.is_some() {
                            dev.user = user.clone();
                        }
                    }
                    Ok(())
                });
                return Ok((device, answer, session));
            }
            Err(OpenError::NeedsUser) => {
                sign_in.user = Some(crate::prompt(&format!("User name on {}: ", device.name))?);
                sign_in.password = Some(crate::prompt_secret("Password (not shown): ")?);
            }
            Err(OpenError::CertificateChanged) => {
                println!(
                    "{} shows another certificate than the one remembered. Windows renews its certificate about twice \
                     a year, and a reinstalled computer has a new one; it also happens when another device answers in \
                     its place.",
                    device.name
                );
                if !crate::prompt("Connect and remember the new one? [y/N] ")?.eq_ignore_ascii_case("y") {
                    bail!("not connected");
                }
                trust.accept_new = true;
            }
            Err(OpenError::NeedsPassword) => {
                sign_in.password = Some(crate::prompt_secret(&format!("Password for {} (not shown): ", device.name))?);
            }
            Err(OpenError::WrongPassword) => {
                sign_in.password = Some(crate::prompt_secret("That was not it. Password (not shown): ")?);
            }
            Err(e) => bail!("{}: {e}", device.name),
        }
    }
}

/// The Kagaz window, started on a device's screen. It is the program
/// next to this one; the window does the connecting and the asking.
fn open_window(which: &str) -> Result<()> {
    let device = Devices::load(&list_path())?.find(which)?.clone();
    if remote::session::way_for(&device).is_none() {
        bail!("{} has no way in that shows its screen (Remote Desktop or VNC)", device.name);
    }
    let name = if cfg!(windows) { "kagaz-desktop.exe" } else { "kagaz-desktop" };
    let window = std::env::current_exe().context("finding this program")?.with_file_name(name);
    if !window.exists() {
        bail!("the Kagaz window ({name}) is not next to this program, in {}", window.parent().map(|p| p.display().to_string()).unwrap_or_default());
    }
    std::process::Command::new(&window)
        .env("KAGAZ_OPEN", format!("remote:{}", device.id))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", window.display()))?;
    println!("Opening {} in the Kagaz window.", device.name);
    Ok(())
}

fn picture_words(picture: u8) -> String {
    match picture {
        0 => "sharpest".into(),
        10 => "smoothest".into(),
        n => format!("{n} of 10 towards smoother"),
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

/// A session in numbers, a line a second: how often the picture came,
/// how much of the screen changed, how much travelled, the delay.
fn try_session(which: &str, seconds: u64, picture: Option<u8>) -> Result<()> {
    let (device, answer, session) = open_session(which, picture)?;
    let (width, height) = session.screen.size();
    println!(
        "{} · {} over {} at {}",
        device.name,
        answer.way.words(),
        device.paths[answer.path].kind.label().to_lowercase(),
        answer.addr
    );
    let name = if session.info.name.is_empty() { String::new() } else { format!(", \"{}\"", session.info.name) };
    println!("screen {width} x {height}{name}");
    if !session.info.encrypted && device.paths[answer.path].kind != PathKind::Tailscale {
        println!("not encrypted on this network: the screen and what is typed can be read by others on it");
    }
    let started = Instant::now();
    let mut last = session.numbers.read();
    let mut ended = None;
    for second in 1..=seconds.max(1) {
        let until = started + Duration::from_secs(second);
        let (mut pieces, mut moves, mut pixels) = (0u32, 0u32, 0u64);
        while Instant::now() < until && ended.is_none() {
            session.screen.wait(until.saturating_duration_since(Instant::now()).min(Duration::from_millis(50)));
            let taken = session.screen.take();
            for piece in &taken.pieces {
                pieces += 1;
                match piece {
                    Piece::Picture { rect, .. } => pixels += rect.area(),
                    Piece::Move { .. } => moves += 1,
                    Piece::Size { .. } => {}
                }
            }
            for event in taken.events {
                match event {
                    Event::Ended(why) => ended = Some(why),
                    Event::Clipboard(text) => println!("  the device copied {} characters", text.chars().count()),
                    Event::Pointer(p) => println!("  its pointer is {} x {}", p.width, p.height),
                    Event::PointerDefault => println!("  its pointer is the ordinary arrow"),
                    Event::Bell => println!("  it rang its bell"),
                }
            }
        }
        let now = session.numbers.read();
        let whole = (width as u64 * height as u64).max(1);
        // Sharper or smoother is said where it can be chosen; a Remote
        // Desktop computer chooses for itself.
        let picture = if session.info.picture_setting { format!("  picture {}", picture_words(now.picture)) } else { String::new() };
        println!(
            "{second:>3} s  {:>3} updates  {:>4} areas ({} moved)  {:>5.1} screens painted  {:>6.2} Mbit/s  delay {}{picture}",
            now.updates - last.updates,
            pieces,
            moves,
            pixels as f64 / whole as f64,
            (now.received - last.received) as f64 * 8.0 / 1e6,
            now.delay.map_or("not measured yet".into(), |d| format!("{d} ms")),
        );
        last = now;
        if let Some(why) = &ended {
            println!("the session ended: {why}");
            break;
        }
    }
    let all = session.numbers.read();
    println!(
        "{} updates in {:.1} s, {} received, {} sent",
        all.updates,
        all.seconds,
        megabytes(all.received),
        megabytes(all.sent)
    );
    Ok(())
}

/// Text to a device's clipboard: the argument, or what is piped in.
fn clip(which: &str, text: Option<String>) -> Result<()> {
    let text = match text {
        Some(t) => t,
        None => {
            let mut all = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut all).context("reading the text to send")?;
            all
        }
    };
    if text.is_empty() {
        bail!("there is no text to send");
    }
    let (device, _, session) = open_session(which, None)?;
    // Remote Desktop hands text over when it is asked for, which a
    // computer does when something is pasted there: the session has to
    // stay open until then.
    if session.info.speaks == Speaks::Rdp {
        const HOLD: u64 = 30;
        std::thread::sleep(Duration::from_millis(400));
        session.send(Input::Clipboard(text.clone()));
        println!(
            "Offered {} characters to {}. Remote Desktop hands text over when something is pasted there, so Kagaz \
             stays connected for {HOLD} seconds: paste on the computer now.",
            text.chars().count(),
            device.name
        );
        let end = Instant::now() + Duration::from_secs(HOLD);
        while Instant::now() < end {
            session.screen.wait(Duration::from_millis(200));
            if let Some(Event::Ended(why)) = session.screen.take().events.into_iter().find(|e| matches!(e, Event::Ended(_))) {
                bail!("the session ended: {why}");
            }
        }
        return Ok(());
    }
    // A moment for the device to say how it takes clipboard text, then
    // the text, then a moment for it to leave.
    std::thread::sleep(Duration::from_millis(400));
    let before = session.numbers.read().sent;
    session.send(Input::Clipboard(text.clone()));
    let end = Instant::now() + Duration::from_secs(3);
    while session.numbers.read().sent == before && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    if session.numbers.read().sent == before {
        bail!("the text did not leave for {}", device.name);
    }
    std::thread::sleep(Duration::from_millis(300));
    println!("Sent {} characters to {}'s clipboard.", text.chars().count(), device.name);
    if session.info.speaks == Speaks::Vnc {
        println!("VNC gives no word back on whether it was taken; some VNC servers keep plain letters only.");
    }
    Ok(())
}

fn parse_size(size: &str) -> Result<(u16, u16)> {
    let (w, h) = size
        .split_once(['x', 'X', '×'])
        .ok_or_else(|| anyhow!("a size is a width and a height, as 1280x720"))?;
    let (w, h): (u16, u16) = (w.trim().parse()?, h.trim().parse()?);
    if !(16..=8192).contains(&w) || !(16..=8192).contains(&h) {
        bail!("a size between 16x16 and 8192x8192");
    }
    Ok((w, h))
}

/// The test card over VNC until Ctrl+C.
fn pattern(port: u16, size: &str, password: Option<String>, rate: u32, open: bool) -> Result<()> {
    use kagaz_core::remote::vnc::server::{Heard, Options, Server};
    let (width, height) = parse_size(size)?;
    let host = if open { "0.0.0.0" } else { "127.0.0.1" };
    let listener = std::net::TcpListener::bind((host, port)).with_context(|| format!("port {port} is taken"))?;
    let server = Server::start(
        listener,
        Options {
            width,
            height,
            password,
            ticks_per_second: rate.min(120),
            ..Options::default()
        },
    )?;
    println!("A test card of {width} x {height} over VNC on port {port}; Ctrl+C stops it.");
    println!("Add it once with: kagaz remote add 127.0.0.1 --vnc {port} --name \"Test card\"");
    if open {
        println!("Other devices on this network can connect to it too, and VNC is not encrypted.");
    }
    println!("A press shows as a square: red for the left button, green the middle, blue the right.");
    // What viewers do, in a line each time it changes kind.
    let mut told = 0;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let heard = server.heard();
        let (mut moves, mut presses, mut keys, mut texts) = (0, 0, 0, 0);
        for h in &heard[told..] {
            match h {
                Heard::Pointer { buttons: 0, .. } => moves += 1,
                Heard::Pointer { .. } => presses += 1,
                Heard::Key { .. } => keys += 1,
                Heard::Text(_) => texts += 1,
                Heard::Encodings(_) => println!("a viewer connected"),
            }
        }
        told = heard.len();
        if moves + presses + keys + texts > 0 {
            println!("heard {moves} pointer moves, {presses} with a button down, {keys} key events, {texts} clipboard texts");
        }
    }
}

/// One known-hosts entry per device, whichever address it is reached at.
fn host_key_alias(d: &Device) -> String {
    format!("HostKeyAlias=kagaz-{}", d.id)
}

fn target(d: &Device, a: &Answer) -> String {
    match &d.user {
        Some(user) => format!("{user}@{}", a.addr.ip()),
        None => a.addr.ip().to_string(),
    }
}

fn ssh(which: &str, command: &[String]) -> Result<()> {
    let (d, a) = locate_ssh(which)?;
    eprintln!("{} over {} ({} ms).", d.name, d.paths[a.path].words(), a.millis);
    let status = std::process::Command::new("ssh")
        .arg("-p")
        .arg(a.addr.port().to_string())
        .arg("-o")
        .arg(host_key_alias(&d))
        .arg(target(&d, &a))
        .args(command)
        .status()
        .context("the system's ssh could not be started (is OpenSSH's client installed?)")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn copy(which: &str, from: &str, to: &str) -> Result<()> {
    if from.starts_with(':') == to.starts_with(':') {
        bail!("one side is here and the other on the device: put a colon before the one on the device, as in `:Documents/report.pdf`");
    }
    let (d, a) = locate_ssh(which)?;
    eprintln!("{} over {} ({} ms).", d.name, d.paths[a.path].words(), a.millis);
    let host = match a.addr.ip() {
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        v4 => v4.to_string(),
    };
    let there = |side: &str| match &d.user {
        Some(user) => format!("{user}@{host}{side}"),
        None => format!("{host}{side}"),
    };
    let side = |s: &str| if s.starts_with(':') { there(s) } else { s.to_string() };
    let status = std::process::Command::new("scp")
        .arg("-P")
        .arg(a.addr.port().to_string())
        .arg("-o")
        .arg(host_key_alias(&d))
        .arg("-r")
        .arg(side(from))
        .arg(side(to))
        .status()
        .context("the system's scp could not be started (is OpenSSH's client installed?)")?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        action: RemoteAction,
    }

    #[test]
    fn add_takes_ports_with_and_without_a_value() {
        let cli = Cli::try_parse_from(["x", "add", "192.0.2.20", "--rdp", "--vnc", "5901", "--name", "Study laptop"]).unwrap();
        let RemoteAction::Add { host, name, rdp, vnc, ssh, .. } = cli.action else {
            panic!("not add");
        };
        assert_eq!(host, "192.0.2.20");
        assert_eq!(name.as_deref(), Some("Study laptop"));
        assert_eq!((rdp, vnc, ssh), (Some(3389), Some(5901), None));
    }

    #[test]
    fn a_size_is_a_width_and_a_height() {
        assert_eq!(parse_size("1280x720").unwrap(), (1280, 720));
        assert_eq!(parse_size("360 X 800").unwrap(), (360, 800));
        assert!(parse_size("1280").is_err());
        assert!(parse_size("4x4").is_err());
        assert!(parse_size("99999x720").is_err());
    }

    #[test]
    fn the_picture_setting_in_words() {
        assert_eq!(picture_words(0), "sharpest");
        assert_eq!(picture_words(10), "smoothest");
        assert_eq!(picture_words(4), "4 of 10 towards smoother");
    }

    #[test]
    fn ssh_passes_a_command_after_two_dashes() {
        let cli = Cli::try_parse_from(["x", "ssh", "study", "--", "uname", "-a"]).unwrap();
        let RemoteAction::Ssh { device, command } = cli.action else {
            panic!("not ssh");
        };
        assert_eq!(device, "study");
        assert_eq!(command, ["uname", "-a"]);
    }
}
