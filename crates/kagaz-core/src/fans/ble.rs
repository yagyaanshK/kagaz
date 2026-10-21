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

/// Connect to the fan at `address`, send `command` (when given) and read
/// the state string back.
pub fn exchange(address: &str, command: Option<&str>) -> Result<String, FanError> {
    runtime()?.block_on(async {
        let adapter = adapter().await?;
        // A fan advertises slowly, and BlueZ gives up on a connection it
        // cannot start from a recent advert ("abort-by-local"), so a failed
        // try is followed by a fresh scan and another go.
        let mut last = FanError::OutOfReach;
        for attempt in 0..4 {
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
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
        Err(last)
    })
}

async fn talk(p: &Peripheral, command: Option<&str>) -> Result<String, FanError> {
    p.discover_services().await.map_err(bt)?;
    let chars = p.characteristics();
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
