//! Atomberg's own protocol, as its app speaks it to the fan: the same JSON
//! commands over Bluetooth LE (a GATT service the fan advertises from the
//! day it is unboxed, no Wi-Fi needed) and over Wi-Fi (UDP datagrams on the
//! local network once the fan has been put on it). The fan answers both with
//! one comma-separated "state string" whose first field is a bit field of
//! the live state.
//!
//! Nothing here is published by Atomberg; it is what the community worked
//! out (github.com/allusv/atomberg-local and its forks) and what was checked
//! against the fans here. Firmware may change it.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// The GATT service every Atomberg fan advertises.
pub const BLE_SERVICE: Uuid = Uuid::from_u128(0x9256cc8a_85b3_46a6_a4a4_9b6a2e1248be);
/// Write a JSON command here (with response).
pub const BLE_COMMAND: Uuid = Uuid::from_u128(0xe29ee02c_af3d_11ec_b909_0242ac120002);
/// Read the full state string here.
pub const BLE_STATE: Uuid = Uuid::from_u128(0x9fa1610e_e6fc_11ec_8fea_0242ac120002);

/// The fan listens for JSON command datagrams here (Wi-Fi).
pub const UDP_COMMAND_PORT: u16 = 5600;
/// The fan broadcasts its presence and state here (Wi-Fi).
pub const UDP_BROADCAST_PORT: u16 = 5625;

pub const MIN_SPEED: u8 = 1;
pub const MAX_SPEED: u8 = 6;
/// Timer choices, by index: off, 1 h, 2 h, 3 h, 6 h.
pub const TIMER_HOURS: [u8; 5] = [0, 1, 2, 3, 6];

/// What the fan's series code says about it: its product name and what it
/// can do. The code comes from the Bluetooth name (`Atomberg_R1_<id>`) and
/// from field 7 of the state string. A series is a product family, not one
/// exact model, so the name is the family's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    pub name: &'static str,
    pub has_light: bool,
    pub has_brightness: bool,
    pub has_color: bool,
}

pub fn profile(series: &str) -> Profile {
    let s = series.trim().to_ascii_uppercase();
    let name = match s.as_str() {
        "R1" => "Renesa",
        "R2" => "Renesa+",
        "R3" => "Renesa Smart+",
        "S1" => "Studio+",
        "S2" => "Renesa Elite",
        "I1" => "Aris",
        "I2" => "Aris Starlight",
        "I3" => "Aris Star",
        "I4" => "Aris",
        "I5" => "Aris Contours",
        "M1" => "Efficio",
        "M2" => "Efficio+",
        "K1" => "Gorilla",
        _ => match s.chars().next() {
            Some('R') => "Renesa",
            Some('S') => "Studio",
            Some('I') => "Aris",
            Some('M') => "Efficio",
            Some('K') => "Gorilla",
            _ => "Atomberg fan",
        },
    };
    let aris = s.starts_with('I');
    Profile {
        name,
        has_light: !s.starts_with('K'),
        has_brightness: aris || matches!(s.as_str(), "M1" | "S1" | "S2"),
        has_color: aris,
    }
}

/// The fan's live state, decoded from its state string.
///
/// Checked on a real Aris (I1) on 2026-10-21, state set with its remote and
/// read back: field 0 bit 0x10 is on, the low three bits are the speed
/// (1 to 5, and 6 in Boost), bit 0x40 is Boost and stays set after the fan
/// is turned off from Boost (a Renesa off after Boost reads 0x46). Field 11
/// counts seconds running and field 13 seconds switched off, each moving
/// only in its own state. The light, checked the same way: 0x20 is on;
/// colour 0x8000 alone is warm, 0x08 alone white ("cool"), both natural
/// white ("daylight"); bits 8 to 14 are the brightness, 10 to 100. Sleep
/// (0x80) and the timer's hours (bits 16 to 19) come back as set by a
/// command (checked). The Aris's remote has neither button; the
/// Efficio's remote has both, so both are the fans' own features.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FanState {
    pub power: bool,
    /// 1 to 5, or 6 in Boost (0 when the fan has never run).
    pub speed: u8,
    /// The Boost flag as the fan reports it; it stays set after the fan is
    /// turned off from Boost, so it means Boost only while `power` is on.
    #[serde(default)]
    pub boost: bool,
    pub led: bool,
    pub sleep: bool,
    /// 10 to 100 (percent), meaningful on fans whose light dims.
    pub brightness: u8,
    /// Hours left on the timer, 0 when none is set.
    pub timer_hours: u8,
    /// "warm", "cool" (white) or "daylight" (natural white), meaningful on
    /// fans with colour modes.
    pub light_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    /// Seconds the fan has run, in all, to date (field 11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_seconds: Option<u64>,
    /// Seconds the fan has been switched off, in all, to date (field 13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_seconds: Option<u64>,
    /// The raw bit field, kept for the report.
    #[serde(default)]
    pub raw: i64,
}

impl FanState {
    /// Decode a state string (`"23,...,R1,..."`). None for the fan's other
    /// replies, such as "No command detected in the message".
    pub fn decode(text: &str) -> Option<FanState> {
        let parts: Vec<&str> = text.trim().split(',').collect();
        let v: i64 = parts.first()?.trim().parse().ok()?;
        let cool = v & 0x08 != 0;
        let warm = v & 0x8000 != 0;
        let light_mode = match (cool, warm) {
            (true, true) => "daylight",
            (true, false) => "cool",
            _ => "warm",
        };
        let count = |i: usize| parts.get(i).and_then(|s| s.trim().parse::<u64>().ok());
        Some(FanState {
            power: v & 0x10 != 0,
            speed: (v & 0x07) as u8,
            boost: v & 0x40 != 0,
            led: v & 0x20 != 0,
            sleep: v & 0x80 != 0,
            brightness: ((v & 0x7F00) >> 8) as u8,
            timer_hours: ((v & 0x0F_0000) >> 16) as u8,
            light_mode: light_mode.to_string(),
            series: parts
                .get(7)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            run_seconds: count(11),
            off_seconds: count(13),
            raw: v,
        })
    }

    /// Running in Boost now (the flag alone stays set after turning off).
    pub fn in_boost(&self) -> bool {
        self.power && self.boost
    }

    /// The state in a few words: "Off", "Speed 3", "Boost", "Speed 2, light on, 1 h left".
    pub fn words(&self) -> String {
        if !self.power {
            return "Off".to_string();
        }
        let mut words = vec![if self.in_boost() {
            "Boost".to_string()
        } else {
            format!("Speed {}", self.speed)
        }];
        if self.sleep {
            words.push("sleep".into());
        }
        if self.led {
            words.push("light on".into());
        }
        if self.timer_hours > 0 {
            words.push(format!("{} h left", self.timer_hours));
        }
        words.join(", ")
    }
}

/// A span of seconds the way people say it: "288 days, 2 h, 4 min and 16 s".
pub fn span_words(seconds: u64) -> String {
    let (d, h, m, s) = (
        seconds / 86_400,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    );
    let mut parts = Vec::new();
    if d > 0 {
        parts.push(format!("{d} day{}", if d == 1 { "" } else { "s" }));
    }
    if h > 0 {
        parts.push(format!("{h} h"));
    }
    if m > 0 {
        parts.push(format!("{m} min"));
    }
    if s > 0 || parts.is_empty() {
        parts.push(format!("{s} s"));
    }
    match parts.len() {
        1 => parts.remove(0),
        n => format!("{} and {}", parts[..n - 1].join(", "), parts[n - 1]),
    }
}

/// One thing to tell the fan. The fan acts on the first key of a command
/// only, so each command carries exactly one.
///
/// Checked on an Aris on 2026-10-21, over one connection, watching the fan:
/// power, speed 1 to 5, speed 6 (Boost), boost on and off (off returns to
/// speed 5), led, light_mode warm/cool/daylight, brightness any whole
/// value 10 to 100 (stored exactly; steps of 2 or more are visible; the
/// remote's own steps are coarser: 10, 25, ...). A timer index outside
/// 0 to 4 (5 was tried) cancels the timer, as 0 does.
/// The fan takes sleep and timer and reports them back, and an Efficio
/// confirms each with an animation of its indicator light; what Sleep does
/// through the night, and the timer switching the fan off, are not yet
/// seen (the state shows whole hours left, with no countdown).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Command {
    Power(bool),
    /// 1 to 6; the fan turns on when it was off.
    Speed(u8),
    Sleep(bool),
    Led(bool),
    /// 10 to 100, as the fan reports it.
    Brightness(u8),
    /// The timer by index into [`TIMER_HOURS`]: 0 off, 1 = 1 h, 2 = 2 h, 3 = 3 h, 4 = 6 h.
    Timer(u8),
    Boost(bool),
    /// "cool", "warm" or "daylight".
    LightMode(String),
    /// Ask for the state without changing anything.
    Read,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CommandError {
    #[error("speed must be {MIN_SPEED} to {MAX_SPEED}")]
    Speed,
    #[error("timer must be 0 (off) to 4 (6 h)")]
    Timer,
    #[error("brightness must be 10 to 100")]
    Brightness,
    #[error("light mode must be cool, warm or daylight")]
    LightMode,
}

impl Command {
    pub fn validate(&self) -> Result<(), CommandError> {
        match self {
            Command::Speed(n) if !(MIN_SPEED..=MAX_SPEED).contains(n) => Err(CommandError::Speed),
            Command::Timer(i) if *i as usize >= TIMER_HOURS.len() => Err(CommandError::Timer),
            // The Aris reports its light from 10 (lowest) to 100 (highest).
            Command::Brightness(b) if !(10..=100).contains(b) => Err(CommandError::Brightness),
            Command::LightMode(m) if !matches!(m.as_str(), "cool" | "warm" | "daylight") => {
                Err(CommandError::LightMode)
            }
            _ => Ok(()),
        }
    }

    /// The JSON the fan takes, one key.
    pub fn to_json(&self) -> String {
        match self {
            Command::Power(on) => format!(r#"{{"power":{on}}}"#),
            Command::Speed(n) => format!(r#"{{"speed":{n}}}"#),
            Command::Sleep(on) => format!(r#"{{"sleep":{on}}}"#),
            Command::Led(on) => format!(r#"{{"led":{on}}}"#),
            Command::Brightness(b) => format!(r#"{{"brightness":{b}}}"#),
            Command::Timer(i) => format!(r#"{{"timer":{i}}}"#),
            Command::Boost(on) => format!(r#"{{"boost":{on}}}"#),
            Command::LightMode(m) => format!(r#"{{"light_mode":"{m}"}}"#),
            Command::Read => r#"{"read":1}"#.to_string(),
        }
    }

    /// Does this command change the fan (as opposed to asking)?
    pub fn changes(&self) -> bool {
        !matches!(self, Command::Read)
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::Power(true) => write!(f, "turn on"),
            Command::Power(false) => write!(f, "turn off"),
            Command::Speed(n) => write!(f, "speed {n}"),
            Command::Sleep(on) => write!(f, "sleep {}", if *on { "on" } else { "off" }),
            Command::Led(on) => write!(f, "light {}", if *on { "on" } else { "off" }),
            Command::Brightness(b) => write!(f, "brightness {b}"),
            Command::Timer(0) => write!(f, "timer off"),
            Command::Timer(i) => write!(
                f,
                "timer {} h",
                TIMER_HOURS.get(*i as usize).copied().unwrap_or(0)
            ),
            Command::Boost(on) => write!(f, "boost {}", if *on { "on" } else { "off" }),
            Command::LightMode(m) => write!(f, "light {m}"),
            Command::Read => write!(f, "read"),
        }
    }
}

/// A fan's identity from its Bluetooth name: `Atomberg_R1_a1b2c3d4e5f6`
/// (any case, sometimes with a trailing `_3`) gives the device id, which is
/// the fan's Wi-Fi hardware address in lowercase hex, and its series.
pub fn parse_ble_name(name: &str) -> Option<(String, String)> {
    let low = name.to_ascii_lowercase();
    if !low.starts_with("atomberg_") {
        return None;
    }
    let mut parts = name.split('_');
    let _prefix = parts.next()?;
    let series = parts.next()?.to_ascii_uppercase();
    let id = parts.next()?.to_ascii_lowercase();
    if id.len() < 8 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((id, series))
}

/// What a Wi-Fi broadcast (port 5625) or a command reply (port 5600) said.
#[derive(Debug, Clone, PartialEq)]
pub struct Datagram {
    pub id: String,
    pub series: Option<String>,
    pub state: Option<FanState>,
}

/// Decode a datagram: JSON (`{"device_id":..,"state_string":..}`), the same
/// JSON hex-encoded, a bare state string, or a presence beacon
/// (`<id>_<series>`).
pub fn parse_datagram(data: &[u8]) -> Option<Datagram> {
    let mut msg = String::from_utf8_lossy(data).trim().to_string();
    // Some firmware prefixes a proxy-protocol line.
    if let Some(rest) = msg.strip_prefix("PROXY ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() >= 6 {
            msg = parts[5..].join(" ");
        }
    }
    let json_object = |text: &str| -> Option<serde_json::Value> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        v.is_object().then_some(v)
    };
    let mut payload = json_object(&msg);
    if payload.is_none() {
        if let Some(bytes) = hex_decode(&msg) {
            payload = json_object(&String::from_utf8_lossy(&bytes));
        }
    }
    if let Some(p) = payload {
        let id = p.get("device_id")?.as_str()?.to_ascii_lowercase();
        let state = p
            .get("state_string")
            .and_then(|s| s.as_str())
            .and_then(FanState::decode);
        let series = state
            .as_ref()
            .and_then(|s| s.series.clone())
            .or_else(|| p.get("series").and_then(|s| s.as_str()).map(str::to_string));
        return Some(Datagram { id, series, state });
    }
    // A bare state string (a command's reply) carries the series in field 7
    // but no id; the caller knows whom it asked.
    if msg.contains(',') {
        if let Some(state) = FanState::decode(&msg) {
            return Some(Datagram {
                id: String::new(),
                series: state.series.clone(),
                state: Some(state),
            });
        }
    }
    // Presence beacon: "<id>_<series>".
    let (id, rest) = msg.split_once('_')?;
    let id = id.trim().to_ascii_lowercase();
    if (8..=16).contains(&id.len()) && id.chars().all(|c| c.is_ascii_hexdigit()) {
        let series = rest
            .split('_')
            .next()
            .map(|s| s.trim().to_ascii_uppercase());
        return Some(Datagram {
            id,
            series: series.filter(|s| !s.is_empty()),
            state: None,
        });
    }
    None
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let t = text.trim();
    if !t.len().is_multiple_of(2) || t.is_empty() || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..t.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&t[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_string_decodes_bit_by_bit() {
        // power on, speed 3, led on, brightness 64, 2 h timer, cool light.
        let v = 0x10 | 3 | 0x20 | (64 << 8) | (2 << 16) | 0x08;
        let text = format!("{v},230,0,0,231.5,0,0,R1,0,0,0,3600,0,0,0,0.12,41");
        let s = FanState::decode(&text).unwrap();
        assert!(s.power && s.led && !s.sleep);
        assert_eq!(s.speed, 3);
        assert_eq!(s.brightness, 64);
        assert_eq!(s.timer_hours, 2);
        assert_eq!(s.light_mode, "cool");
        assert_eq!(s.series.as_deref(), Some("R1"));
        assert_eq!(s.words(), "Speed 3, light on, 2 h left");
        assert_eq!(FanState::decode("3,0").unwrap().words(), "Off");
        assert!(FanState::decode("No command detected in the message").is_none());
    }

    /// State strings read from real fans on 2026-10-21, with what the fan
    /// was doing (set with its remote).
    const ARIS_OFF: &str = "35338,1,Z,0,108.69,106,153,I1,1498,1,3140,24890656,634730394,13916395,0,0.62,25.29,0,385714287,0,0,0,300,0,282,END";
    const ARIS_SPEED_1: &str = "35353,1,Z,0,86.85,76,120,I1,1498,1,3140,24890669,634730460,13916413,0,0.45,25.29,0,385714287,0,0,0,300,0,282,END";
    const ARIS_SPEED_1_LATER: &str = "35353,1,Z,0,86.46,70,120,I1,1498,1,3140,24890680,634730493,13916413,0,0.41,25.29,0,385714287,0,0,0,300,0,282,END";
    const ARIS_SPEED_3: &str = "35355,1,Z,0,131.03,136,180,I1,1498,1,3140,24890731,634730808,13916413,0,0.80,25.18,0,385714287,0,0,0,300,0,282,END";
    const ARIS_BOOST: &str = "35422,1,Z,0,222.69,309,252,I1,1498,1,3140,24890754,634731222,13916413,0,1.81,25.18,0,385714287,0,0,0,300,0,282,END";
    const RENESA_OFF_AFTER_BOOST: &str = "70,1,Z,0,210.06,273,312,R1,1816,1,2481,20195877,257194846,7587414,0,1.60,25.06,0,158823531,0,0,0,340,0,307,END";

    #[test]
    fn real_fans_read_as_they_were() {
        let off = FanState::decode(ARIS_OFF).unwrap();
        assert!(!off.power);
        assert_eq!(off.words(), "Off");
        let one = FanState::decode(ARIS_SPEED_1).unwrap();
        assert!(one.power && !one.in_boost());
        assert_eq!(one.speed, 1);
        assert_eq!(FanState::decode(ARIS_SPEED_3).unwrap().words(), "Speed 3");
        let boost = FanState::decode(ARIS_BOOST).unwrap();
        assert!(boost.in_boost());
        assert_eq!(boost.speed, 6);
        assert_eq!(boost.words(), "Boost");
        let renesa = FanState::decode(RENESA_OFF_AFTER_BOOST).unwrap();
        assert!(renesa.boost && !renesa.power && !renesa.in_boost());
        assert_eq!(renesa.words(), "Off");
        assert_eq!(renesa.series.as_deref(), Some("R1"));
    }

    /// The Aris at speed 2 with its light, cycled with the remote (field 0
    /// as read; two counters this test does not use set to 0).
    const LIGHT_NATURAL_10: &str = "35386,1,Z,0,108.72,103,153,I1,1499,1,3142,24893329,0,13916671,0,0.60,25.29,0,0,0,0,0,300,0,278,END";
    const LIGHT_WARM: &str = "35378,1,Z,0,108.71,100,153,I1,1499,1,3142,24893357,0,13916671,0,0.59,25.18,0,0,0,0,0,300,0,278,END";
    const LIGHT_WHITE: &str = "2618,1,Z,0,108.97,101,152,I1,1499,1,3142,24893372,0,13916671,0,0.59,25.18,0,0,0,0,0,300,0,278,END";
    const LIGHT_NATURAL_100: &str = "58426,1,Z,0,108.77,100,152,I1,1499,1,3142,24893429,0,13916671,0,0.59,25.18,0,0,0,0,0,300,0,278,END";
    const LIGHT_OFF_FAN_ON: &str = "58394,1,Z,0,108.67,97,152,I1,1499,1,3142,24893442,0,13916671,0,0.57,25.18,0,0,0,0,0,300,0,278,END";

    /// An Efficio (M1), one white light, read on 2026-10-21: its string has
    /// one more field than the Aris's (field 25, changing on every read,
    /// not decoded). Here at speed 5, light on at 100, a 1-hour timer set.
    const EFFICIO_TIMER_1H: &str = "123957,1,Z,0,186,234,245,M1,1909,1,101,22031950,578698512,10844128,0,1.37,25.18,0,385714287,0,0,0,306,0,282,2331,END";

    #[test]
    fn an_efficio_reads_as_it_was() {
        let s = FanState::decode(EFFICIO_TIMER_1H).unwrap();
        assert!(s.power && s.led && !s.in_boost());
        assert_eq!((s.speed, s.brightness, s.timer_hours), (5, 100, 1));
        assert_eq!(s.series.as_deref(), Some("M1"));
        assert_eq!(s.words(), "Speed 5, light on, 1 h left");
        assert!(!profile("M1").has_color && profile("M1").has_brightness);
    }

    #[test]
    fn the_light_reads_as_it_was() {
        let n = FanState::decode(LIGHT_NATURAL_10).unwrap();
        assert!(n.led && n.power);
        assert_eq!(
            (n.light_mode.as_str(), n.brightness, n.speed),
            ("daylight", 10, 2)
        );
        assert_eq!(FanState::decode(LIGHT_WARM).unwrap().light_mode, "warm");
        assert_eq!(FanState::decode(LIGHT_WHITE).unwrap().light_mode, "cool");
        let bright = FanState::decode(LIGHT_NATURAL_100).unwrap();
        assert_eq!(
            (bright.light_mode.as_str(), bright.brightness),
            ("daylight", 100)
        );
        let off = FanState::decode(LIGHT_OFF_FAN_ON).unwrap();
        assert!(!off.led && off.power);
        assert_eq!(
            off.brightness, 100,
            "the light keeps its brightness while off"
        );
    }

    #[test]
    fn running_time_moves_only_while_running() {
        let a = FanState::decode(ARIS_SPEED_1).unwrap();
        let b = FanState::decode(ARIS_SPEED_1_LATER).unwrap();
        assert_eq!(b.run_seconds.unwrap() - a.run_seconds.unwrap(), 11);
        assert_eq!(b.off_seconds, a.off_seconds);
        let off = FanState::decode(ARIS_OFF).unwrap();
        assert_eq!(off.run_seconds, Some(24_890_656));
        assert_eq!(
            span_words(off.run_seconds.unwrap()),
            "288 days, 2 h, 4 min and 16 s"
        );
        assert_eq!(span_words(3600 * 5 + 14 * 60 + 3), "5 h, 14 min and 3 s");
        assert_eq!(span_words(60), "1 min");
        assert_eq!(span_words(0), "0 s");
    }

    #[test]
    fn commands_carry_one_key_and_are_checked() {
        assert_eq!(Command::Speed(4).to_json(), r#"{"speed":4}"#);
        assert_eq!(Command::Power(false).to_json(), r#"{"power":false}"#);
        assert_eq!(
            Command::LightMode("warm".into()).to_json(),
            r#"{"light_mode":"warm"}"#
        );
        assert_eq!(Command::Read.to_json(), r#"{"read":1}"#);
        assert_eq!(Command::Speed(7).validate(), Err(CommandError::Speed));
        assert_eq!(Command::Timer(5).validate(), Err(CommandError::Timer));
        assert_eq!(Command::Timer(4).to_string(), "timer 6 h");
        assert!(Command::Speed(1).validate().is_ok());
    }

    #[test]
    fn bluetooth_names_give_id_and_series() {
        assert_eq!(
            parse_ble_name("Atomberg_R1_A1B2C3D4E5F6"),
            Some(("a1b2c3d4e5f6".into(), "R1".into()))
        );
        assert_eq!(
            parse_ble_name("atomberg_s2_a1b2c3d4e5f6_3"),
            Some(("a1b2c3d4e5f6".into(), "S2".into()))
        );
        assert_eq!(parse_ble_name("[LG] webOS TV"), None);
        assert_eq!(parse_ble_name("Atomberg_R1_zz"), None);
    }

    #[test]
    fn datagrams_in_every_shape() {
        let j = r#"{"device_id":"A1B2C3D4E5F6","state_string":"19,0,0,0,0,0,0,M1"}"#;
        let d = parse_datagram(j.as_bytes()).unwrap();
        assert_eq!(d.id, "a1b2c3d4e5f6");
        assert_eq!(d.series.as_deref(), Some("M1"));
        assert_eq!(d.state.as_ref().unwrap().speed, 3);
        let hex: String = j.bytes().map(|b| format!("{b:02x}")).collect();
        assert_eq!(parse_datagram(hex.as_bytes()).unwrap(), d);
        let beacon = parse_datagram(b"a1b2c3d4e5f6_R1").unwrap();
        assert_eq!(beacon.series.as_deref(), Some("R1"));
        assert!(beacon.state.is_none());
        let reply = parse_datagram(b"3,0,0,0,0,0,0,R1").unwrap();
        assert_eq!(reply.id, "");
        assert!(!reply.state.unwrap().power);
        assert!(parse_datagram(b"hello").is_none());
    }

    #[test]
    fn series_tell_what_a_fan_has() {
        assert_eq!(profile("R1").name, "Renesa");
        assert!(!profile("R1").has_brightness);
        assert!(profile("I1").has_color && profile("I1").has_brightness);
        assert!(!profile("K1").has_light);
        assert_eq!(profile("I9").name, "Aris");
        assert_eq!(profile("").name, "Atomberg fan");
    }
}
