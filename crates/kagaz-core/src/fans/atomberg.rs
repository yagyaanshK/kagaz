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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FanState {
    pub power: bool,
    /// 1 to 6 (0 when the fan has never run).
    pub speed: u8,
    pub led: bool,
    pub sleep: bool,
    /// 0 to 127, meaningful on fans whose light dims.
    pub brightness: u8,
    /// Hours left on the timer, 0 when none is set.
    pub timer_hours: u8,
    /// "cool", "warm" or "daylight" (meaningful on fans with colour modes).
    pub light_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
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
        Some(FanState {
            power: v & 0x10 != 0,
            speed: (v & 0x07) as u8,
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
            raw: v,
        })
    }

    /// The state in a few words: "Off", "Speed 3", "Speed 2, light on, 1 h left".
    pub fn words(&self) -> String {
        if !self.power {
            return "Off".to_string();
        }
        let mut words = vec![format!("Speed {}", self.speed)];
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

/// One thing to tell the fan. The fan acts on the first key of a command
/// only, so each command carries exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Command {
    Power(bool),
    /// 1 to 6; the fan turns on when it was off.
    Speed(u8),
    Sleep(bool),
    Led(bool),
    /// 0 to 127.
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
    #[error("brightness must be 0 to 127")]
    Brightness,
    #[error("light mode must be cool, warm or daylight")]
    LightMode,
}

impl Command {
    pub fn validate(&self) -> Result<(), CommandError> {
        match self {
            Command::Speed(n) if !(MIN_SPEED..=MAX_SPEED).contains(n) => Err(CommandError::Speed),
            Command::Timer(i) if *i as usize >= TIMER_HOURS.len() => Err(CommandError::Timer),
            Command::Brightness(b) if *b > 127 => Err(CommandError::Brightness),
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
