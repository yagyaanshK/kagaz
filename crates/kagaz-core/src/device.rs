//! The device model: one `Device` per physical printer/scanner, merged from
//! every way it was found, with the services it advertises.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;

/// A protocol a device offers. Driverless ones work on every OS without a driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Protocol {
    /// Internet Printing Protocol (driverless printing).
    Ipp,
    /// IPP over TLS.
    Ipps,
    /// eSCL / AirScan (driverless scanning over HTTP).
    Escl,
    /// eSCL over TLS.
    Escls,
    /// WSD print service (Web Services for Devices).
    WsdPrint,
    /// WSD scan service.
    WsdScan,
    /// Raw port-9100 printing (needs a driver that produces the device's language).
    PdlDataStream,
    /// LPD printing.
    Lpd,
    /// SANE `_scanner._tcp` advertisement (vendor-specific scanning).
    SaneNet,
    /// Something else, kept for the report.
    Other,
}

impl Protocol {
    pub fn is_driverless_print(self) -> bool {
        matches!(self, Protocol::Ipp | Protocol::Ipps | Protocol::WsdPrint)
    }
    pub fn is_driverless_scan(self) -> bool {
        matches!(self, Protocol::Escl | Protocol::Escls | Protocol::WsdScan)
    }
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Ipp => "IPP",
            Protocol::Ipps => "IPPS",
            Protocol::Escl => "eSCL",
            Protocol::Escls => "eSCL/TLS",
            Protocol::WsdPrint => "WSD-print",
            Protocol::WsdScan => "WSD-scan",
            Protocol::PdlDataStream => "raw-9100",
            Protocol::Lpd => "LPD",
            Protocol::SaneNet => "SANE-net",
            Protocol::Other => "other",
        }
    }
}

/// One advertised service endpoint on a device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub protocol: Protocol,
    /// Where it came from: "mdns" or "wsd".
    pub source: String,
    pub port: Option<u16>,
    /// Resource path or full URL, when known.
    pub endpoint: Option<String>,
    /// Raw attributes (TXT records for mDNS, metadata fields for WSD).
    pub attributes: BTreeMap<String, String>,
}

/// A physical printer or scanner.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Device {
    /// Human-readable name as advertised.
    pub name: String,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub hostname: Option<String>,
    pub addresses: Vec<IpAddr>,
    /// Stable identifier when the device gives one (mDNS UUID / WSD endpoint).
    pub uuid: Option<String>,
    pub services: Vec<Service>,
}

impl Device {
    pub fn can_print_driverless(&self) -> bool {
        self.services
            .iter()
            .any(|s| s.protocol.is_driverless_print())
    }
    pub fn can_scan_driverless(&self) -> bool {
        self.services
            .iter()
            .any(|s| s.protocol.is_driverless_scan())
    }
    pub fn has_protocol(&self, p: Protocol) -> bool {
        self.services.iter().any(|s| s.protocol == p)
    }
    pub fn protocols(&self) -> Vec<Protocol> {
        let mut v: Vec<Protocol> = self.services.iter().map(|s| s.protocol).collect();
        v.sort();
        v.dedup();
        v
    }

    /// True when `other` is most likely the same physical device.
    pub fn same_device(&self, other: &Device) -> bool {
        if let (Some(a), Some(b)) = (&self.uuid, &other.uuid) {
            if a.eq_ignore_ascii_case(b) {
                return true;
            }
        }
        if let (Some(a), Some(b)) = (&self.hostname, &other.hostname) {
            if a.eq_ignore_ascii_case(b) {
                return true;
            }
        }
        self.addresses.iter().any(|a| other.addresses.contains(a))
    }

    /// Fold `other` into `self`, keeping the best-known fields.
    pub fn merge(&mut self, other: Device) {
        if self.name.is_empty() {
            self.name = other.name;
        }
        if self.manufacturer.is_none() {
            self.manufacturer = other.manufacturer;
        }
        if self.model.is_none() {
            self.model = other.model;
        }
        if self.hostname.is_none() {
            self.hostname = other.hostname;
        }
        if self.uuid.is_none() {
            self.uuid = other.uuid;
        }
        for a in other.addresses {
            if !self.addresses.contains(&a) {
                self.addresses.push(a);
            }
        }
        self.services.extend(other.services);
    }
}

/// Merge a list of partial sightings into one entry per physical device.
pub fn merge_devices(found: Vec<Device>) -> Vec<Device> {
    let mut out: Vec<Device> = Vec::new();
    for d in found {
        if let Some(existing) = out.iter_mut().find(|e| e.same_device(&d)) {
            existing.merge(d);
        } else {
            out.push(d);
        }
    }
    for d in &mut out {
        d.addresses.sort();
        d.services.sort_by_key(|s| s.protocol);
    }
    out
}
