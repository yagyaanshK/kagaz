//! Atomberg fans over Bluetooth LE, from this computer's own adapter: find
//! the fans in range by their advertised name, and talk to one (write a
//! command, read the state). Not on Android: the phone's Bluetooth is
//! Kotlin's side.
//!
//! Connections are precious. On 2026-10-21 a fan switched on at the wall
//! took about a dozen short connections (one per reading) and then refused
//! more until it was switched off and on again; another had stopped
//! offering its services the same way. An untouched fan was fine. So a
//! [`Link`] keeps one connection for many exchanges: the window keeps one
//! per fan while it is being used ([`kept_exchange`]) and the command
//! line's console keeps one for the session.

use super::atomberg::{self, parse_ble_name};
use super::FanError;
use btleplug::api::{
    Central, CentralState, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
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

fn runtime() -> Result<tokio::runtime::Runtime, FanError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| FanError::Bluetooth(e.to_string()))
}

/// A runtime whose worker keeps running between calls, so a kept
/// connection's background work (BlueZ's signals) goes on.
fn kept_runtime() -> Result<tokio::runtime::Runtime, FanError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| FanError::Bluetooth(e.to_string()))
}

async fn adapter() -> Result<Adapter, FanError> {
    let manager = Manager::new().await.map_err(bt)?;
    let adapters = manager.adapters().await.map_err(bt)?;
    let adapter = adapters.into_iter().next().ok_or(FanError::NoBluetooth)?;
    if let Ok(CentralState::PoweredOff) = adapter.adapter_state().await {
        return Err(FanError::BluetoothOff);
    }
    Ok(adapter)
}

fn bt(e: btleplug::Error) -> FanError {
    match e {
        btleplug::Error::PermissionDenied => FanError::Bluetooth("permission denied".into()),
        btleplug::Error::DeviceNotFound => FanError::OutOfReach,
        btleplug::Error::NotConnected => FanError::OutOfReach,
        other => FanError::Bluetooth(other.to_string()),
    }
}

async fn sighting(p: &Peripheral) -> Option<Sighting> {
    let props = p.properties().await.ok()??;
    let name = props.local_name?;
    let (id, series) = parse_ble_name(&name)?;
    Some(Sighting {
        id,
        series,
        address: p.address().to_string(),
        name,
        rssi: props.rssi,
    })
}

/// Fans advertising within reach, after listening for `seconds`.
pub fn scan(seconds: f32) -> Result<Vec<Sighting>, FanError> {
    runtime()?.block_on(async {
        let adapter = adapter().await?;
        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(bt)?;
        tokio::time::sleep(Duration::from_secs_f32(seconds.clamp(1.0, 60.0))).await;
        let _ = adapter.stop_scan().await;
        let mut out = Vec::new();
        for p in adapter.peripherals().await.map_err(bt)? {
            if let Some(s) = sighting(&p).await {
                if !out.iter().any(|o: &Sighting| o.id == s.id) {
                    out.push(s);
                }
            }
        }
        Ok(out)
    })
}

/// The peripheral at `address`: from the adapter's list (unless `fresh`),
/// else from a scan of up to `wait`.
async fn find(
    adapter: &Adapter,
    address: &str,
    wait: Duration,
    fresh: bool,
) -> Result<Peripheral, FanError> {
    let want = address.to_ascii_uppercase();
    let matches = |p: &Peripheral| p.address().to_string().to_ascii_uppercase() == want;
    if !fresh {
        if let Some(p) = adapter
            .peripherals()
            .await
            .map_err(bt)?
            .into_iter()
            .find(matches)
        {
            return Ok(p);
        }
    }
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(bt)?;
    let start = Instant::now();
    let found = loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        if let Some(p) = adapter
            .peripherals()
            .await
            .map_err(bt)?
            .into_iter()
            .find(matches)
        {
            break Some(p);
        }
        if start.elapsed() > wait {
            break None;
        }
    };
    let _ = adapter.stop_scan().await;
    found.ok_or(FanError::OutOfReach)
}

/// When each fan was last let go, so the next connection waits a little.
static LAST: std::sync::Mutex<Vec<(String, Instant)>> = std::sync::Mutex::new(Vec::new());
/// The least time between two connections to the same fan.
const GAP: Duration = Duration::from_secs(2);

/// One connection to a fan, kept for as many exchanges as needed and
/// closed when dropped.
pub struct Link {
    rt: tokio::runtime::Runtime,
    peripheral: Peripheral,
    command: Option<Characteristic>,
    state: Characteristic,
    address: String,
}

impl Link {
    /// Connect to the fan at `address`, gently: two tries at most, three
    /// seconds apart, every failed try cancelled with a disconnect so no
    /// half-open link is left, and two seconds since this process last let
    /// the fan go.
    pub fn open(address: &str) -> Result<Link, FanError> {
        let key = address.to_ascii_uppercase();
        let wait = LAST
            .lock()
            .ok()
            .and_then(|l| l.iter().find(|(a, _)| *a == key).map(|(_, t)| *t))
            .map(|t| GAP.saturating_sub(t.elapsed()))
            .unwrap_or_default();
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        let rt = kept_runtime()?;
        let opened = rt.block_on(async {
            let adapter = adapter().await?;
            let mut last = FanError::OutOfReach;
            for attempt in 0..2 {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                // A fan advertises slowly and BlueZ gives up on a connection
                // it cannot start from a recent advert ("abort-by-local"),
                // so the second try looks for a fresh advert first.
                let p = match find(&adapter, address, Duration::from_secs(6), attempt > 0).await {
                    Ok(p) => p,
                    Err(e) => {
                        last = e;
                        continue;
                    }
                };
                match tokio::time::timeout(Duration::from_secs(12), p.connect()).await {
                    Ok(Ok(())) => match characteristics(&p).await {
                        Ok((command, state)) => return Ok((p, command, state)),
                        Err(e) => {
                            let _ = p.disconnect().await;
                            return Err(e);
                        }
                    },
                    // Seen advertising, but it would not connect.
                    Ok(Err(e)) => last = bt(e),
                    Err(_) => last = FanError::Refuses,
                }
                // Cancel whatever BlueZ still has under way with the fan.
                let _ = p.disconnect().await;
            }
            Err(last)
        });
        let (peripheral, command, state) = opened.inspect_err(|_| let_go(&key))?;
        Ok(Link {
            rt,
            peripheral,
            command,
            state,
            address: key,
        })
    }

    /// Send `command` (when given) and read the state string back.
    pub fn exchange(&self, command: Option<&str>) -> Result<String, FanError> {
        self.rt.block_on(async {
            if let Some(json) = command {
                let c = self.command.as_ref().ok_or_else(|| {
                    FanError::Bluetooth("this fan takes no Atomberg commands".into())
                })?;
                self.peripheral
                    .write(c, json.as_bytes(), WriteType::WithResponse)
                    .await
                    .map_err(bt)?;
                // The fan applies the command and refreshes its state string.
                tokio::time::sleep(Duration::from_millis(450)).await;
            }
            let mut text = String::new();
            for attempt in 0..3 {
                let raw = self.peripheral.read(&self.state).await.map_err(bt)?;
                text = String::from_utf8_lossy(&raw).trim().to_string();
                if atomberg::FanState::decode(&text).is_some() {
                    return Ok(text);
                }
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            }
            Err(FanError::Reply(text))
        })
    }

    /// Is the connection still up (the fan may have dropped it)?
    pub fn is_connected(&self) -> bool {
        self.rt
            .block_on(self.peripheral.is_connected())
            .unwrap_or(false)
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), self.peripheral.disconnect()).await
        });
        let_go(&self.address);
    }
}

fn let_go(key: &str) {
    if let Ok(mut l) = LAST.lock() {
        l.retain(|(a, _)| a != key);
        l.push((key.to_string(), Instant::now()));
    }
}

/// One exchange on a connection of its own (the command line): connect,
/// send `command` (when given), read the state string, let go.
pub fn exchange(address: &str, command: Option<&str>) -> Result<String, FanError> {
    Link::open(address)?.exchange(command)
}

/// Kept connections, one per fan, with when each was last used.
static KEPT: std::sync::Mutex<Vec<(Link, Instant)>> = std::sync::Mutex::new(Vec::new());
static KEEP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Keep connections between exchanges (the window turns this on, and closes
/// idle ones with [`close_idle`] and all of them on exit with [`close_all`]).
pub fn keep_links(on: bool) {
    KEEP.store(on, std::sync::atomic::Ordering::Release);
}

pub fn keeping() -> bool {
    KEEP.load(std::sync::atomic::Ordering::Acquire)
}

/// An exchange on the fan's kept connection, opening it when there is none
/// or the fan dropped it.
pub fn kept_exchange(address: &str, command: Option<&str>) -> Result<String, FanError> {
    let key = address.to_ascii_uppercase();
    let mut kept = KEPT
        .lock()
        .map_err(|e| FanError::Bluetooth(e.to_string()))?;
    if let Some(i) = kept.iter().position(|(l, _)| l.address() == key) {
        if kept[i].0.is_connected() {
            match kept[i].0.exchange(command) {
                Ok(text) => {
                    kept[i].1 = Instant::now();
                    return Ok(text);
                }
                Err(_) => {
                    kept.remove(i);
                }
            }
        } else {
            kept.remove(i);
        }
    }
    let link = Link::open(address)?;
    let text = link.exchange(command)?;
    kept.push((link, Instant::now()));
    Ok(text)
}

/// Close the kept connections not used for `idle`.
pub fn close_idle(idle: Duration) {
    if let Ok(mut kept) = KEPT.lock() {
        kept.retain(|(_, used)| used.elapsed() < idle);
    }
}

/// Close every kept connection (the window is going away).
pub fn close_all() {
    if let Ok(mut kept) = KEPT.lock() {
        kept.clear();
    }
}

/// The fan's command and state characteristics, or why it offers none.
async fn characteristics(
    p: &Peripheral,
) -> Result<(Option<Characteristic>, Characteristic), FanError> {
    p.discover_services().await.map_err(bt)?;
    let chars = p.characteristics();
    // A fan that lists nothing at all has stopped taking connections.
    if !chars
        .iter()
        .any(|c| c.service_uuid == atomberg::BLE_SERVICE)
        && chars.len() <= 1
    {
        return Err(FanError::Busy);
    }
    let command = chars
        .iter()
        .find(|c| c.uuid == atomberg::BLE_COMMAND)
        .cloned();
    let state = chars
        .iter()
        .find(|c| c.uuid == atomberg::BLE_STATE)
        .cloned()
        .ok_or_else(|| FanError::Bluetooth("this device has no Atomberg state service".into()))?;
    Ok((command, state))
}
