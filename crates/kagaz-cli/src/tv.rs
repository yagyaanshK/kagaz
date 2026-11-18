//! `kagaz tvs` and `kagaz tv`: TVs & displays from the command line.

use anyhow::{anyhow, bail, Result};
use clap::{Subcommand, ValueEnum};
use kagaz_core::tv::miracast::{video, Event, Options, Pictures};
use kagaz_core::tv::send::{self, Said};
use kagaz_core::tv::{self, Tv};
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

#[derive(Clone, Copy, ValueEnum)]
pub enum Power {
    /// Less power: the graphics chip's low-power encoder where it has one, at
    /// most 30 pictures a second
    Battery,
    /// The smoothest and sharpest the TV takes
    Performance,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum EncoderChoice {
    /// The graphics chip when it can, else the processor
    Auto,
    /// Only the graphics chip
    Hardware,
    /// Only the processor (OpenH264)
    Software,
}

impl From<Power> for video::Priority {
    fn from(p: Power) -> Self {
        match p {
            Power::Battery => video::Priority::Battery,
            Power::Performance => video::Priority::Performance,
        }
    }
}

impl From<EncoderChoice> for video::Engine {
    fn from(e: EncoderChoice) -> Self {
        match e {
            EncoderChoice::Auto => video::Engine::Auto,
            EncoderChoice::Hardware => video::Engine::Hardware,
            EncoderChoice::Software => video::Engine::Software,
        }
    }
}

#[derive(Subcommand)]
pub enum TvsAction {
    /// Show or change how Kagaz sends pictures to TVs
    Settings {
        /// Optimise for battery or for performance (Kagaz's power setting)
        #[arg(long, value_enum)]
        power: Option<Power>,
        /// Which encoder makes the pictures
        #[arg(long, value_enum)]
        encoder: Option<EncoderChoice>,
    },
    /// Be a stand-in TV: this computer announces itself on the network as
    /// a screen-mirroring TV, takes a sender's session (Kagaz on a phone,
    /// Windows' Cast) and reports what arrives, in numbers only (no
    /// picture is shown or kept). For checking a sender without a TV.
    /// Ctrl+C ends it.
    StandIn {
        /// The name senders list it under
        #[arg(long, default_value = "Kagaz stand-in TV")]
        name: String,
    },
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
        /// The tallest picture to send: 1080 is sharper, 720 can move more
        /// smoothly (60 pictures a second on TVs that take that only at 720)
        #[arg(long, value_enum, default_value = "1080")]
        size: Size,
        /// Stop after this many seconds
        #[arg(long)]
        seconds: Option<u64>,
        /// The name the TV shows for this computer
        #[arg(long, default_value = "Kagaz")]
        name: String,
        /// Optimise for battery or performance this time (default: Kagaz's
        /// setting, `kagaz tvs settings`)
        #[arg(long, value_enum)]
        power: Option<Power>,
        /// Which encoder this time (default: Kagaz's setting)
        #[arg(long, value_enum)]
        encoder: Option<EncoderChoice>,
    },
    /// Open the system's own screen sender instead of Kagaz's: Windows' Cast
    /// panel (Win+K), GNOME Network Displays on Linux where installed, AirPlay
    /// on a Mac
    System,
    /// Use the TV as a second monitor: the desktop makes a new monitor of the
    /// TV's size (see it in the display settings, drag windows onto it).
    /// Ctrl+C ends it and the monitor goes away.
    Extend {
        /// How to reach the TV; by default the network when the TV is on it,
        /// Wi-Fi Direct otherwise
        #[arg(long, value_enum)]
        via: Option<Via>,
        /// The new monitor's size
        #[arg(long, value_enum, default_value = "1080")]
        size: Size,
        /// Stop after this many seconds
        #[arg(long)]
        seconds: Option<u64>,
        /// The name the TV shows for this computer
        #[arg(long, default_value = "Kagaz")]
        name: String,
        /// Optimise for battery or performance this time (default: Kagaz's
        /// setting, `kagaz tvs settings`)
        #[arg(long, value_enum)]
        power: Option<Power>,
        /// Which encoder this time (default: Kagaz's setting)
        #[arg(long, value_enum)]
        encoder: Option<EncoderChoice>,
    },
}

fn height(size: Size) -> u32 {
    match size {
        Size::P720 => 720,
        Size::P1080 => 1080,
    }
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

#[cfg(windows)]
fn stop_on_ctrl_c() {
    unsafe extern "system" fn on_ctrl(_: u32) -> windows_sys::core::BOOL {
        STOP.store(true, Ordering::SeqCst);
        1
    }
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_ctrl), 1);
    }
}

#[cfg(not(any(unix, windows)))]
fn stop_on_ctrl_c() {}

fn ways(tv: &Tv) -> String {
    tv.ways.iter().map(|w| w.label()).collect::<Vec<_>>().join(", ")
}

/// `kagaz tvs settings` and `kagaz tvs stand-in`
pub fn settings(action: TvsAction) -> Result<()> {
    let (power, encoder) = match action {
        TvsAction::Settings { power, encoder } => (power, encoder),
        TvsAction::StandIn { name } => {
            stop_on_ctrl_c();
            return kagaz_core::tv::miracast::stand_in::run(&name, &STOP, |l| println!("{l}")).map_err(|e| anyhow!(e));
        }
    };
    if let Some(p) = power {
        kagaz_core::paths::set_setting("power", Some(video::Priority::from(p).name()))?;
    }
    if let Some(e) = encoder {
        kagaz_core::paths::set_setting("tv_encoder", Some(video::Engine::from(e).name()))?;
    }
    println!("Power: optimise for {}.", video::Priority::from_settings().name());
    println!("Encoder: {}.", video::Engine::from_settings().name());
    Ok(())
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
        TvAction::Mirror { pattern, window, via, size, seconds, name: shown, power, encoder } => {
            let engine = encoder.map(Into::into).unwrap_or_else(video::Engine::from_settings);
            let what = if pattern { What::Pattern } else if window { What::Window } else { What::Screen };
            let mut pictures = pictures(what, size, engine)?;
            let opts = Options {
                name: shown,
                max_height: height(size),
                engine,
                priority: power.map(Into::into).unwrap_or_else(video::Priority::from_settings),
                length: seconds.map(Duration::from_secs),
                wait: Duration::from_secs(30),
            };
            stop_on_ctrl_c();
            mirror(name, via, &opts, pictures.as_mut())
        }
        TvAction::System => {
            let said = kagaz_core::tv::open_system_sender().map_err(|e| anyhow!(e))?;
            println!("{}", said.replace("the TV", &format!("\"{name}\"")));
            Ok(())
        }
        TvAction::Extend { via, size, seconds, name: shown, power, encoder } => {
            let engine = encoder.map(Into::into).unwrap_or_else(video::Engine::from_settings);
            let mut pictures = pictures(What::Extend, size, engine)?;
            let opts = Options {
                name: shown,
                max_height: height(size),
                engine,
                priority: power.map(Into::into).unwrap_or_else(video::Priority::from_settings),
                length: seconds.map(Duration::from_secs),
                wait: Duration::from_secs(30),
            };
            stop_on_ctrl_c();
            mirror(name, via, &opts, pictures.as_mut())
        }
    }
}

enum What {
    Pattern,
    Screen,
    Window,
    Extend,
}

/// The pattern, or the screen the owner picks in the desktop's dialog, or
/// a new monitor for the TV.
fn pictures(what: What, size: Size, engine: video::Engine) -> Result<Box<dyn Pictures>> {
    if let What::Pattern = what {
        return Ok(Box::new(kagaz_core::tv::miracast::video::Pattern::new(16, 16)));
    }
    #[cfg(target_os = "linux")]
    {
        use kagaz_core::tv::capture::{Choice, Screen};
        let choice = match what {
            What::Window => Choice::Window,
            What::Extend => {
                let (width, height) = if height(size) > 720 { (1920, 1080) } else { (1280, 720) };
                Choice::Extend { width, height }
            }
            _ => Choice::Screen,
        };
        println!("Allow it in the desktop's sharing dialog...");
        // Pictures stay in graphics memory when the graphics chip encodes.
        let gpu = engine.wants_gpu_pictures();
        let screen = Screen::ask(choice, gpu).map_err(|e| anyhow!(e))?;
        match screen.first_picture(Duration::from_secs(5)) {
            Some((w, h)) if matches!(what, What::Extend) => {
                println!("A new {w}x{h} monitor is on the desktop: drag windows onto it.")
            }
            Some((w, h)) => println!("Sharing {w}x{h}."),
            None => bail!("the desktop sent no picture of the screen"),
        }
        Ok(Box::new(screen))
    }
    #[cfg(windows)]
    {
        let _ = engine;
        if !matches!(what, What::Screen) {
            bail!("on Windows Kagaz shows the main screen so far; a window or a new monitor for the TV come later");
        }
        let _ = size;
        let mut screen = kagaz_core::tv::capture_win::Screen::main().map_err(|e| anyhow!(e))?;
        match screen.first_picture(Duration::from_secs(5)) {
            Some((w, h)) => println!("Sharing the main screen, {w}x{h}."),
            None => bail!("Windows sent no picture of the screen"),
        }
        Ok(Box::new(screen))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (size, engine);
        if !matches!(what, What::Screen) {
            bail!("on a Mac Kagaz shows the main display so far; a window or a new monitor for the TV come later");
        }
        let screen = kagaz_core::tv::capture_mac::Screen::main().map_err(|e| anyhow!(e))?;
        match screen.first_picture(Duration::from_secs(5)) {
            Some((w, h)) => println!("Sharing the main display, {w}x{h}."),
            None => bail!("macOS sent no picture of the screen"),
        }
        Ok(Box::new(screen))
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = (size, engine);
        bail!("showing the screen works on Linux so far; `--pattern` sends the test pattern")
    }
}

fn on_said(s: Said) {
    match s {
        Said::Line(l) => println!("{l}"),
        Said::Event(e) => {
            if let Some(l) = send::describe(&e) {
                println!("{}", if matches!(e, Event::Playing(_)) { format!("{l} Ctrl+C ends it.") } else { l });
            }
        }
    }
}

fn mirror(name: &str, via: Option<Via>, opts: &Options, pictures: &mut dyn Pictures) -> Result<()> {
    let via = via.map(|v| match v {
        Via::Network => send::Via::Network,
        Via::Direct => send::Via::Direct,
    });
    let report = send::mirror_named(name, via, opts, pictures, &STOP, on_said).map_err(|e| {
        anyhow!(if e.starts_with("no TV called") { format!("{e} (`kagaz tvs` lists the ones that do)") } else { e })
    })?;
    println!("{}", send::ended(&report));
    Ok(())
}
