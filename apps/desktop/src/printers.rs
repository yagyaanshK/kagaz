//! The printers the person added (Add on a found printer). They are kept
//! with how to reach them, so the Printers page lists them at once and says
//! whether each is ready, printing, needs attention or is offline, without
//! searching the network first. A later search refreshes their addresses
//! (a printer's address can change when the router hands out new ones).

use super::blocking;
use kagaz_core::ipp::status::{PrinterState, PrinterStatus};
use kagaz_core::{Device, Protocol};
use serde::{Deserialize, Serialize};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    printers: Vec<Device>,
}

fn path() -> PathBuf {
    kagaz_core::paths::config_dir().join("printers.json")
}

fn load() -> Vec<Device> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str::<Saved>(&t).ok())
        .map(|s| s.printers)
        .unwrap_or_default()
}

fn save(printers: &[Device]) -> Result<(), String> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&Saved {
        printers: printers.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::write(p, text).map_err(|e| format!("could not save the printer list: {e}"))
}

/// What stays the same about a printer when its address changes: the UUID
/// it announces, else its USB identity, else its host name, else its name.
pub fn key(d: &Device) -> String {
    if let Some(u) = d.uuid.as_deref().filter(|u| !u.is_empty()) {
        return format!("uuid:{}", u.trim_start_matches("urn:uuid:").to_lowercase());
    }
    if let Some(usb) = &d.usb {
        return match &usb.serial {
            Some(s) => format!("usb:{:04x}:{:04x}:{s}", usb.vendor_id, usb.product_id),
            None => format!("usb:{:04x}:{:04x}", usb.vendor_id, usb.product_id),
        };
    }
    if let Some(h) = d.hostname.as_deref().filter(|h| !h.is_empty()) {
        return format!("host:{}", h.trim_end_matches('.').to_lowercase());
    }
    format!("name:{}", d.name.to_lowercase())
}

/// Put `device` in the list, in place of the one with the same key.
fn upsert(list: &mut Vec<Device>, device: Device) {
    let k = key(&device);
    match list.iter_mut().find(|d| key(d) == k) {
        Some(d) => *d = device,
        None => list.push(device),
    }
}

/// The saved printers, each replaced by its fresh copy from `found` when the
/// search saw it. True when anything changed.
fn refresh(list: &mut [Device], found: &[Device]) -> bool {
    let mut changed = false;
    for d in list.iter_mut() {
        let k = key(d);
        if let Some(f) = found.iter().find(|f| key(f) == k) {
            let before = serde_json::to_string(&*d).ok();
            *d = f.clone();
            changed |= before != serde_json::to_string(&*d).ok();
        }
    }
    changed
}

/// A saved printer's state, in the words the page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct State {
    /// "ready", "printing", "attention", "online" (it answers, but says
    /// nothing about itself), "offline" or "usb" (on a cable; not checked).
    pub state: &'static str,
    /// What the printer says is wrong, in plain words, when it says.
    pub reason: String,
}

/// The printer's own report, reduced to one state and its main reason.
fn from_status(s: &PrinterStatus) -> State {
    let worst = |sev: &str| s.reasons.iter().find(|r| r.severity == sev && !r.text.is_empty());
    let error = worst("error");
    let state = match s.state {
        _ if error.is_some() => "attention",
        PrinterState::Stopped => "attention",
        PrinterState::Processing => "printing",
        PrinterState::Idle => "ready",
        PrinterState::Unknown => "online",
    };
    let reason = error
        .or_else(|| worst("warning"))
        .or_else(|| s.reasons.iter().find(|r| !r.text.is_empty()))
        .map(|r| r.text.clone())
        .unwrap_or_default();
    State { state, reason }
}

/// Does anything answer at the printer's addresses, on the ports it offers?
fn answers(d: &Device) -> bool {
    let mut ports: Vec<u16> = d.services.iter().filter_map(|s| s.port).collect();
    ports.extend([631, 80, 9100]);
    ports.sort_unstable();
    ports.dedup();
    d.addresses.iter().any(|ip| {
        ports.iter().any(|&port| {
            TcpStream::connect_timeout(&SocketAddr::new(*ip, port), Duration::from_millis(1200)).is_ok()
        })
    })
}

fn check(d: &Device) -> State {
    let quiet = |state| State {
        state,
        reason: String::new(),
    };
    if d.usb.is_some() && d.addresses.is_empty() {
        return quiet("usb");
    }
    // A quick knock first: an offline printer says so in about a second,
    // not after the full IPP wait.
    if !answers(d) {
        return quiet("offline");
    }
    if d.has_protocol(Protocol::Ipp) {
        if let Ok(s) = kagaz_core::ipp::status::fetch(d) {
            return from_status(&s);
        }
    }
    quiet("online")
}

#[tauri::command]
pub fn printers_saved() -> Vec<Device> {
    load()
}

#[tauri::command]
pub fn printer_add(device: Device) -> Result<Vec<Device>, String> {
    let mut list = load();
    upsert(&mut list, device);
    save(&list)?;
    Ok(list)
}

#[tauri::command]
pub fn printer_forget(device: Device) -> Result<Vec<Device>, String> {
    let k = key(&device);
    let mut list = load();
    list.retain(|d| key(d) != k);
    save(&list)?;
    Ok(list)
}

/// A search finished: the saved printers it saw take their fresh details.
#[tauri::command]
pub fn printers_seen(found: Vec<Device>) -> Result<Vec<Device>, String> {
    let mut list = load();
    if refresh(&mut list, &found) {
        save(&list)?;
    }
    Ok(list)
}

#[tauri::command]
pub async fn printer_state(device: Device) -> Result<State, String> {
    blocking(move || Ok(check(&device))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use kagaz_core::ipp::status::Reason;
    use std::net::{IpAddr, Ipv4Addr};

    fn printer(name: &str, uuid: Option<&str>, ip: u8) -> Device {
        Device {
            name: name.into(),
            uuid: uuid.map(str::to_string),
            addresses: vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, ip))],
            ..Default::default()
        }
    }

    #[test]
    fn the_same_printer_at_a_new_address_is_the_same_printer() {
        let old = printer("Office", Some("urn:uuid:ABC-1"), 10);
        let moved = printer("Office", Some("abc-1"), 42);
        assert_eq!(key(&old), key(&moved));

        let mut list = vec![old];
        assert!(refresh(&mut list, &[moved.clone(), printer("Other", None, 7)]));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].addresses, moved.addresses);
        assert!(!refresh(&mut list, &[moved]), "nothing new the second time");
    }

    #[test]
    fn adding_twice_keeps_one() {
        let mut list = Vec::new();
        upsert(&mut list, printer("Office", Some("u1"), 10));
        upsert(&mut list, printer("Office (renamed)", Some("u1"), 11));
        upsert(&mut list, printer("Hall", None, 12));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Office (renamed)");
    }

    fn status(state: PrinterState, reasons: &[(&str, &str)]) -> PrinterStatus {
        PrinterStatus {
            name: String::new(),
            info: None,
            make_and_model: None,
            location: None,
            state,
            reasons: reasons
                .iter()
                .map(|(sev, text)| Reason {
                    keyword: String::new(),
                    severity: sev.to_string(),
                    text: text.to_string(),
                })
                .collect(),
            message: None,
            accepting_jobs: true,
            queued_jobs: 0,
            supplies: vec![],
            media_ready: vec![],
            media_default: None,
            color: false,
            duplex: false,
            pages_per_minute: None,
            max_dpi: None,
            document_formats: vec![],
            up_time: None,
            uuid: None,
            more_info: None,
            jobs: vec![],
        }
    }

    #[test]
    fn a_printers_report_becomes_one_state() {
        assert_eq!(from_status(&status(PrinterState::Idle, &[])).state, "ready");
        assert_eq!(from_status(&status(PrinterState::Processing, &[])).state, "printing");
        let empty = from_status(&status(PrinterState::Idle, &[("warning", "toner low"), ("error", "out of paper")]));
        assert_eq!(empty, State { state: "attention", reason: "out of paper".into() });
        let low = from_status(&status(PrinterState::Idle, &[("warning", "toner low")]));
        assert_eq!(low, State { state: "ready", reason: "toner low".into() });
        assert_eq!(from_status(&status(PrinterState::Stopped, &[])).state, "attention");
    }

    #[test]
    fn nothing_listening_is_offline() {
        // 192.0.2.0/24 is for documentation: nothing answers there.
        assert!(!answers(&printer("Gone", None, 99)));
    }
}
