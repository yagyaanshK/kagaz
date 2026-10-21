//! Atomberg fans over Bluetooth LE, from this computer's own adapter: find
//! the fans in range by their advertised name, and talk to one (write a
//! command, read the state). Each exchange is its own short connection, as
//! the Atomberg app does it, so the fan stays free for the app and the
//! remote. Not on Android: the phone's Bluetooth is Kotlin's side.

use super::atomberg::{self, parse_ble_name};
use super::FanError;
use btleplug::api::{Central, CentralState, Manager as _, Peripheral as _, ScanFilter, WriteType};
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

/// Connect to the fan at `address`, send `command` (when given) and read
/// the state string back.
///
/// Gentle on purpose. On 2026-10-21 two fans that Kagaz had connected to
/// many times in quick succession (up to four tries per request, timed-out
/// tries never cancelled) went on accepting connections but offered no
/// services, while an untouched fan was fine. So: two tries at most, three
/// seconds apart; every try that fails or times out is cancelled with a
/// disconnect, so no half-open link is left with the fan; and a pause of
/// at least two seconds between connections to the same fan.
pub fn exchange(address: &str, command: Option<&str>) -> Result<String, FanError> {
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
    let result = runtime()?.block_on(async {
        let adapter = adapter().await?;
        let mut last = FanError::OutOfReach;
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            // A fan advertises slowly and BlueZ gives up on a connection it
            // cannot start from a recent advert ("abort-by-local"), so the
            // second try looks for a fresh advert first.
            let p = match find(&adapter, address, Duration::from_secs(6), attempt > 0).await {
                Ok(p) => p,
                Err(e) => {
                    last = e;
                    continue;
                }
            };
            match tokio::time::timeout(Duration::from_secs(12), p.connect()).await {
                Ok(Ok(())) => {
                    let result = talk(&p, command).await;
                    let _ = p.disconnect().await;
                    return result;
                }
                Ok(Err(e)) => last = bt(e),
                Err(_) => last = FanError::OutOfReach,
            }
            // Cancel whatever BlueZ still has under way with the fan.
            let _ = p.disconnect().await;
        }
        Err(last)
    });
    if let Ok(mut l) = LAST.lock() {
        l.retain(|(a, _)| *a != key);
        l.push((key, Instant::now()));
    }
    result
}

async fn talk(p: &Peripheral, command: Option<&str>) -> Result<String, FanError> {
    p.discover_services().await.map_err(bt)?;
    let chars = p.characteristics();
    // A fan that lists nothing at all is answering someone else.
    if !chars
        .iter()
        .any(|c| c.service_uuid == atomberg::BLE_SERVICE)
        && chars.len() <= 1
    {
        return Err(FanError::Busy);
    }
    let cmd_char = chars.iter().find(|c| c.uuid == atomberg::BLE_COMMAND);
    let state_char = chars
        .iter()
        .find(|c| c.uuid == atomberg::BLE_STATE)
        .ok_or_else(|| FanError::Bluetooth("this device has no Atomberg state service".into()))?;
    if let Some(json) = command {
        let c = cmd_char
            .ok_or_else(|| FanError::Bluetooth("this device takes no Atomberg commands".into()))?;
        p.write(c, json.as_bytes(), WriteType::WithResponse)
            .await
            .map_err(bt)?;
        // The fan applies the command and refreshes its state string.
        tokio::time::sleep(Duration::from_millis(450)).await;
    }
    let mut text = String::new();
    for attempt in 0..3 {
        let raw = p.read(state_char).await.map_err(bt)?;
        text = String::from_utf8_lossy(&raw).trim().to_string();
        if atomberg::FanState::decode(&text).is_some() {
            return Ok(text);
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }
    Err(FanError::Reply(text))
}
