//! Plain words about one device: can this computer print to it and scan from
//! it right now, why or why not, and what would fix it. The reasoning lives
//! here so the CLI and the desktop app say exactly the same thing.

use crate::device::{Device, Protocol};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Os {
    Linux,
    Windows,
    MacOs,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Os::Linux => "Linux",
            Os::Windows => "Windows",
            Os::MacOs => "macOS",
            Os::Other => "this operating system",
        }
    }
}

/// What the computer running Kagaz has, as far as the verdict depends on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub os: Os,
    /// Linux: the open-source `sane-airscan` backend (eSCL and WSD scanning) is installed.
    pub has_sane_airscan: bool,
    /// Linux: the `ipp-usb` service that turns an IPP-USB printer into a local network printer.
    pub has_ipp_usb: bool,
}

impl Host {
    /// Look at this computer.
    pub fn detect() -> Host {
        let os = Os::current();
        let (has_sane_airscan, has_ipp_usb) = if os == Os::Linux {
            (
                exists_any(&[
                    "/usr/lib/x86_64-linux-gnu/sane/libsane-airscan.so.1",
                    "/usr/lib/aarch64-linux-gnu/sane/libsane-airscan.so.1",
                    "/usr/lib64/sane/libsane-airscan.so.1",
                    "/usr/lib/sane/libsane-airscan.so.1",
                    "/usr/bin/airscan-discover",
                ]),
                exists_any(&["/usr/sbin/ipp-usb", "/usr/bin/ipp-usb"]),
            )
        } else {
            (false, false)
        };
        Host {
            os,
            has_sane_airscan,
            has_ipp_usb,
        }
    }
}

fn exists_any(paths: &[&str]) -> bool {
    paths.iter().any(|p| std::path::Path::new(p).exists())
}

/// How far one function (print or scan) is from working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Works now, with nothing to install.
    Works,
    /// Works after a step this computer can take (install a package, run a service).
    NeedsSetup,
    /// Needs the vendor's driver.
    NeedsDriver,
    /// The device does not offer it, or did not say.
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub status: Status,
    /// One line, e.g. "works without a driver (IPP Everywhere)".
    pub summary: String,
    /// Plain sentences: what the device offers, what this OS does with it, what would fix it.
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Explanation {
    pub device: String,
    pub host: Host,
    pub print: Verdict,
    pub scan: Verdict,
    /// Anything else worth knowing: identity notes, open ports, status.
    pub notes: Vec<String>,
}

/// Ports worth a knock on a network printer, with what an answer means.
pub const PROBE_PORTS: &[(u16, &str)] = &[
    (631, "IPP"),
    (9100, "raw printing"),
    (515, "LPD"),
    (80, "web interface"),
    (443, "web interface over TLS"),
];

/// Which of `PROBE_PORTS` accept a TCP connection at `ip`.
pub fn open_ports(ip: IpAddr, timeout: Duration) -> Vec<(u16, &'static str)> {
    PROBE_PORTS
        .iter()
        .copied()
        .filter(|(port, _)| {
            TcpStream::connect_timeout(&SocketAddr::new(ip, *port), timeout).is_ok()
        })
        .collect()
}

/// Explain `device` for `host`. `ports` are the open ports found by
/// [`open_ports`], or empty when nothing was checked.
pub fn explain(device: &Device, host: &Host, ports: &[(u16, &'static str)]) -> Explanation {
    let title = if device.name.is_empty() {
        device
            .model
            .clone()
            .unwrap_or_else(|| "Unknown device".into())
    } else {
        device.name.clone()
    };
    let mut notes = Vec::new();
    if let Some(n) = device.identity_note() {
        notes.push(n.to_string());
    }
    let print = explain_print(device, host, ports);
    let scan = explain_scan(device, host);
    if !ports.is_empty() {
        let list: Vec<String> = ports
            .iter()
            .map(|(p, what)| format!("{p} ({what})"))
            .collect();
        notes.push(format!("Open ports: {}.", list.join(", ")));
    }
    if let Some(serial) = device.attribute("serial") {
        let mut line = format!("Serial number {serial}");
        if let Some(pages) = device.attribute("pages_printed") {
            line.push_str(&format!(", {pages} pages printed so far"));
        }
        line.push_str(", as reported over SNMP.");
        notes.push(line);
    }
    Explanation {
        device: title,
        host: host.clone(),
        print,
        scan,
        notes,
    }
}

fn pdl(device: &Device) -> String {
    device
        .services
        .iter()
        .filter(|s| matches!(s.protocol, Protocol::Ipp | Protocol::Ipps))
        .find_map(|s| s.attributes.get("pdl"))
        .cloned()
        .or_else(|| device.attribute("CMD").map(str::to_string))
        .unwrap_or_default()
}

fn explain_print(device: &Device, host: &Host, ports: &[(u16, &'static str)]) -> Verdict {
    let os = host.os;
    let pdl = pdl(device);
    let ipp_everywhere = pdl.contains("image/pwg-raster");
    let airprint = pdl.contains("image/urf") || pdl.contains("URF");
    let ipp = device.has_protocol(Protocol::Ipp) || device.has_protocol(Protocol::Ipps);
    let wsd = device.has_protocol(Protocol::WsdPrint);
    let ipp_usb = device.has_protocol(Protocol::IppUsb);
    let usb_classic = device.has_protocol(Protocol::UsbPrinter);
    let raw = device.has_protocol(Protocol::PdlDataStream) || device.has_protocol(Protocol::Lpd);
    let mut details = Vec::new();

    if ipp && (ipp_everywhere || airprint) {
        let mut standards = Vec::new();
        if ipp_everywhere {
            standards.push("IPP Everywhere");
        }
        if airprint {
            standards.push("AirPrint");
        }
        details.push(format!(
            "The printer accepts print jobs over IPP in the {} format{}, so no vendor driver is needed.",
            standards.join(" and "),
            if standards.len() > 1 { "s" } else { "" }
        ));
        details.push(match os {
            Os::Linux => "Linux prints to it through CUPS's built-in driverless support (CUPS 2.2 or newer, as on every current distribution).".to_string(),
            Os::Windows => "Windows 10 and 11 print to it with the built-in Microsoft IPP class driver.".to_string(),
            Os::MacOs if airprint => "macOS prints to it as an AirPrint printer, nothing to install.".to_string(),
            Os::MacOs => "macOS has no AirPrint (URF) support on this printer; it should still print over IPP Everywhere, but that is not verified.".to_string(),
            Os::Other => "Any IPP Everywhere-capable system prints to it without a driver.".to_string(),
        });
        details.push("Kagaz will also print to it directly over IPP (milestone 3).".to_string());
        return Verdict {
            status: Status::Works,
            summary: format!("works without a driver ({})", standards.join(", ")),
            details,
        };
    }

    if ipp_usb {
        details.push("The printer speaks IPP over the USB cable (IPP-USB), which carries the same driverless printing as a network printer.".to_string());
        let (status, summary) = match os {
            Os::Linux if host.has_ipp_usb => {
                details.push("The ipp-usb service is installed; it presents the printer to CUPS as a local network printer.".to_string());
                (
                    Status::Works,
                    "works without a driver (IPP-USB)".to_string(),
                )
            }
            Os::Linux => {
                details.push("Linux needs the small ipp-usb service to use it (package `ipp-usb` on Debian, Ubuntu, Fedora and Arch); after that CUPS sees it as a driverless network printer.".to_string());
                (
                    Status::NeedsSetup,
                    "works after installing ipp-usb".to_string(),
                )
            }
            Os::Windows => {
                details.push("Windows 10 (version 1903 or newer) and 11 support IPP over USB out of the box.".to_string());
                (
                    Status::Works,
                    "works without a driver (IPP-USB)".to_string(),
                )
            }
            Os::MacOs => {
                details.push(
                    "macOS treats it as an AirPrint printer on USB, nothing to install."
                        .to_string(),
                );
                (
                    Status::Works,
                    "works without a driver (IPP-USB)".to_string(),
                )
            }
            Os::Other => (
                Status::Unknown,
                "IPP-USB printer; support depends on the system".to_string(),
            ),
        };
        return Verdict {
            status,
            summary,
            details,
        };
    }

    if ipp {
        details.push("The printer accepts IPP jobs but did not list the driverless formats (PWG Raster or URF).".to_string());
        details.push("It may still work with the system's driverless support; otherwise it needs the vendor's driver.".to_string());
        return Verdict {
            status: Status::Unknown,
            summary: "IPP, driverless support not confirmed".to_string(),
            details,
        };
    }

    if wsd {
        details.push("The printer offers WSD printing, the protocol Windows uses to add network printers by itself.".to_string());
        let (status, summary) = match os {
            Os::Windows => (Status::Works, "works without a driver (WSD)".to_string()),
            _ => {
                details.push(format!(
                    "{} does not print over WSD; this printer needs the vendor's driver here.",
                    os.label()
                ));
                (
                    Status::NeedsDriver,
                    "needs the vendor's driver (WSD is Windows-only)".to_string(),
                )
            }
        };
        return Verdict {
            status,
            summary,
            details,
        };
    }

    if usb_classic {
        details.push("The printer is on USB with the classic printer port only (no IPP-USB), so the computer has to speak the printer's own page language.".to_string());
        if !pdl.is_empty() {
            details.push(format!("It says it understands: {pdl}."));
            if pdl.contains("PCL") || pdl.contains("PostScript") || pdl.contains("POSTSCRIPT") {
                details.push("A generic PCL or PostScript driver may work; the vendor's own driver is the reliable choice.".to_string());
            }
        }
        details.push("Kagaz's driver finder (milestone 4) will fetch the vendor's official driver for this model.".to_string());
        return Verdict {
            status: Status::NeedsDriver,
            summary: "needs the vendor's printer driver".to_string(),
            details,
        };
    }

    if raw {
        details.push("The printer only offers raw port-9100 or LPD printing, which sends data in the printer's own language; that needs the vendor's driver.".to_string());
        return Verdict {
            status: Status::NeedsDriver,
            summary: "needs the vendor's printer driver (raw/LPD only)".to_string(),
            details,
        };
    }

    if device.has_protocol(Protocol::Snmp) {
        details.push("The printer answers SNMP but did not announce any print service over mDNS or WS-Discovery.".to_string());
        if ports.iter().any(|(p, _)| *p == 631) {
            details.push("Port 631 (IPP) is open, so driverless printing is probably possible; enabling Bonjour/mDNS (sometimes called AirPrint or Mopria) in the printer's network settings would let every computer find it by itself.".to_string());
        } else if ports.iter().any(|(p, _)| *p == 9100 || *p == 515) {
            details.push(
                "Only raw or LPD printing ports are open; that path needs the vendor's driver."
                    .to_string(),
            );
        } else {
            details.push("No printing port answered either; the printer may be asleep, on another network, or blocking this computer.".to_string());
        }
        return Verdict {
            status: Status::Unknown,
            summary: "unknown: found over SNMP only".to_string(),
            details,
        };
    }

    details.push("The device advertises no printing at all.".to_string());
    Verdict {
        status: Status::Unknown,
        summary: "not advertised".to_string(),
        details,
    }
}

fn explain_scan(device: &Device, host: &Host) -> Verdict {
    let os = host.os;
    let escl = device.has_protocol(Protocol::Escl) || device.has_protocol(Protocol::Escls);
    let wsd = device.has_protocol(Protocol::WsdScan);
    let ipp_usb = device.has_protocol(Protocol::IppUsb);
    let sane_net = device.has_protocol(Protocol::SaneNet);
    let mut details = Vec::new();

    if escl || wsd {
        let mut standards = Vec::new();
        if escl {
            standards.push("eSCL");
        }
        if wsd {
            standards.push("WSD");
        }
        details.push(format!(
            "The scanner speaks {}, so no vendor driver is needed to scan from it.",
            standards.join(" and ")
        ));
        let native = match os {
            Os::Linux if host.has_sane_airscan => {
                details.push("The open-source sane-airscan backend is installed, so every SANE application (Document Scanner, simple-scan, scanimage) can use it today.".to_string());
                true
            }
            Os::Linux => {
                details.push("Linux has no built-in eSCL or WSD scanning; the open-source sane-airscan backend adds both (package `sane-airscan`).".to_string());
                false
            }
            Os::Windows if wsd => {
                details.push("Windows scans from it natively over WSD (Windows Fax and Scan, or any app using Windows Image Acquisition).".to_string());
                true
            }
            Os::Windows => {
                details.push("Windows 11 includes an eSCL scanner class driver; older Windows versions need the vendor's scanner driver.".to_string());
                false
            }
            Os::MacOs if escl => {
                details.push("macOS scans from it natively over eSCL (AirScan) in Image Capture and Preview.".to_string());
                true
            }
            Os::MacOs => {
                details.push(
                    "macOS does not scan over WSD; the vendor's scanner driver is needed there."
                        .to_string(),
                );
                false
            }
            Os::Other => false,
        };
        details.push("Kagaz will scan from it directly over eSCL and WSD on every OS (milestone 2), so none of this will be needed.".to_string());
        let status = if native {
            Status::Works
        } else {
            Status::NeedsSetup
        };
        return Verdict {
            status,
            summary: if native {
                format!("works without a driver ({})", standards.join(", "))
            } else {
                format!(
                    "driverless ({}) once Kagaz scans itself; today needs a helper",
                    standards.join(", ")
                )
            },
            details,
        };
    }

    if ipp_usb {
        details.push("IPP-USB devices usually carry eSCL scanning over the same cable, but that has not been checked yet.".to_string());
        return Verdict {
            status: Status::Unknown,
            summary: "likely driverless over IPP-USB (eSCL); not checked yet".to_string(),
            details,
        };
    }

    if sane_net {
        details.push("The device announces only its vendor's own network scanning protocol (the `_scanner._tcp` record); that needs the vendor's scanner driver.".to_string());
        if device.manufacturer.as_deref() == Some("Brother") {
            details.push(match os {
                Os::Linux => "For Brother on Linux that is the official brscan4 package, plus brscan-skey for the printer's Scan button.".to_string(),
                _ => "For Brother that is the official full driver package from support.brother.com.".to_string(),
            });
        }
        details.push("Kagaz's driver finder (milestone 4) will fetch and install it after showing you what it does.".to_string());
        return Verdict {
            status: Status::NeedsDriver,
            summary: "needs the vendor's scanner driver".to_string(),
            details,
        };
    }

    details.push("The device advertises no scanning.".to_string());
    Verdict {
        status: Status::Unknown,
        summary: "not advertised".to_string(),
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Service, UsbInfo};
    use std::collections::BTreeMap;

    fn host(os: Os) -> Host {
        Host {
            os,
            has_sane_airscan: false,
            has_ipp_usb: false,
        }
    }

    fn svc(protocol: Protocol, source: &str, attrs: &[(&str, &str)]) -> Service {
        Service {
            protocol,
            source: source.into(),
            port: None,
            endpoint: None,
            attributes: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn brother() -> Device {
        Device {
            name: "Brother DCP-L2540DW series".into(),
            manufacturer: Some("Brother".into()),
            addresses: vec!["198.51.100.167".parse().unwrap()],
            services: vec![
                svc(
                    Protocol::Ipp,
                    "mdns",
                    &[("pdl", "application/octet-stream,image/urf,image/pwg-raster")],
                ),
                svc(Protocol::WsdPrint, "wsd", &[]),
                svc(Protocol::WsdScan, "wsd", &[]),
                svc(Protocol::PdlDataStream, "mdns", &[]),
                svc(Protocol::SaneNet, "mdns", &[]),
                svc(
                    Protocol::Snmp,
                    "snmp",
                    &[("serial", "KAGAZ0000000001"), ("pages_printed", "302")],
                ),
            ],
            ..Device::default()
        }
    }

    #[test]
    fn brother_on_each_os() {
        let d = brother();
        let linux = explain(&d, &host(Os::Linux), &[(631, "IPP")]);
        assert_eq!(linux.print.status, Status::Works);
        assert_eq!(
            linux.print.summary,
            "works without a driver (IPP Everywhere, AirPrint)"
        );
        assert_eq!(linux.scan.status, Status::NeedsSetup);
        assert!(linux
            .scan
            .details
            .iter()
            .any(|s| s.contains("sane-airscan")));
        assert!(linux
            .notes
            .iter()
            .any(|n| n.contains("KAGAZ0000000001") && n.contains("302")));
        assert!(linux.notes.iter().any(|n| n.starts_with("Open ports: 631")));

        let mut airscan = host(Os::Linux);
        airscan.has_sane_airscan = true;
        assert_eq!(explain(&d, &airscan, &[]).scan.status, Status::Works);

        let windows = explain(&d, &host(Os::Windows), &[]);
        assert_eq!(windows.print.status, Status::Works);
        assert_eq!(windows.scan.status, Status::Works);
        assert!(windows
            .scan
            .details
            .iter()
            .any(|s| s.contains("natively over WSD")));

        let mac = explain(&d, &host(Os::MacOs), &[]);
        assert_eq!(mac.print.status, Status::Works);
        assert!(mac.print.details.iter().any(|s| s.contains("AirPrint")));
        assert_eq!(mac.scan.status, Status::NeedsSetup);
    }

    #[test]
    fn snmp_only_printer_gets_a_port_hint() {
        let d = Device {
            name: "HP LaserJet 400".into(),
            addresses: vec!["198.51.100.90".parse().unwrap()],
            services: vec![svc(Protocol::Snmp, "snmp", &[])],
            ..Device::default()
        };
        let e = explain(&d, &host(Os::Linux), &[(631, "IPP"), (80, "web interface")]);
        assert_eq!(e.print.status, Status::Unknown);
        assert!(e.print.details.iter().any(|s| s.contains("Port 631")));
        let e = explain(&d, &host(Os::Linux), &[(9100, "raw printing")]);
        assert!(e.print.details.iter().any(|s| s.contains("raw or LPD")));
        let e = explain(&d, &host(Os::Linux), &[]);
        assert!(e
            .print
            .details
            .iter()
            .any(|s| s.contains("No printing port")));
        assert_eq!(e.scan.summary, "not advertised");
    }

    #[test]
    fn usb_printers() {
        let usb = |protocol: Protocol| Device {
            name: "HP LaserJet 1020".into(),
            usb: Some(UsbInfo {
                bus: 1,
                address: 2,
                vendor_id: 0x03f0,
                product_id: 0x2b17,
                serial: None,
            }),
            services: vec![svc(protocol, "usb", &[("CMD", "ACL,PCL")])],
            ..Device::default()
        };
        let classic = usb(Protocol::UsbPrinter);
        let e = explain(&classic, &host(Os::Linux), &[]);
        assert_eq!(e.print.status, Status::NeedsDriver);
        assert!(e.print.details.iter().any(|s| s.contains("generic PCL")));

        let modern = usb(Protocol::IppUsb);
        assert_eq!(
            explain(&modern, &host(Os::Linux), &[]).print.status,
            Status::NeedsSetup
        );
        let mut with_daemon = host(Os::Linux);
        with_daemon.has_ipp_usb = true;
        assert_eq!(
            explain(&modern, &with_daemon, &[]).print.status,
            Status::Works
        );
        assert_eq!(
            explain(&modern, &host(Os::Windows), &[]).print.status,
            Status::Works
        );
        assert_eq!(
            explain(&modern, &host(Os::MacOs), &[]).print.status,
            Status::Works
        );
        assert_eq!(
            explain(&modern, &host(Os::Linux), &[]).scan.status,
            Status::Unknown
        );
    }

    #[test]
    fn wsd_only_printer_is_windows_only() {
        let d = Device {
            name: "Old WSD printer".into(),
            services: vec![svc(Protocol::WsdPrint, "wsd", &[])],
            ..Device::default()
        };
        assert_eq!(
            explain(&d, &host(Os::Windows), &[]).print.status,
            Status::Works
        );
        assert_eq!(
            explain(&d, &host(Os::Linux), &[]).print.status,
            Status::NeedsDriver
        );
    }
}
