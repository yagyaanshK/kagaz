//! mDNS / DNS-SD discovery: the way AirPrint, IPP Everywhere and eSCL devices
//! announce themselves. Pure Rust, no Avahi or Bonjour needed.

use crate::device::{Device, Protocol, Service};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const SERVICE_TYPES: &[(&str, Protocol)] = &[
    ("_ipp._tcp.local.", Protocol::Ipp),
    ("_ipps._tcp.local.", Protocol::Ipps),
    ("_uscan._tcp.local.", Protocol::Escl),
    ("_uscans._tcp.local.", Protocol::Escls),
    ("_pdl-datastream._tcp.local.", Protocol::PdlDataStream),
    ("_printer._tcp.local.", Protocol::Lpd),
    ("_scanner._tcp.local.", Protocol::SaneNet),
];

/// Browse all printer/scanner service types for `timeout` and return one
/// partial `Device` per resolved service.
pub fn browse(timeout: Duration) -> Result<Vec<Device>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
    let mut receivers = Vec::new();
    for (ty, proto) in SERVICE_TYPES {
        let rx = daemon.browse(ty).map_err(|e| e.to_string())?;
        receivers.push((rx, *proto));
    }

    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    // Poll each browser with a short wait until the deadline.
    while Instant::now() < deadline {
        let mut got_any = false;
        for (rx, proto) in &receivers {
            while let Ok(event) = rx.recv_timeout(Duration::from_millis(50)) {
                got_any = true;
                if std::env::var_os("KAGAZ_DEBUG").is_some() {
                    eprintln!("mdns event: {event:?}");
                }
                if let ServiceEvent::ServiceResolved(info) = event {
                    found.push(device_from_info(&info, *proto));
                }
            }
        }
        if !got_any {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    for (ty, _) in SERVICE_TYPES {
        let _ = daemon.stop_browse(ty);
    }
    let _ = daemon.shutdown();
    Ok(found)
}

fn device_from_info(info: &mdns_sd::ResolvedService, proto: Protocol) -> Device {
    let mut attributes = BTreeMap::new();
    for prop in info.txt_properties.iter() {
        attributes.insert(prop.key().to_string(), prop.val_str().to_string());
    }
    if attributes.is_empty() {
        // See mdns_txt: the library will not ask for a late TXT record.
        attributes = super::mdns_txt::fetch_txt(&info.fullname, Duration::from_millis(800));
    }
    // Instance name is "<name>._ipp._tcp.local."; keep the part before the type.
    let full = info.fullname.as_str();
    let name = full
        .strip_suffix(info.ty_domain.as_str())
        .map(|s| s.trim_end_matches('.').to_string())
        .unwrap_or_else(|| full.to_string())
        .replace("\\032", " ");

    let model = attributes
        .get("ty")
        .or_else(|| attributes.get("product"))
        .map(|s| s.trim_matches(|c| c == '(' || c == ')').to_string());
    let manufacturer = attributes.get("usb_MFG").cloned().or_else(|| {
        model
            .as_ref()
            .and_then(|m| m.split_whitespace().next().map(|s| s.to_string()))
    });
    let endpoint = attributes
        .get("rp")
        .or_else(|| attributes.get("rs"))
        .cloned();
    let hostname = Some(info.host.trim_end_matches('.').to_string());

    Device {
        name,
        manufacturer,
        model,
        hostname,
        addresses: info.addresses.iter().map(|a| a.to_ip_addr()).collect(),
        uuid: attributes.get("UUID").cloned(),
        usb: None,
        services: vec![Service {
            protocol: proto,
            source: "mdns".into(),
            port: Some(info.port),
            endpoint,
            attributes,
        }],
    }
}
