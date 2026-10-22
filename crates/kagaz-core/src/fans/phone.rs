//! Atomberg fans over the phone's Bluetooth LE. Android's Bluetooth is only
//! reachable from Kotlin (`KagazBle.kt` in the Android project), so the app
//! installs a [`Radio`] that calls it, and this module gives `fans` and the
//! window the same calls as the computer's `ble.rs` (it is compiled as
//! `fans::ble` on Android).
//!
//! The same care with connections as on the computer: a fan takes only so
//! many new ones before it stops answering until switched off at the wall
//! (seen 2026-10-21), so Kotlin keeps one per fan and this side closes it
//! once it has been idle a while.

use super::atomberg::{self, parse_ble_name};
use super::FanError;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A fan seen advertising.
#[derive(Debug, Clone)]
pub struct Sighting {
    pub id: String,
    pub series: String,
    pub address: String,
    pub name: String,
    pub rssi: Option<i16>,
}

/// A named device heard advertising.
#[derive(Debug, Clone)]
pub struct Heard {
    pub address: String,
    pub name: String,
    pub rssi: Option<i16>,
}

/// The phone's Bluetooth, as the app reaches it.
pub trait Radio: Send + Sync {
    /// Named devices heard advertising over `seconds`.
    fn scan(&self, seconds: f32) -> Result<Vec<Heard>, FanError>;
    /// On the fan's kept connection (opened when there is none): write
    /// `command` (when given) to the command characteristic, then read the
    /// state characteristic.
    fn exchange(&self, address: &str, command: Option<&str>) -> Result<String, FanError>;
    /// Let go of the fan's connection, if any.
    fn close(&self, address: &str);
}

static RADIO: OnceLock<Box<dyn Radio>> = OnceLock::new();

/// The app's way to the phone's Bluetooth; the first one installed stays.
pub fn install(radio: Box<dyn Radio>) {
    let _ = RADIO.set(radio);
}

fn radio() -> Result<&'static dyn Radio, FanError> {
    RADIO
        .get()
        .map(|r| r.as_ref())
        .ok_or_else(|| FanError::Bluetooth("the phone's Bluetooth is not reachable yet".into()))
}

/// Fans advertising within reach, after listening for `seconds`.
pub fn scan(seconds: f32) -> Result<Vec<Sighting>, FanError> {
    let mut out: Vec<Sighting> = Vec::new();
    for h in radio()?.scan(seconds.clamp(1.0, 60.0))? {
        if let Some((id, series)) = parse_ble_name(&h.name) {
            if !out.iter().any(|o| o.id == id) {
                out.push(Sighting {
                    id,
                    series,
                    address: h.address,
                    name: h.name,
                    rssi: h.rssi,
                });
            }
        }
    }
    Ok(out)
}

/// Each kept connection's fan, with when it was last used.
static USED: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
static KEEP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Keep connections between exchanges (the window turns this on, and closes
/// idle ones with [`close_idle`] and all of them on exit with [`close_all`]).
pub fn keep_links(on: bool) {
    KEEP.store(on, std::sync::atomic::Ordering::Release);
}

pub fn keeping() -> bool {
    KEEP.load(std::sync::atomic::Ordering::Acquire)
}

/// One exchange, letting go of the fan afterwards.
pub fn exchange(address: &str, command: Option<&str>) -> Result<String, FanError> {
    let out = kept_exchange(address, command);
    let key = address.to_ascii_uppercase();
    if let Ok(mut used) = USED.lock() {
        used.retain(|(a, _)| *a != key);
    }
    if let Ok(r) = radio() {
        r.close(address);
    }
    out
}

/// An exchange on the fan's kept connection, opening it when there is none
/// or the fan dropped it. A state string that does not decode (the fan
/// still writing it) is read again, twice at most.
pub fn kept_exchange(address: &str, command: Option<&str>) -> Result<String, FanError> {
    let radio = radio()?;
    let mut text = radio.exchange(address, command)?;
    for _ in 0..2 {
        if atomberg::FanState::decode(&text).is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
        text = radio.exchange(address, None)?;
    }
    let key = address.to_ascii_uppercase();
    if let Ok(mut used) = USED.lock() {
        used.retain(|(a, _)| *a != key);
        used.push((key, Instant::now()));
    }
    if atomberg::FanState::decode(&text).is_none() {
        return Err(FanError::Reply(text));
    }
    Ok(text)
}

/// Close the kept connections not used for `idle`.
pub fn close_idle(idle: Duration) {
    let Ok(radio) = radio() else { return };
    if let Ok(mut used) = USED.lock() {
        used.retain(|(a, t)| {
            let keep = t.elapsed() < idle;
            if !keep {
                radio.close(a);
            }
            keep
        });
    }
}

/// Close every kept connection (the window is going away).
pub fn close_all() {
    let Ok(radio) = radio() else { return };
    if let Ok(mut used) = USED.lock() {
        for (a, _) in used.drain(..) {
            radio.close(&a);
        }
    }
}
