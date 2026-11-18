//! `kagaz tvs` and `kagaz tv`: TVs & displays from the command line.

use anyhow::{anyhow, bail, Result};
use clap::{Subcommand, ValueEnum};
use kagaz_core::tv::miracast::{self, Event, Options, Pictures, Route};
use kagaz_core::tv::{self, Tv, Way};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, ValueEnum)]
pub enum Size {
    #[value(name = "720")]
    P720,
    #[value(name = "1080")]
    P1080,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Via {
    /// The ordinary network, when the TV is on it
    Network,
    /// Wi-Fi Direct: a link of this computer's own, for a TV on no network
    Direct,
}

#[derive(Subcommand)]
pub enum TvAction {
    /// Show this computer's screen on the TV (Miracast). The desktop asks
    /// which screen or window to share. Ctrl+C ends it.
    Mirror {
        /// Send the test pattern (colour bars, a moving square and a clock)
        /// instead of this computer's screen
        #[arg(long, conflicts_with = "window")]
        pattern: bool,
        /// Share one window instead of a whole screen
        #[arg(long)]
        window: bool,
        /// How to reach the TV; by default the network when the TV is on it,
        /// Wi-Fi Direct otherwise
        #[arg(long, value_enum)]
        via: Option<Via>,
        /// The tallest picture to send
        #[arg(long, value_enum, default_value = "720")]
        size: Size,
        /// Stop after this many seconds
        #[arg(long)]
        seconds: Option<u64>,
        /// The name the TV shows for this computer
        #[arg(long, default_value = "Kagaz")]
        name: String,
    },
}

static STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn stop_on_ctrl_c() {
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }
    let handler: extern "C" fn(libc::c_int) = on_signal;
    unsafe {
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
    }
}

#[cfg(not(unix))]
fn stop_on_ctrl_c() {}

fn ways(tv: &Tv) -> String {
    tv.ways.iter().map(|w| w.label()).collect::<Vec<_>>().join(", ")
}

/// `kagaz tvs`
pub fn list(seconds: f32, direct: bool, json: bool) -> Result<()> {
    let tvs = tv::nearby(Duration::from_secs_f32(seconds), direct).map_err(|e| anyhow!(e))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&tvs)?);
        return Ok(());
    }
    if tvs.is_empty() {
        println!("No TV answered.");
        println!("A TV that is asleep, on the other Wi-Fi band, or on no network may not show here.");
        if !direct {
            println!("To find a TV on no network, switch on its screen sharing and run `kagaz tvs --direct`.");
        }
        return Ok(());
    }
    for t in &tvs {
        let model = t.model.as_deref().map(|m| format!(" ({m})")).unwrap_or_default();
        let place = match t.address {
            Some(a) => format!("at {a}"),
            None => "not on this network: screen mirroring only".into(),
        };
        println!("{}{model}, {place}", t.name);
        println!("  {}", ways(t));
    }
    Ok(())
}

/// `kagaz tv NAME ...`
pub fn tv(name: &str, action: TvAction) -> Result<()> {
    match action {
        TvAction::Mirror { pattern, window, via, size, seconds, name: shown } => {
            let mut pictures = pictures(pattern, window)?;
            let opts = Options {
                name: shown,
                max_height: match size {
                    Size::P720 => 720,
                    Size::P1080 => 1080,
                },
                length: seconds.map(Duration::from_secs),
                wait: Duration::from_secs(30),
            };
            stop_on_ctrl_c();
            mirror(name, via, &opts, pictures.as_mut())
        }
    }
}

/// The pattern, or the screen the owner picks in the desktop's dialog.
fn pictures(pattern: bool, window: bool) -> Result<Box<dyn Pictures>> {
    if pattern {
        return Ok(Box::new(kagaz_core::tv::miracast::video::Pattern::new(16, 16)));
    }
    #[cfg(target_os = "linux")]
    {
        use kagaz_core::tv::capture::{Choice, Screen};
        println!("Choose what to show in the desktop's sharing dialog...");
        let screen = Screen::ask(if window { Choice::Window } else { Choice::Screen }).map_err(|e| anyhow!(e))?;
        match screen.first_picture(Duration::from_secs(5)) {
            Some((w, h)) => println!("Sharing {w}x{h}."),
            None => bail!("the desktop sent no picture of the screen"),
        }
        Ok(Box::new(screen))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = window;
        bail!("showing the screen works on Linux so far; `--pattern` sends the test pattern")
    }
}

fn find_one(tvs: Vec<Tv>, name: &str) -> Result<Tv> {
    let mut hits: Vec<Tv> = tvs.into_iter().filter(|t| t.matches(name)).collect();
    match hits.len() {
        0 => bail!("no TV called \"{name}\" answered (`kagaz tvs` lists the ones that do)"),
        1 => Ok(hits.remove(0)),
        _ => bail!(
            "\"{name}\" fits more than one TV: {}",
            hits.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn on_event(e: Event) {
    match e {
        Event::Waiting => println!("Waiting for the TV to connect (it may ask to allow this computer)..."),
        Event::Connected(_) => println!("The TV connected."),
        Event::Abilities(a) => {
            let mut notes = Vec::new();
            if a.hdcp {
                notes.push("offers HDCP; Kagaz sends without it");
            }
            if a.uibc {
                notes.push("can send a mouse and keyboard back");
            }
            if !notes.is_empty() {
                println!("The TV {}.", notes.join(", "));
            }
        }
        Event::Playing(m) => println!("Playing at {}x{}, {} pictures a second. Ctrl+C ends it.", m.width, m.height, m.fps),
        Event::Progress { seconds, frames, kilobits_per_second } => {
            if seconds % 10 == 0 {
                println!("  {seconds} s, {frames} pictures, {kilobits_per_second} kbit/s");
            }
        }
        Event::KeyFrameAsked => println!("  The TV asked for a fresh picture."),
    }
}

fn mirror(name: &str, via: Option<Via>, opts: &Options, pictures: &mut dyn Pictures) -> Result<()> {
    // A typed address goes straight to the network route.
    if let (Ok(ip), None | Some(Via::Network)) = (name.parse::<IpAddr>(), via) {
        return run(Route::Network(ip), opts, pictures, || Ok(()));
    }
    if !matches!(via, Some(Via::Direct)) {
        let tvs = tv::nearby(Duration::from_secs(4), false).map_err(|e| anyhow!(e))?;
        let found = find_one(tvs, name);
        match found {
            Ok(t) if t.has(Way::MiracastNetwork) => {
                println!("Mirroring to {} over the network.", t.name);
                return run(Route::Network(t.address.expect("found on the network")), opts, pictures, || Ok(()));
            }
            Ok(t) if matches!(via, Some(Via::Network)) => {
                bail!(
                    "{} is not taking screen mirroring over the network right now (it may be starting up, \
                     or in standby); `--via direct` reaches it over Wi-Fi Direct",
                    t.name
                )
            }
            Err(e) if matches!(via, Some(Via::Network)) => return Err(e),
            _ => {}
        }
    }
    direct(name, opts, pictures)
}

#[cfg(target_os = "linux")]
fn direct(name: &str, opts: &Options, pictures: &mut dyn Pictures) -> Result<()> {
    use kagaz_core::tv::p2p;
    println!("Looking for {name} over Wi-Fi Direct (its screen sharing must be on)...");
    let q = name.to_lowercase();
    let peers = p2p::find(Duration::from_secs(30), |ps| {
        ps.iter().any(|p| p.display && p.name.to_lowercase().contains(&q))
    })
    .map_err(|e| anyhow!(e))?;
    let peer = peers
        .into_iter()
        .find(|p| p.display && p.name.to_lowercase().contains(&q))
        .ok_or_else(|| anyhow!("no TV called \"{name}\" showed over Wi-Fi Direct. Is its screen sharing on?"))?;
    println!("Mirroring to {} over Wi-Fi Direct.", peer.name);
    run(Route::Direct, opts, pictures, || {
        println!("Making the Wi-Fi Direct link (up to a minute)...");
        // A TV just out of another session can miss the first try (seen on
        // an LG); a second one usually takes seconds.
        p2p::connect(&peer, miracast::RTSP_PORT, Duration::from_secs(60)).or_else(|e| {
            println!("The link did not come up ({e}). Trying once more...");
            p2p::connect(&peer, miracast::RTSP_PORT, Duration::from_secs(60))
        })
    })
}

#[cfg(not(target_os = "linux"))]
fn direct(name: &str, _opts: &Options, _pictures: &mut dyn Pictures) -> Result<()> {
    bail!("{name} was not found on the network, and Wi-Fi Direct from Kagaz works on Linux only so far")
}

fn run<R>(
    route: Route,
    opts: &Options,
    pictures: &mut dyn Pictures,
    ready: impl FnOnce() -> Result<R, String>,
) -> Result<()> {
    let report = miracast::mirror(route, opts, pictures, &STOP, ready, on_event).map_err(|e| anyhow!(e))?;
    let by = match report.ended_by.as_str() {
        "you" => "you ended it",
        "time" => "the time ran out",
        "the TV" => "the TV ended it",
        _ => "it ended",
    };
    println!(
        "Mirroring stopped: {by}. {} pictures, {:.1} MB sent.",
        report.frames,
        report.bytes as f64 / 1_000_000.0
    );
    Ok(())
}
