//! Printers and scanners on a USB cable.
//!
//! Every USB printer exposes an interface of class 7 (printer), subclass 1.
//! The interface protocol says how much the host can do without a driver:
//!
//! | protocol | meaning |
//! |---|---|
//! | 1 | unidirectional (print only, no status) |
//! | 2 | bidirectional |
//! | 3 | IEEE 1284.4 |
//! | 4 | IPP over USB: the device speaks HTTP/IPP (and usually eSCL) over the cable, driverless |
//!
//! The device also carries an IEEE 1284 Device ID (`MFG:Brother;MDL:...;CMD:...`),
//! the same string that AirPrint devices mirror in their `usb_MFG`/`usb_MDL`
//! TXT records and the key the driver database is indexed by. On Linux the
//! kernel's `usblp` driver publishes it in sysfs, readable by anyone; elsewhere,
//! and when `usblp` is not bound, it is fetched with the class-specific
//! GET_DEVICE_ID request, which needs permission to open the device.

use crate::device::{Device, Protocol, Service, UsbInfo};
use nusb::MaybeFuture;
use std::collections::BTreeMap;

const CLASS_PRINTER: u8 = 7;
const SUBCLASS_PRINTER: u8 = 1;
const PROTOCOL_IPP_USB: u8 = 4;

/// What a device's printer-class interfaces say it can do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrinterInterfaces {
    /// At least one 7/1/4 interface: driverless IPP (and usually eSCL) over USB.
    pub ipp_usb: bool,
    /// At least one 7/1/1, 7/1/2 or 7/1/3 interface: the classic printer port, needs a driver.
    pub legacy: bool,
    /// Interface number to send GET_DEVICE_ID to, when there is one.
    pub first_interface: Option<u8>,
}

impl PrinterInterfaces {
    pub fn is_printer(&self) -> bool {
        self.ipp_usb || self.legacy
    }
}

/// Classify a device from its `(interface number, class, subclass, protocol)` tuples.
pub fn classify(interfaces: impl IntoIterator<Item = (u8, u8, u8, u8)>) -> PrinterInterfaces {
    let mut out = PrinterInterfaces::default();
    for (number, class, subclass, protocol) in interfaces {
        if class != CLASS_PRINTER || subclass != SUBCLASS_PRINTER {
            continue;
        }
        if protocol == PROTOCOL_IPP_USB {
            out.ipp_usb = true;
        } else {
            out.legacy = true;
        }
        if out.first_interface.is_none() {
            out.first_interface = Some(number);
        }
    }
    out
}

/// Parse an IEEE 1284 Device ID (`KEY:value;KEY:value;...`) into upper-cased
/// keys, folding the long spellings onto the short ones (`MANUFACTURER` ->
/// `MFG`, `MODEL` -> `MDL`, `COMMAND SET` -> `CMD`, `CLASS` -> `CLS`).
pub fn parse_device_id(id: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for field in id.split(';') {
        let Some((key, value)) = field.split_once(':') else {
            continue;
        };
        let key = match key.trim().to_ascii_uppercase().as_str() {
            "MANUFACTURER" => "MFG".to_string(),
            "MODEL" => "MDL".to_string(),
            "COMMAND SET" => "CMD".to_string(),
            "CLASS" => "CLS".to_string(),
            other => other.to_string(),
        };
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        out.entry(key).or_insert_with(|| value.to_string());
    }
    out
}

/// Decode the payload of a GET_DEVICE_ID reply: a big-endian length that
/// counts itself, then the ID string. Devices get the length wrong often
/// enough that the shorter of "what it said" and "what arrived" wins.
pub fn decode_device_id_reply(raw: &[u8]) -> Option<String> {
    if raw.len() < 2 {
        return None;
    }
    let declared = u16::from_be_bytes([raw[0], raw[1]]) as usize;
    let end = if declared >= 2 {
        declared.min(raw.len())
    } else {
        raw.len()
    };
    let text = String::from_utf8_lossy(&raw[2..end]);
    let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Enumerate the USB bus and return one `Device` per printer-class device.
pub fn scan() -> Result<Vec<Device>, String> {
    let devices = nusb::list_devices()
        .wait()
        .map_err(|e| format!("cannot list USB devices: {e}"))?;
    let mut found = Vec::new();
    for info in devices {
        let ifaces = classify(
            info.interfaces()
                .map(|i| (i.interface_number(), i.class(), i.subclass(), i.protocol())),
        );
        if !ifaces.is_printer() {
            continue;
        }
        found.push(describe(&info, ifaces));
    }
    Ok(found)
}

fn describe(info: &nusb::DeviceInfo, ifaces: PrinterInterfaces) -> Device {
    let mut attributes = BTreeMap::new();
    attributes.insert("vendor_id".to_string(), format!("{:04x}", info.vendor_id()));
    attributes.insert(
        "product_id".to_string(),
        format!("{:04x}", info.product_id()),
    );
    if let Some(s) = info.manufacturer_string() {
        attributes.insert("usb_manufacturer".to_string(), s.to_string());
    }
    if let Some(s) = info.product_string() {
        attributes.insert("usb_product".to_string(), s.to_string());
    }

    let mut manufacturer = info.manufacturer_string().map(str::to_string);
    let mut model = info.product_string().map(str::to_string);
    match ifaces.first_interface.map(|i| read_device_id(info, i)) {
        Some(Ok(id)) => {
            let fields = parse_device_id(&id);
            if let Some(m) = fields.get("MFG") {
                manufacturer = Some(m.clone());
            }
            if let Some(m) = fields.get("MDL") {
                model = Some(m.clone());
            }
            attributes.insert("device_id".to_string(), id);
            attributes.extend(fields);
        }
        Some(Err(note)) => {
            attributes.insert("identity_note".to_string(), note);
        }
        None => {}
    }

    let name = match (&manufacturer, &model) {
        (Some(mf), Some(md)) if !md.starts_with(mf.as_str()) => format!("{mf} {md}"),
        (_, Some(md)) => md.clone(),
        (Some(mf), None) => mf.clone(),
        (None, None) => format!(
            "USB printer {:04x}:{:04x}",
            info.vendor_id(),
            info.product_id()
        ),
    };

    let endpoint = Some(format!(
        "usb://{:04x}:{:04x}{}",
        info.vendor_id(),
        info.product_id(),
        info.serial_number()
            .map(|s| format!("?serial={s}"))
            .unwrap_or_default()
    ));
    let mut services = Vec::new();
    if ifaces.ipp_usb {
        services.push(Service {
            protocol: Protocol::IppUsb,
            source: "usb".into(),
            port: None,
            endpoint: endpoint.clone(),
            attributes: attributes.clone(),
        });
    }
    if ifaces.legacy {
        services.push(Service {
            protocol: Protocol::UsbPrinter,
            source: "usb".into(),
            port: None,
            endpoint,
            attributes,
        });
    }

    Device {
        name,
        manufacturer,
        model,
        hostname: None,
        addresses: Vec::new(),
        uuid: None,
        usb: Some(UsbInfo {
            bus: info.busnum(),
            address: info.device_address(),
            vendor_id: info.vendor_id(),
            product_id: info.product_id(),
            serial: info.serial_number().map(str::to_string),
        }),
        services,
    }
}

/// The IEEE 1284 Device ID of interface `interface`, or a plain-words note
/// saying why it could not be read.
fn read_device_id(info: &nusb::DeviceInfo, interface: u8) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    if let Some(id) = sysfs_device_id(info.sysfs_path(), interface) {
        return Ok(id);
    }
    control_device_id(info, interface)
}

/// `usblp` publishes the ID as `<device>/<device>:<config>.<interface>/ieee1284_id`.
#[cfg(target_os = "linux")]
fn sysfs_device_id(device_dir: &std::path::Path, interface: u8) -> Option<String> {
    let suffix = format!(".{interface}");
    let entries = std::fs::read_dir(device_dir).ok()?;
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.contains(':') || !name.ends_with(&suffix) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path().join("ieee1284_id")) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn control_device_id(info: &nusb::DeviceInfo, interface: u8) -> Result<String, String> {
    use nusb::transfer::{ControlIn, ControlType, Recipient};
    use std::time::Duration;

    let device = info.open().wait().map_err(|e| match e.kind() {
        nusb::ErrorKind::PermissionDenied => permission_note(),
        nusb::ErrorKind::Busy => {
            "the printer's identity could not be read: another program or driver holds the device"
                .to_string()
        }
        _ => format!("the printer's identity could not be read: {e}"),
    })?;
    // Printer class 1.1 §4.2.1 GET_DEVICE_ID: wValue = configuration index,
    // wIndex = interface in the high byte, alternate setting in the low byte.
    let reply = device
        .control_in(
            ControlIn {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: 0,
                value: 0,
                index: (interface as u16) << 8,
                length: 1024,
            },
            Duration::from_secs(2),
        )
        .wait()
        .map_err(|e| format!("the printer did not answer GET_DEVICE_ID: {e}"))?;
    decode_device_id_reply(&reply).ok_or_else(|| "the printer sent an empty device ID".to_string())
}

#[cfg(target_os = "windows")]
fn control_device_id(_info: &nusb::DeviceInfo, _interface: u8) -> Result<String, String> {
    // Windows binds printers to usbprint.sys and only lets WinUSB clients send
    // control transfers; the spooler's own device ID has to be read via the
    // print API instead. Until then the USB strings identify the device.
    Err("the printer's identity is not read on Windows yet; using the USB product name".to_string())
}

#[cfg(not(target_os = "windows"))]
fn permission_note() -> String {
    if cfg!(target_os = "linux") {
        "the printer's identity could not be read: no permission to open the USB device (add your user to the `lp` group, or run once with sudo)".to_string()
    } else {
        "the printer's identity could not be read: no permission to open the USB device".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // What a Brother DCP-L2540DW reports; reconstructed from the usb_MFG,
    // usb_MDL and usb_CMD fields it mirrors in its AirPrint TXT record, until
    // the cable recording in tests/fixtures/usb replaces it.
    const BROTHER_ID: &str =
        "MFG:Brother;CMD:PJL,PCL,PCLXL,URF;MDL:DCP-L2540DW series;CLS:PRINTER;";

    #[test]
    fn parses_short_keys() {
        let f = parse_device_id(BROTHER_ID);
        assert_eq!(f["MFG"], "Brother");
        assert_eq!(f["MDL"], "DCP-L2540DW series");
        assert_eq!(f["CMD"], "PJL,PCL,PCLXL,URF");
        assert_eq!(f["CLS"], "PRINTER");
    }

    #[test]
    fn folds_long_keys_and_ignores_junk() {
        let f = parse_device_id(
            "MANUFACTURER: HP ;MODEL:LaserJet 1020; COMMAND SET:ACL;CLASS:PRINTER;garbage;:empty;KEY:",
        );
        assert_eq!(f["MFG"], "HP");
        assert_eq!(f["MDL"], "LaserJet 1020");
        assert_eq!(f["CMD"], "ACL");
        assert_eq!(f["CLS"], "PRINTER");
        assert_eq!(f.len(), 4);
    }

    #[test]
    fn decodes_reply_with_correct_length() {
        let mut raw = Vec::new();
        let len = (BROTHER_ID.len() + 2) as u16;
        raw.extend_from_slice(&len.to_be_bytes());
        raw.extend_from_slice(BROTHER_ID.as_bytes());
        raw.extend_from_slice(&[0, 0, 0]); // padding past the declared length
        assert_eq!(decode_device_id_reply(&raw).as_deref(), Some(BROTHER_ID));
    }

    #[test]
    fn decodes_reply_with_wrong_length() {
        let mut raw = vec![0xff, 0xff]; // claims 65535 bytes, sends 20
        raw.extend_from_slice(b"MFG:X;MDL:Y;\0\0");
        assert_eq!(
            decode_device_id_reply(&raw).as_deref(),
            Some("MFG:X;MDL:Y;")
        );
        let mut raw = vec![0x00, 0x00]; // claims nothing
        raw.extend_from_slice(b"MFG:X;");
        assert_eq!(decode_device_id_reply(&raw).as_deref(), Some("MFG:X;"));
        assert_eq!(decode_device_id_reply(&[0, 2]), None);
        assert_eq!(decode_device_id_reply(&[]), None);
    }

    #[test]
    fn classifies_interfaces() {
        // A 2014 laser: one bidirectional printer interface.
        let c = classify([(0, 7, 1, 2)]);
        assert_eq!(
            c,
            PrinterInterfaces {
                ipp_usb: false,
                legacy: true,
                first_interface: Some(0)
            }
        );
        // A modern all-in-one: legacy interface plus two IPP-USB ones.
        let c = classify([(0, 7, 1, 2), (1, 7, 1, 4), (2, 7, 1, 4)]);
        assert!(c.ipp_usb && c.legacy);
        assert_eq!(c.first_interface, Some(0));
        // A webcam and a hub are not printers.
        assert!(!classify([(0, 14, 1, 0), (1, 14, 2, 0)]).is_printer());
        assert!(!classify([(0, 9, 0, 0)]).is_printer());
        // Printer class with a non-printer subclass does not count.
        assert!(!classify([(0, 7, 0, 2)]).is_printer());
    }
}
