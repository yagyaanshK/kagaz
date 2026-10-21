//! Ceiling fans, starting with Atomberg: find the fans within reach (over
//! Bluetooth from this computer, and over Wi-Fi when a fan is on the
//! network), remember the ones the person adds by room and name, and turn
//! them on and off, set the speed, the light, sleep and the timer, exactly
//! as the Atomberg app and remote do.

pub mod atomberg;
#[cfg(not(target_os = "android"))]
pub mod ble;
pub mod store;
pub mod udp;

pub use atomberg::{Command, FanState};
pub use store::{Fan, Fans};

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum FanError {
    #[error("this computer has no Bluetooth adapter")]
    NoBluetooth,
    #[error("Bluetooth is turned off")]
    BluetoothOff,
    #[error("Bluetooth: {0}")]
    Bluetooth(String),
    #[error("the fan is out of reach")]
    OutOfReach,
    #[error("the fan is not on this network and not within Bluetooth reach")]
    NoWay,
    #[error("the fan answered with something other than its state: {0}")]
    Reply(String),
    #[error("{0}")]
    Command(#[from] atomberg::CommandError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Store(#[from] store::StoreError),
}

/// A fan a scan saw, whether remembered or not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub id: String,
    pub series: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ble_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rssi: Option<i16>,
    /// Its state, when the scan learnt it (Wi-Fi broadcasts carry it;
    /// Bluetooth adverts do not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<FanState>,
}

impl Found {
    /// What to call a fan nobody has named: its product name and the end of
    /// its id ("Renesa 4a24").
    pub fn suggested_name(&self) -> String {
        let tail = &self.id[self.id.len().saturating_sub(4)..];
        format!("{} {}", atomberg::profile(&self.series).name, tail)
    }
}

/// How a fan was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reach {
    Bluetooth,
    Wifi,
}

impl Reach {
    pub fn label(self) -> &'static str {
        match self {
            Reach::Bluetooth => "bluetooth",
            Reach::Wifi => "wifi",
        }
    }
}

/// Look for fans for about `seconds`: Bluetooth adverts and Wi-Fi
/// broadcasts at the same time, merged by id. Bluetooth trouble (no
/// adapter, turned off) is returned apart, so a Wi-Fi-only computer still
/// gets its list.
pub fn scan(seconds: f32) -> (Vec<Found>, Option<FanError>) {
    let wifi = std::thread::spawn(move || udp::listen(seconds));
    #[cfg(not(target_os = "android"))]
    let ble = ble::scan(seconds);
    #[cfg(target_os = "android")]
    let ble: Result<Vec<ble_stub::Sighting>, FanError> = Ok(Vec::new());
    let mut found: Vec<Found> = Vec::new();
    let mut trouble = None;
    match ble {
        Ok(sightings) => {
            for s in sightings {
                found.push(Found {
                    id: s.id,
                    series: s.series,
                    ble_address: Some(s.address),
                    ip: None,
                    rssi: s.rssi,
                    state: None,
                });
            }
        }
        Err(e) => trouble = Some(e),
    }
    if let Ok(Ok(heard)) = wifi.join() {
        for h in heard {
            let series = h.datagram.series.clone().unwrap_or_default();
            match found.iter_mut().find(|f| f.id == h.datagram.id) {
                Some(f) => {
                    f.ip = Some(h.ip.to_string());
                    if f.series.is_empty() {
                        f.series = series;
                    }
                    f.state = h.datagram.state;
                }
                None => found.push(Found {
                    id: h.datagram.id,
                    series,
                    ble_address: None,
                    ip: Some(h.ip.to_string()),
                    rssi: None,
                    state: h.datagram.state,
                }),
            }
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    (found, trouble)
}

#[cfg(target_os = "android")]
mod ble_stub {
    pub struct Sighting {
        pub id: String,
        pub series: String,
        pub address: String,
        pub rssi: Option<i16>,
    }
}

/// Send `command` to a remembered fan (or just ask, with [`Command::Read`])
/// and return its state: over Wi-Fi when the fan has an address there and
/// answers, else over Bluetooth.
pub fn send(fan: &Fan, command: &Command) -> Result<(FanState, Reach), FanError> {
    let (text, reach) = send_raw(fan, command)?;
    let state = FanState::decode(&text).ok_or(FanError::Reply(text))?;
    Ok((state, reach))
}

/// [`send`], returning the fan's state string exactly as it came, for
/// checking the decoding against what the fan is really doing.
pub fn send_raw(fan: &Fan, command: &Command) -> Result<(String, Reach), FanError> {
    command.validate()?;
    let json = command.to_json();
    let mut last: Option<FanError> = None;
    if let Some(ip) = fan.ip.as_deref().and_then(|s| s.parse::<IpAddr>().ok()) {
        // Over Wi-Fi a read is the community's `{"read":1}`: the fan only
        // answers a datagram. Not yet checked on a fan on the network.
        match udp::send(ip, &json, Duration::from_millis(800)) {
            Ok(Some(text)) => return Ok((text, Reach::Wifi)),
            Ok(None) => last = Some(FanError::OutOfReach),
            Err(e) => last = Some(e),
        }
    }
    #[cfg(not(target_os = "android"))]
    if let Some(address) = fan.ble_address.as_deref() {
        // Over Bluetooth a read only reads the state: nothing is written.
        let write = command.changes().then_some(json.as_str());
        let text = ble::exchange(address, write)?;
        return Ok((text, Reach::Bluetooth));
    }
    Err(last.unwrap_or(FanError::NoWay))
}

/// The fan's state now, without changing anything.
pub fn read(fan: &Fan) -> Result<(FanState, Reach), FanError> {
    send(fan, &Command::Read)
}

/// How a remembered fan stands, in a few words, from what is known.
pub fn standing(fan: &Fan, reachable: bool) -> String {
    match (&fan.last_state, reachable) {
        (Some(s), true) => s.words(),
        (Some(s), false) if !s.power => "Off, out of reach".to_string(),
        (Some(_), false) => "Out of reach".to_string(),
        (None, true) => "Within reach".to_string(),
        (None, false) => "Out of reach".to_string(),
    }
}
