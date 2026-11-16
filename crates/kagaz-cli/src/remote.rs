//! `kagaz remote`: the computers and phones you added, every way to reach
//! each and how it does now, and the system's own ssh and scp over
//! whichever path answers. The same list as the window's Computers &
//! phones.

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use kagaz_core::remote::tailscale::{self, Tailscale};
use kagaz_core::remote::{
    self, Answer, Device, Devices, LocateError, PathKind, PathReport, PathTo, Speaks, Trouble, Way,
};
use std::time::Duration;

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

/// The quickest path that answered for the way that shows the screen (or
/// for any way, when the device has none that does).
fn best(d: &Device, reports: &[PathReport]) -> Option<Answer> {
    let screen = d.screen_way();
    let mut ok: Vec<&Answer> = reports.iter().filter_map(|r| r.outcome.as_ref().ok()).collect();
    ok.sort_by_key(|a| (Some(a.way) != screen, a.millis));
    ok.first().map(|a| (*a).clone())
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
                if let Some(a) = reports.as_deref().and_then(|r| best(d, r)) {
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
                    let answer = best(d, reports);
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
            Some(reports) => match best(d, reports) {
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
    let tailscale = tailscale::status();
    let peer = tailscale
        .peers()
        .iter()
        .find(|p| p.address == host || p.name.eq_ignore_ascii_case(host) || p.dns_name.eq_ignore_ascii_case(host.trim_end_matches('.')));
    // A Tailscale device given by its short name is reached by its address.
    let reach_host = peer.map(|p| p.address.as_str()).unwrap_or(host);

    let mut ways: Vec<Way> = asked
        .iter()
        .filter_map(|(speaks, port)| port.map(|port| Way { speaks: *speaks, port }))
        .collect();
    let mut identity = Default::default();
    if ways.is_empty() {
        println!("Asking {reach_host} what it speaks…");
        let found = remote::detect(reach_host, QUICK);
        if found.is_empty() {
            bail!(
                "nothing answered at {reach_host} for Remote Desktop, VNC or SSH. Check that it is on and that remote access is switched on there; \
                 to add it anyway, say what it speaks: --rdp, --vnc or --ssh"
            );
        }
        for (way, _, seen) in &found {
            println!("  {}{}", way.words(), if seen.banner.is_empty() || way.speaks == Speaks::Rdp { String::new() } else { format!(" ({})", seen.banner) });
            if !seen.identity.is_empty() {
                identity = seen.identity.clone();
            }
            ways.push(*way);
        }
    }
    let system = peer.map(|p| p.os.to_ascii_lowercase()).unwrap_or_default();
    let name = name
        .or_else(|| peer.map(|p| p.name.clone()))
        .or_else(|| identity.name.clone())
        .unwrap_or_else(|| host.to_string());
    let mut paths = vec![PathTo::new(reach_host)];
    if hotspot {
        paths.push(PathTo::gateway());
    }
    let device = Device {
        name,
        system,
        ways,
        paths,
        identity,
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
    fn ssh_passes_a_command_after_two_dashes() {
        let cli = Cli::try_parse_from(["x", "ssh", "study", "--", "uname", "-a"]).unwrap();
        let RemoteAction::Ssh { device, command } = cli.action else {
            panic!("not ssh");
        };
        assert_eq!(device, "study");
        assert_eq!(command, ["uname", "-a"]);
    }

    #[test]
    fn the_best_answer_is_the_screen_way_then_the_quickest() {
        let d = Device {
            ways: vec![Way::new(Speaks::Ssh), Way::new(Speaks::Rdp)],
            paths: vec![PathTo::new("192.0.2.20"), PathTo::new("100.64.0.7")],
            ..Default::default()
        };
        let answer = |path: usize, speaks: Speaks, millis: u32| PathReport {
            path,
            way: Way::new(speaks),
            outcome: Ok(Answer {
                path,
                way: Way::new(speaks),
                addr: "192.0.2.20:1".parse().unwrap(),
                millis,
                seen: remote::Seen { speaks, banner: String::new(), identity: Default::default() },
            }),
        };
        let reports = vec![answer(0, Speaks::Ssh, 2), answer(1, Speaks::Rdp, 40), answer(0, Speaks::Rdp, 9)];
        let b = best(&d, &reports).unwrap();
        assert_eq!((b.path, b.way.speaks, b.millis), (0, Speaks::Rdp, 9));
    }
}
