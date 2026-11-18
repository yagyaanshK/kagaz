//! Sending the screen to a TV named by its owner: over the network when
//! the TV is on it, else over Wi-Fi Direct (Linux for now), with a second
//! try when a Wi-Fi Direct link does not come up the first time. Used by
//! the command line and the app alike.

use super::miracast::{self, Event, Options, Pictures, Report, Route};
use super::{nearby, Tv, Way};
use std::net::IpAddr;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// How to reach the TV; `None` lets Kagaz choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Network,
    Direct,
}

/// What a session says as it goes: a line for the owner, or the session's
/// own news.
pub enum Said {
    Line(String),
    Event(Event),
}

/// The one TV that `name` fits among those found.
pub fn find_one(tvs: Vec<Tv>, name: &str) -> Result<Tv, String> {
    let mut hits: Vec<Tv> = tvs.into_iter().filter(|t| t.matches(name)).collect();
    match hits.len() {
        0 => Err(format!("no TV called \"{name}\" answered")),
        1 => Ok(hits.remove(0)),
        _ => Err(format!(
            "\"{name}\" fits more than one TV: {}",
            hits.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Mirrors to the TV called `name` (part of its name, or its address).
pub fn mirror_named(
    name: &str,
    via: Option<Via>,
    opts: &Options,
    pictures: &mut dyn Pictures,
    stop: &AtomicBool,
    mut say: impl FnMut(Said),
) -> Result<Report, String> {
    // A typed address goes straight to the network route.
    if let (Ok(ip), None | Some(Via::Network)) = (name.parse::<IpAddr>(), via) {
        return miracast::mirror(Route::Network(ip), opts, pictures, stop, || Ok(()), |e| say(Said::Event(e)));
    }
    if via != Some(Via::Direct) {
        let tvs = nearby(Duration::from_secs(4), false)?;
        match find_one(tvs, name) {
            Ok(t) if t.has(Way::MiracastNetwork) => {
                say(Said::Line(format!("Mirroring to {} over the network.", t.name)));
                let ip = t.address.ok_or("the TV gave no address")?;
                return miracast::mirror(Route::Network(ip), opts, pictures, stop, || Ok(()), |e| say(Said::Event(e)));
            }
            Ok(t) if via == Some(Via::Network) => {
                return Err(format!(
                    "{} is not taking screen mirroring over the network right now (it may be starting up, \
                     or in standby); Wi-Fi Direct reaches it",
                    t.name
                ))
            }
            Err(e) if via == Some(Via::Network) => return Err(e),
            _ => {}
        }
    }
    direct(name, opts, pictures, stop, say)
}

#[cfg(target_os = "linux")]
fn direct(
    name: &str,
    opts: &Options,
    pictures: &mut dyn Pictures,
    stop: &AtomicBool,
    mut say: impl FnMut(Said),
) -> Result<Report, String> {
    use super::p2p;
    say(Said::Line(format!("Looking for {name} over Wi-Fi Direct (its screen sharing must be on)...")));
    let q = name.to_lowercase();
    let peers = p2p::find(Duration::from_secs(30), |ps| ps.iter().any(|p| p.display && p.name.to_lowercase().contains(&q)))?;
    let peer = peers
        .into_iter()
        .find(|p| p.display && p.name.to_lowercase().contains(&q))
        .ok_or_else(|| format!("no TV called \"{name}\" showed over Wi-Fi Direct. Is its screen sharing on?"))?;
    say(Said::Line(format!("Mirroring to {} over Wi-Fi Direct.", peer.name)));
    // Lines from the link's set-up, said with the next news.
    let lines = std::cell::RefCell::new(Vec::new());
    miracast::mirror(
        Route::Direct,
        opts,
        pictures,
        stop,
        || {
            lines.borrow_mut().push("Making the Wi-Fi Direct link (up to a minute)...".to_string());
            // A TV just out of another session can miss the first try (seen
            // on an LG); a second one usually takes seconds.
            p2p::connect(&peer, miracast::RTSP_PORT, Duration::from_secs(60)).or_else(|e| {
                lines.borrow_mut().push(format!("The link did not come up ({e}). Trying once more..."));
                p2p::connect(&peer, miracast::RTSP_PORT, Duration::from_secs(60))
            })
        },
        |e| {
            for l in lines.borrow_mut().drain(..) {
                say(Said::Line(l));
            }
            say(Said::Event(e))
        },
    )
}

#[cfg(not(target_os = "linux"))]
fn direct(
    name: &str,
    _opts: &Options,
    _pictures: &mut dyn Pictures,
    _stop: &AtomicBool,
    _say: impl FnMut(Said),
) -> Result<Report, String> {
    Err(format!(
        "{name} was not found on the network, and Wi-Fi Direct from Kagaz works on Linux only so far; \
         this system's own sender can reach it"
    ))
}

/// A line for the owner about a session's news (the command line prints
/// these; the app shows them).
pub fn describe(e: &Event) -> Option<String> {
    Some(match e {
        Event::Waiting => "Waiting for the TV to connect (it may ask to allow this device)...".into(),
        Event::Connected(_) => "The TV connected.".into(),
        Event::Abilities(a) => {
            let mut notes = Vec::new();
            if a.hdcp {
                notes.push("offers HDCP; Kagaz sends without it");
            }
            if a.uibc {
                notes.push("can send a mouse and keyboard back");
            }
            if notes.is_empty() {
                return None;
            }
            format!("The TV {}.", notes.join(", "))
        }
        Event::Encoder(which) => format!("Encoding on {which}."),
        Event::Playing(m) => format!("Playing at {}x{}, {} pictures a second.", m.width, m.height, m.fps),
        Event::Progress { seconds, frames, kilobits_per_second, busy_ms, worst_ms, budget_ms, dropped, delivered_per_second } => {
            format!(
                "  {seconds} s, {frames} pictures, {kilobits_per_second} kbit/s; the screen gave \
                 {delivered_per_second}/s; each took {busy_ms:.1} ms (worst {worst_ms:.0}) of {budget_ms:.1}; \
                 {dropped} dropped"
            )
        }
        Event::KeyFrameAsked => "  The TV asked for a fresh picture.".into(),
    })
}

/// A line saying how a session ended.
pub fn ended(report: &Report) -> String {
    let by = match report.ended_by.as_str() {
        "you" => "you ended it",
        "time" => "the time ran out",
        "the TV" => "the TV ended it",
        _ => "it ended",
    };
    format!("Mirroring stopped: {by}. {} pictures, {:.1} MB sent.", report.frames, report.bytes as f64 / 1_000_000.0)
}
