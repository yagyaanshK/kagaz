//! Picking one device out of a discovery result by whatever the user typed:
//! its number in the list, an IP address, a hostname, a USB `vendor:product`
//! id, or a fragment of its name or model.

use crate::device::Device;
use std::net::IpAddr;

#[derive(Debug, thiserror::Error)]
pub enum SelectError {
    #[error("no device matches \"{0}\"")]
    NoMatch(String),
    /// More than one device matched; the 1-based numbers of the candidates.
    #[error("\"{selector}\" matches {} devices", candidates.len())]
    Ambiguous {
        selector: String,
        candidates: Vec<usize>,
    },
}

/// Find the device `selector` names. Numbers are 1-based positions in `devices`.
pub fn find<'a>(devices: &'a [Device], selector: &str) -> Result<&'a Device, SelectError> {
    let selector = selector.trim();
    if let Ok(n) = selector.parse::<usize>() {
        return match n.checked_sub(1).and_then(|i| devices.get(i)) {
            Some(d) => Ok(d),
            None => Err(SelectError::NoMatch(selector.to_string())),
        };
    }
    let matching: Vec<usize> = devices
        .iter()
        .enumerate()
        .filter(|(_, d)| matches(d, selector))
        .map(|(i, _)| i + 1)
        .collect();
    match matching.as_slice() {
        [] => Err(SelectError::NoMatch(selector.to_string())),
        [one] => Ok(&devices[one - 1]),
        _ => Err(SelectError::Ambiguous {
            selector: selector.to_string(),
            candidates: matching,
        }),
    }
}

fn matches(d: &Device, selector: &str) -> bool {
    if let Ok(ip) = selector.parse::<IpAddr>() {
        return d.addresses.contains(&ip);
    }
    if let Some((vid, pid)) = parse_usb_id(selector) {
        return d
            .usb
            .as_ref()
            .is_some_and(|u| u.vendor_id == vid && u.product_id == pid);
    }
    let needle = selector.to_ascii_lowercase();
    let hay = |s: &str| s.to_ascii_lowercase().contains(&needle);
    hay(&d.name)
        || d.model.as_deref().is_some_and(hay)
        || d.manufacturer.as_deref().is_some_and(hay)
        || d.hostname.as_deref().is_some_and(hay)
        || d.usb
            .as_ref()
            .and_then(|u| u.serial.as_deref())
            .is_some_and(|s| s.eq_ignore_ascii_case(selector))
        || d.attribute("serial")
            .is_some_and(|s| s.eq_ignore_ascii_case(selector))
}

fn parse_usb_id(s: &str) -> Option<(u16, u16)> {
    let (a, b) = s.split_once(':')?;
    if a.len() != 4 || b.len() != 4 {
        return None;
    }
    Some((
        u16::from_str_radix(a, 16).ok()?,
        u16::from_str_radix(b, 16).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Protocol, Service, UsbInfo};
    use std::collections::BTreeMap;

    fn fleet() -> Vec<Device> {
        let mut snmp_attrs = BTreeMap::new();
        snmp_attrs.insert("serial".to_string(), "KAGAZ0000000001".to_string());
        vec![
            Device {
                name: "Brother DCP-L2540DW series".into(),
                manufacturer: Some("Brother".into()),
                model: Some("DCP-L2540DW series".into()),
                hostname: Some("BRWEXAMPLE00001.local".into()),
                addresses: vec!["198.51.100.167".parse().unwrap()],
                services: vec![Service {
                    protocol: Protocol::Snmp,
                    source: "snmp".into(),
                    port: Some(161),
                    endpoint: None,
                    attributes: snmp_attrs,
                }],
                ..Device::default()
            },
            Device {
                name: "HP LaserJet 1020".into(),
                manufacturer: Some("HP".into()),
                model: Some("LaserJet 1020".into()),
                usb: Some(UsbInfo {
                    bus: 1,
                    address: 4,
                    vendor_id: 0x03f0,
                    product_id: 0x2b17,
                    serial: Some("CNB1234".into()),
                }),
                ..Device::default()
            },
            Device {
                name: "Brother HL-L2350DW series".into(),
                manufacturer: Some("Brother".into()),
                addresses: vec!["198.51.100.80".parse().unwrap()],
                ..Device::default()
            },
        ]
    }

    #[test]
    fn selects_by_number_address_host_usb_id_and_fragment() {
        let f = fleet();
        assert_eq!(find(&f, "2").unwrap().name, "HP LaserJet 1020");
        assert_eq!(
            find(&f, "198.51.100.167").unwrap().name,
            "Brother DCP-L2540DW series"
        );
        assert_eq!(
            find(&f, "brwexample00001").unwrap().name,
            "Brother DCP-L2540DW series"
        );
        assert_eq!(find(&f, "03F0:2B17").unwrap().name, "HP LaserJet 1020");
        assert_eq!(find(&f, "laserjet").unwrap().name, "HP LaserJet 1020");
        assert_eq!(
            find(&f, "l2540").unwrap().name,
            "Brother DCP-L2540DW series"
        );
        assert_eq!(find(&f, "CNB1234").unwrap().name, "HP LaserJet 1020");
        assert_eq!(
            find(&f, "kagaz0000000001").unwrap().name,
            "Brother DCP-L2540DW series"
        );
    }

    #[test]
    fn reports_no_match_and_ambiguity() {
        let f = fleet();
        assert!(matches!(find(&f, "0"), Err(SelectError::NoMatch(_))));
        assert!(matches!(find(&f, "4"), Err(SelectError::NoMatch(_))));
        assert!(matches!(find(&f, "canon"), Err(SelectError::NoMatch(_))));
        assert!(matches!(find(&f, "10.0.0.1"), Err(SelectError::NoMatch(_))));
        match find(&f, "brother") {
            Err(SelectError::Ambiguous { candidates, .. }) => assert_eq!(candidates, vec![1, 3]),
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }
}
