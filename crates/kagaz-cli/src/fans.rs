//! `kagaz fans`, `kagaz fan` and `kagaz rooms`: ceiling fans (Atomberg)
//! from the command line, the same list, rooms and controls as the window.

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use kagaz_core::fans::atomberg::TIMER_HOURS;
use kagaz_core::fans::{self, Command as FanCommand, Fans, Found};

#[derive(Clone, Copy, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

impl From<OnOff> for bool {
    fn from(v: OnOff) -> bool {
        matches!(v, OnOff::On)
    }
}

#[derive(Subcommand)]
pub enum FanAction {
    /// Turn the fan on (at the speed it was on last)
    On,
    /// Turn the fan off
    Off,
    /// Set the speed, 1 to 5, or 6 for Boost (turns the fan on)
    Speed { speed: u8 },
    /// The fan's light on or off
    Light { state: OnOff },
    /// How bright the light is, 0 to 127 (fans whose light dims)
    Brightness { level: u8 },
    /// The light's colour: cool, warm or daylight (fans with colour modes)
    LightMode { mode: String },
    /// Sleep mode on or off (the fan slows through the night)
    Sleep { state: OnOff },
    /// Turn off after this many hours: 1, 2, 3 or 6; 0 cancels the timer
    Timer { hours: u8 },
    /// Boost on or off
    Boost { state: OnOff },
    /// Ask the fan how it is, without changing anything
    Read {
        /// Also print the fan's state string as it came, bit by bit
        #[arg(long)]
        raw: bool,
    },
    /// Remember a fan found nearby (by its id or the end of it), with a name and a room
    Add {
        /// What to call it ("Bedroom fan"); without it, the product name and the end of its id
        #[arg(long)]
        name: Option<String>,
        /// The room it is in; a new room is created
        #[arg(long)]
        room: Option<String>,
        /// Seconds to look for the fan
        #[arg(long, default_value_t = 6.0)]
        seconds: f32,
    },
    /// Stop remembering the fan
    Forget,
    /// Keep one connection open and send commands one at a time (on, off,
    /// speed 3, light on, brightness 50, light-mode warm, sleep on, timer 1,
    /// boost on, read, send <json>); a fan takes only so many connections
    /// before it needs switching off at the wall, so a session uses one
    Console {
        /// Walk through the commands in this file, one per line, asking
        /// before each and noting what the fan did
        #[arg(long)]
        steps: Option<std::path::PathBuf>,
        /// Append every command, the fan's answer and your notes to this file
        #[arg(long)]
        log: Option<std::path::PathBuf>,
    },
    /// Give the fan a new name
    Rename { name: String },
    /// Put the fan in a room (a new room is created); without a room, in none
    Room { room: Option<String> },
}

#[derive(Subcommand)]
pub enum RoomAction {
    /// Add a room
    Add { name: String },
    /// Rename a room (its fans move with it)
    Rename { old: String, new: String },
    /// Remove a room (its fans stay, in no room)
    Remove { name: String },
}

fn load() -> Result<Fans> {
    Fans::load(&Fans::default_path()).map_err(|e| anyhow!("{e}"))
}

fn save(fans: &Fans) -> Result<()> {
    fans.save(&Fans::default_path()).map_err(|e| anyhow!("{e}"))
}

/// Change the saved list as it is on disk now, and return it as saved.
fn update(
    change: impl FnOnce(&mut Fans) -> Result<(), kagaz_core::fans::store::StoreError>,
) -> Result<Fans> {
    Fans::update(&Fans::default_path(), change)
        .map(|((), fans)| fans)
        .map_err(|e| anyhow!("{e}"))
}

fn reach_word(f: &fans::Fan) -> &'static str {
    match f.last_reach.as_deref() {
        Some("wifi") => "Wi-Fi",
        Some(_) => "Bluetooth",
        None => "",
    }
}

/// `kagaz fans`: the remembered fans by room with how each is, then the
/// fans within reach that are not remembered.
pub fn list(seconds: f32, no_scan: bool, json: bool) -> Result<()> {
    let (found, trouble) = if no_scan {
        (Vec::new(), None)
    } else {
        fans::scan(seconds)
    };
    // The window may change the list meanwhile: each finding is noted on
    // the list as it is on disk then.
    let mut fans = if no_scan {
        load()?
    } else {
        update(|f| {
            f.note_seen(&found);
            Ok(())
        })?
    };
    // A fan seen over Bluetooth tells its state only when asked.
    let mut reachable: Vec<String> = found.iter().map(|f| f.id.clone()).collect();
    if !no_scan {
        let to_ask: Vec<fans::Fan> = found
            .iter()
            .filter(|f| f.state.is_none())
            .filter_map(|f| fans.find(&f.id).cloned())
            .collect();
        let mut answers = Vec::new();
        for fan in to_ask {
            match fans::read(&fan) {
                Ok((state, reach)) => answers.push((fan.id, state, reach)),
                Err(_) => reachable.retain(|id| id != &fan.id),
            }
        }
        fans = update(|f| {
            for (id, state, reach) in &answers {
                f.note_state(id, state, reach.label());
            }
            Ok(())
        })?;
    }
    if json {
        let others: Vec<&Found> = found
            .iter()
            .filter(|f| !fans.is_remembered(&f.id))
            .collect();
        let out = serde_json::json!({
            "rooms": fans.rooms,
            "fans": fans.fans.iter().map(|f| serde_json::json!({
                "id": f.id, "name": f.name, "room": f.room, "series": f.series,
                "product": f.profile().name,
                "reachable": reachable.contains(&f.id),
                "standing": fans::standing(f, reachable.contains(&f.id)),
                "state": f.last_state, "last_seen": f.last_seen, "reach": f.last_reach,
            })).collect::<Vec<_>>(),
            "found": others,
            "bluetooth": trouble.as_ref().map(|e| e.to_string()),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if let Some(e) = &trouble {
        println!("Bluetooth: {e}. Only fans on the Wi-Fi network can be found.");
    }
    if fans.fans.is_empty() {
        println!("No fans added yet.");
    } else {
        println!("My fans");
        let mut rooms: Vec<String> = fans.rooms.clone();
        rooms.push(String::new());
        for room in rooms {
            let in_room = fans.in_room(&room);
            if in_room.is_empty() && room.is_empty() {
                continue;
            }
            println!(
                "  {}",
                if room.is_empty() {
                    "No room"
                } else {
                    room.as_str()
                }
            );
            for f in in_room {
                let here = reachable.contains(&f.id);
                let how = if here { reach_word(f) } else { "" };
                println!(
                    "    {:<24} {:<28} {}",
                    f.name,
                    fans::standing(f, here),
                    if how.is_empty() {
                        String::new()
                    } else {
                        format!("({how})")
                    }
                );
            }
        }
    }
    let others: Vec<&Found> = found
        .iter()
        .filter(|f| !fans.is_remembered(&f.id))
        .collect();
    if !no_scan {
        if others.is_empty() {
            println!("No other fans within reach.");
        } else {
            println!("Found nearby (not added)");
            for f in &others {
                let how = match (&f.ble_address, &f.ip) {
                    (Some(_), Some(ip)) => format!("Bluetooth and Wi-Fi {ip}"),
                    (Some(_), None) => match f.rssi {
                        Some(r) => format!("Bluetooth, {r} dBm"),
                        None => "Bluetooth".to_string(),
                    },
                    (None, Some(ip)) => format!("Wi-Fi {ip}"),
                    (None, None) => String::new(),
                };
                println!(
                    "    {:<24} id {}  {}  {}",
                    f.suggested_name(),
                    f.id,
                    f.series,
                    how
                );
            }
            let first = &others[0];
            println!(
                "Add one: kagaz fan {} add --name \"{}\" --room \"Bedroom\"",
                &first.id[first.id.len().saturating_sub(4)..],
                first.suggested_name()
            );
        }
    }
    Ok(())
}

/// `kagaz fan <which> <action>`.
pub fn fan(which: &str, action: FanAction) -> Result<()> {
    let mut fans = load()?;
    let command = match action {
        FanAction::On => FanCommand::Power(true),
        FanAction::Off => FanCommand::Power(false),
        FanAction::Speed { speed } => FanCommand::Speed(speed),
        FanAction::Light { state } => FanCommand::Led(state.into()),
        FanAction::Brightness { level } => FanCommand::Brightness(level),
        FanAction::LightMode { mode } => FanCommand::LightMode(mode.to_ascii_lowercase()),
        FanAction::Sleep { state } => FanCommand::Sleep(state.into()),
        FanAction::Timer { hours } => {
            let i = TIMER_HOURS
                .iter()
                .position(|h| *h == hours)
                .ok_or_else(|| anyhow!("the timer takes 1, 2, 3 or 6 hours, or 0 for off"))?;
            FanCommand::Timer(i as u8)
        }
        FanAction::Boost { state } => FanCommand::Boost(state.into()),
        FanAction::Read { raw: true } => {
            let fan = fans
                .find(which)
                .cloned()
                .ok_or_else(|| anyhow!("no fan called \"{which}\"; kagaz fans lists them"))?;
            let (text, reach) = fans::send_raw(&fan, &FanCommand::Read)
                .with_context(|| format!("could not reach {}", fan.name))?;
            println!("{} over {}: {text}", fan.name, reach.label());
            if let Some(v) = text
                .split(',')
                .next()
                .and_then(|f| f.trim().parse::<i64>().ok())
            {
                let bits: Vec<String> = (0..24)
                    .filter(|b| v & (1 << b) != 0)
                    .map(|b| format!("0x{:x}", 1i64 << b))
                    .collect();
                println!("field 0 = {v} = 0x{v:x}; bits set: {}", bits.join(" "));
            }
            if let Some(state) = kagaz_core::fans::FanState::decode(&text) {
                println!("decoded as: {}", state.words());
                print_times(&state);
            }
            return Ok(());
        }
        FanAction::Read { raw: false } => FanCommand::Read,
        FanAction::Console { steps, log } => return console(&fans, which, steps, log),
        FanAction::Add {
            name,
            room,
            seconds,
        } => {
            let w = which.trim().to_ascii_lowercase();
            let (found, trouble) = fans::scan(seconds);
            let f = found
                .iter()
                .find(|f| f.id == w || (w.len() >= 4 && f.id.ends_with(&w)))
                .ok_or_else(|| match trouble {
                    Some(e) => {
                        anyhow!("no fan with id ending \"{which}\" within reach (Bluetooth: {e})")
                    }
                    None => anyhow!(
                        "no fan with id ending \"{which}\" within reach; kagaz fans lists them"
                    ),
                })?;
            let name = name.unwrap_or_else(|| f.suggested_name());
            let room = room.unwrap_or_default();
            let fan = fans.add(f, &name, &room)?.clone();
            save(&fans)?;
            println!(
                "Added {} ({} {}){}.",
                fan.name,
                fan.profile().name,
                fan.id,
                if fan.room.is_empty() {
                    String::new()
                } else {
                    format!(" in {}", fan.room)
                }
            );
            return Ok(());
        }
        FanAction::Forget => {
            let f = fans.forget(which)?;
            save(&fans)?;
            println!("Forgot {}.", f.name);
            return Ok(());
        }
        FanAction::Rename { name } => {
            fans.rename(which, &name)?;
            save(&fans)?;
            println!("Renamed to {name}.");
            return Ok(());
        }
        FanAction::Room { room } => {
            let room = room.unwrap_or_default();
            fans.set_room(which, &room)?;
            save(&fans)?;
            let f = fans.find(which).expect("just set");
            if room.is_empty() {
                println!("{} is in no room now.", f.name);
            } else {
                println!("{} is in {} now.", f.name, room);
            }
            return Ok(());
        }
    };
    let fan = fans
        .find(which)
        .cloned()
        .ok_or_else(|| anyhow!("no fan called \"{which}\"; kagaz fans lists them"))?;
    if fan.ble_address.is_none() && fan.ip.is_none() {
        bail!(
            "{} has never been reached; run kagaz fans with it in range first",
            fan.name
        );
    }
    let (state, reach) =
        fans::send(&fan, &command).with_context(|| format!("could not reach {}", fan.name))?;
    update(|f| {
        f.note_state(&fan.id, &state, reach.label());
        Ok(())
    })?;
    let how = match reach {
        fans::Reach::Wifi => "Wi-Fi",
        fans::Reach::Bluetooth => "Bluetooth",
    };
    if command.changes() {
        println!("{}: {} ({}, over {how})", fan.name, command, state.words());
    } else {
        println!("{}: {} (over {how})", fan.name, state.words());
        print_times(&state);
    }
    Ok(())
}

/// The fan's own count of its running and switched-off time.
fn print_times(state: &kagaz_core::fans::FanState) {
    use kagaz_core::fans::atomberg::span_words;
    if let Some(s) = state.run_seconds {
        println!("  has run for {} to date", span_words(s));
    }
    if let Some(s) = state.off_seconds {
        println!("  has been switched off for {} to date", span_words(s));
    }
}

/// `kagaz rooms [add|rename|remove]`.
pub fn rooms(action: Option<RoomAction>) -> Result<()> {
    let mut fans = load()?;
    match action {
        None => {
            if fans.rooms.is_empty() {
                println!("No rooms yet. kagaz rooms add \"Bedroom\"");
            }
            for r in &fans.rooms {
                let n = fans.in_room(r).len();
                println!("{r:<24} {n} fan{}", if n == 1 { "" } else { "s" });
            }
            let none = fans.in_room("").len();
            if none > 0 {
                println!(
                    "{:<24} {none} fan{}",
                    "(no room)",
                    if none == 1 { "" } else { "s" }
                );
            }
        }
        Some(RoomAction::Add { name }) => {
            fans.room_add(&name)?;
            save(&fans)?;
            println!("Added {name}.");
        }
        Some(RoomAction::Rename { old, new }) => {
            if !fans.rooms.contains(&old) {
                bail!("no room called \"{old}\"");
            }
            fans.room_rename(&old, &new)?;
            save(&fans)?;
            println!("{old} is {new} now.");
        }
        Some(RoomAction::Remove { name }) => {
            if !fans.rooms.contains(&name) {
                bail!("no room called \"{name}\"");
            }
            fans.room_remove(&name);
            save(&fans)?;
            println!("Removed {name}; its fans are in no room.");
        }
    }
    Ok(())
}

/// One line typed in the console, read as a fan action.
#[derive(clap::Parser)]
#[command(no_binary_name = true)]
struct ConsoleLine {
    #[command(subcommand)]
    action: FanAction,
}

/// What a console line asks the fan: JSON to send (None: just read).
fn console_json(line: &str) -> Result<Option<String>> {
    let line = line.trim();
    if line == "read" {
        return Ok(None);
    }
    if let Some(json) = line.strip_prefix("send ") {
        return Ok(Some(json.trim().to_string()));
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let parsed = ConsoleLine::try_parse_from(words).map_err(|e| {
        anyhow!(
            "{}",
            e.to_string().lines().next().unwrap_or("not understood")
        )
    })?;
    let command = match parsed.action {
        FanAction::On => FanCommand::Power(true),
        FanAction::Off => FanCommand::Power(false),
        FanAction::Speed { speed } => FanCommand::Speed(speed),
        FanAction::Light { state } => FanCommand::Led(state.into()),
        FanAction::Brightness { level } => FanCommand::Brightness(level),
        FanAction::LightMode { mode } => FanCommand::LightMode(mode.to_ascii_lowercase()),
        FanAction::Sleep { state } => FanCommand::Sleep(state.into()),
        FanAction::Timer { hours } => {
            let i = TIMER_HOURS
                .iter()
                .position(|h| *h == hours)
                .ok_or_else(|| anyhow!("the timer takes 1, 2, 3 or 6 hours, or 0 for off"))?;
            FanCommand::Timer(i as u8)
        }
        FanAction::Boost { state } => FanCommand::Boost(state.into()),
        FanAction::Read { .. } => return Ok(None),
        _ => bail!("not in the console: use kagaz fan for that"),
    };
    command.validate().map_err(|e| anyhow!("{e}"))?;
    Ok(Some(command.to_json()))
}

/// The answer laid out: the state string, its bits, and in words.
fn show_answer(text: &str) -> String {
    let mut out = format!("   fan: {text}\n");
    if let Some(v) = text
        .split(',')
        .next()
        .and_then(|f| f.trim().parse::<i64>().ok())
    {
        let bits: Vec<String> = (0..24)
            .filter(|b| v & (1 << b) != 0)
            .map(|b| format!("0x{:x}", 1i64 << b))
            .collect();
        out += &format!("   field 0 = 0x{v:x}: {}\n", bits.join(" "));
    }
    if let Some(state) = kagaz_core::fans::FanState::decode(text) {
        out += &format!("   Kagaz reads: {}\n", state.words());
    }
    out
}

#[cfg(not(target_os = "android"))]
fn console(
    fans: &Fans,
    which: &str,
    steps: Option<std::path::PathBuf>,
    log: Option<std::path::PathBuf>,
) -> Result<()> {
    use kagaz_core::fans::ble::Link;
    use std::io::{BufRead, Write};
    let fan = fans
        .find(which)
        .cloned()
        .ok_or_else(|| anyhow!("no fan called \"{which}\"; kagaz fans lists them"))?;
    let address = fan.ble_address.clone().ok_or_else(|| {
        anyhow!(
            "{} has no Bluetooth address yet; run kagaz fans with it in range",
            fan.name
        )
    })?;
    let mut log_file = match &log {
        Some(p) => Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .with_context(|| format!("cannot write {}", p.display()))?,
        ),
        None => None,
    };
    let mut note = |what: &str, answer: &str, said: &str| {
        if let Some(f) = log_file.as_mut() {
            let n = kagaz_core::localtime::now();
            let t = format!("{:02}:{:02}:{:02}", n.hour, n.minute, n.second);
            let _ = writeln!(f, "{t}\t{what}\t{answer}\t{said}");
        }
    };
    println!("Connecting to {}…", fan.name);
    let mut link = Link::open(&address).with_context(|| format!("could not reach {}", fan.name))?;
    println!("Connected. One connection for the whole session.");
    let stdin = std::io::stdin();
    let mut input = stdin.lock().lines();
    let mut ask = |prompt: &str| -> Option<String> {
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        input.next().and_then(|l| l.ok())
    };
    // Run one line on the kept connection (reconnecting once if the fan dropped it).
    let run = |link: &mut Link, line: &str| -> Result<String> {
        let json = console_json(line)?;
        match link.exchange(json.as_deref()) {
            Ok(text) => Ok(text),
            Err(e) if !link.is_connected() => {
                println!("   The connection dropped ({e}); connecting again…");
                *link = Link::open(&address)
                    .with_context(|| format!("could not reach {}", fan.name))?;
                Ok(link.exchange(json.as_deref())?)
            }
            Err(e) => Err(anyhow!("{e}")),
        }
    };
    let save = |text: &str| {
        if let Some(state) = kagaz_core::fans::FanState::decode(text) {
            let _ = update(|f| {
                f.note_state(&fan.id, &state, "bluetooth");
                Ok(())
            });
        }
    };
    match steps {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let lines: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            println!("{} steps. Watch the fan after each.", lines.len());
            for (i, line) in lines.iter().enumerate() {
                println!("\n>> {}/{}: Kagaz sends: {line}", i + 1, lines.len());
                let go = ask("   Enter to send, 'skip' to pass, 'stop' to end: ")
                    .unwrap_or_else(|| "stop".into());
                match go.trim() {
                    "stop" => break,
                    "skip" => {
                        note(line, "skipped", "");
                        continue;
                    }
                    _ => {}
                }
                match run(&mut link, line) {
                    Ok(answer) => {
                        print!("{}", show_answer(&answer));
                        save(&answer);
                        let said = ask("   What did the fan do? (Enter = exactly that): ")
                            .unwrap_or_default();
                        note(
                            line,
                            &answer,
                            if said.trim().is_empty() {
                                "as asked"
                            } else {
                                said.trim()
                            },
                        );
                    }
                    Err(e) => {
                        println!("   {e}");
                        note(line, &format!("error: {e}"), "");
                    }
                }
            }
        }
        None => {
            println!("Type a command (on, off, speed 3, light on, brightness 50, light-mode warm,");
            println!(
                "sleep on, timer 1, boost on, read, send {{\"json\":1}}), '# what the fan did'"
            );
            println!("to note it, or 'quit'.");
            while let Some(line) = ask("> ") {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line == "quit" || line == "exit" {
                    break;
                }
                if let Some(said) = line.strip_prefix('#') {
                    note("note", "", said.trim());
                    continue;
                }
                match run(&mut link, line) {
                    Ok(answer) => {
                        print!("{}", show_answer(&answer));
                        save(&answer);
                        note(line, &answer, "");
                    }
                    Err(e) => {
                        println!("   {e}");
                        note(line, &format!("error: {e}"), "");
                    }
                }
            }
        }
    }
    drop(link);
    println!("Disconnected.");
    Ok(())
}

#[cfg(target_os = "android")]
fn console(
    _: &Fans,
    _: &str,
    _: Option<std::path::PathBuf>,
    _: Option<std::path::PathBuf>,
) -> Result<()> {
    bail!("the console needs this computer's Bluetooth")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_lines_become_one_key_commands() {
        assert_eq!(console_json("read").unwrap(), None);
        assert_eq!(
            console_json("speed 3").unwrap().as_deref(),
            Some(r#"{"speed":3}"#)
        );
        assert_eq!(
            console_json("light on").unwrap().as_deref(),
            Some(r#"{"led":true}"#)
        );
        assert_eq!(
            console_json("light-mode warm").unwrap().as_deref(),
            Some(r#"{"light_mode":"warm"}"#)
        );
        assert_eq!(
            console_json("timer 6").unwrap().as_deref(),
            Some(r#"{"timer":4}"#)
        );
        assert_eq!(
            console_json("send {\"x\":1}").unwrap().as_deref(),
            Some(r#"{"x":1}"#)
        );
        assert!(console_json("speed 9").is_err());
        assert!(console_json("forget").is_err());
        assert!(console_json("dance").is_err());
    }
}
