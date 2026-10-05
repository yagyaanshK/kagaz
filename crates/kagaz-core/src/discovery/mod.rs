//! Finding devices. Each method returns partial `Device`s; `discover` runs the
//! methods and merges the results into one entry per physical device.

pub mod mdns;
pub mod mdns_txt;
pub mod snmp;
pub mod usb;
pub mod wsd;

use crate::device::{merge_devices, Device};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    /// How long to listen for answers.
    pub timeout: Duration,
    pub use_mdns: bool,
    pub use_wsd: bool,
    pub use_usb: bool,
    pub use_snmp: bool,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(3),
            use_mdns: true,
            use_wsd: true,
            use_usb: true,
            use_snmp: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
    #[error("mDNS: {0}")]
    Mdns(String),
    #[error("WS-Discovery: {0}")]
    Wsd(#[from] std::io::Error),
    #[error("USB: {0}")]
    Usb(String),
    #[error("SNMP: {0}")]
    Snmp(String),
}

/// Run every enabled discovery method and return one `Device` per physical device.
pub fn discover(opts: &DiscoverOptions) -> Result<Vec<Device>, DiscoverError> {
    let mut found = Vec::new();
    let mut errors = Vec::new();

    // The network methods wait for the full timeout, so run everything side by side.
    std::thread::scope(|s| {
        let mdns_handle = opts
            .use_mdns
            .then(|| s.spawn(|| mdns::browse(opts.timeout)));
        let wsd_handle = opts.use_wsd.then(|| s.spawn(|| wsd::probe(opts.timeout)));
        let usb_handle = opts.use_usb.then(|| s.spawn(usb::scan));
        let snmp_handle = opts.use_snmp.then(|| s.spawn(|| snmp::probe(opts.timeout)));
        if let Some(h) = mdns_handle {
            match h.join().expect("mdns thread panicked") {
                Ok(v) => found.extend(v),
                Err(e) => errors.push(DiscoverError::Mdns(e)),
            }
        }
        if let Some(h) = wsd_handle {
            match h.join().expect("wsd thread panicked") {
                Ok(v) => found.extend(v),
                Err(e) => errors.push(DiscoverError::Wsd(e)),
            }
        }
        if let Some(h) = usb_handle {
            match h.join().expect("usb thread panicked") {
                Ok(v) => found.extend(v),
                Err(e) => errors.push(DiscoverError::Usb(e)),
            }
        }
        if let Some(h) = snmp_handle {
            match h.join().expect("snmp thread panicked") {
                Ok(v) => found.extend(v),
                Err(e) => errors.push(DiscoverError::Snmp(e.to_string())),
            }
        }
    });

    if found.is_empty() {
        if let Some(e) = errors.into_iter().next() {
            return Err(e);
        }
    }
    let mut devices = merge_devices(found);
    if opts.use_snmp {
        // Devices that did not answer the broadcast get one direct question;
        // this returns at once when everyone already answered.
        let _ = snmp::enrich(&mut devices, Duration::from_secs(1));
    }
    // A stable order, so "device 2" means the same thing on the next run.
    devices.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.addresses.cmp(&b.addresses))
            .then_with(|| usb_key(a).cmp(&usb_key(b)))
    });
    Ok(devices)
}

fn usb_key(d: &Device) -> Option<(u16, u16, u8, u8)> {
    d.usb
        .as_ref()
        .map(|u| (u.vendor_id, u.product_id, u.bus, u.address))
}
