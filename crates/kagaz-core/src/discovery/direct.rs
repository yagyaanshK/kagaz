//! Asking a found device directly for the services multicast should have
//! announced. mDNS answers go missing on busy Wi-Fi, behind the Windows
//! firewall, and on Android unless the app holds a multicast lock; WSD and
//! SNMP may still have found the device. Then one IPP request on port 631
//! and one eSCL request on port 80 show whether it prints and scans
//! without a driver, the same as the announcement would have.

use crate::device::{Device, Protocol, Service};
use crate::ipp::{self, call, encode_request, out, TAG_KEYWORD};
use std::collections::BTreeMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

/// The resource paths printers use for IPP, most common first.
const IPP_PATHS: &[&str] = &["ipp/print", "ipp", ""];

/// Fill in IPP and eSCL for network devices that lack them, asking each
/// device at most a few short questions, all devices side by side.
pub fn fill(devices: &mut [Device], wait: Duration) {
    std::thread::scope(|s| {
        for d in devices.iter_mut() {
            let Some(ip) = d.addresses.iter().find(|a| a.is_ipv4()).copied() else {
                continue;
            };
            let need_ipp = !d.has_protocol(Protocol::Ipp);
            let need_escl = !d.has_protocol(Protocol::Escl);
            if !need_ipp && !need_escl {
                continue;
            }
            s.spawn(move || {
                if need_ipp {
                    if let Some(svc) = ipp_service(ip, wait) {
                        d.add_service(svc);
                    }
                }
                if need_escl {
                    if let Some(svc) = escl_service(ip, wait) {
                        d.add_service(svc);
                    }
                }
            });
        }
    });
}

fn port_open(ip: IpAddr, port: u16, wait: Duration) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::new(ip, port),
        wait.min(Duration::from_millis(600)),
    )
    .is_ok()
}

fn host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
}

/// An IPP service, if the device answers Get-Printer-Attributes and takes
/// a driverless format.
fn ipp_service(ip: IpAddr, wait: Duration) -> Option<Service> {
    if !port_open(ip, 631, wait) {
        return None;
    }
    let h = host(ip);
    for path in IPP_PATHS {
        let request = encode_request(
            ipp::OP_GET_PRINTER_ATTRIBUTES,
            1,
            &format!("ipp://{h}:631/{path}"),
            &[out(
                TAG_KEYWORD,
                "requested-attributes",
                &[
                    "document-format-supported",
                    "printer-make-and-model",
                    "printer-uuid",
                ],
            )],
            &[],
            &[],
        );
        let Ok(r) = call(&format!("http://{h}:631/{path}"), &request, wait) else {
            continue;
        };
        if !r.ok() {
            continue;
        }
        let formats = r
            .printer("document-format-supported")
            .map(|a| a.strings())
            .unwrap_or_default();
        let mut attributes = BTreeMap::new();
        attributes.insert("pdl".to_string(), formats.join(","));
        if let Some(m) = r
            .printer("printer-make-and-model")
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
        {
            attributes.insert("ty".to_string(), m.to_string());
        }
        return Some(Service {
            protocol: Protocol::Ipp,
            source: "direct".into(),
            port: Some(631),
            endpoint: Some(path.to_string()),
            attributes,
        });
    }
    None
}

/// An eSCL service, if the device serves scanner capabilities on port 80.
fn escl_service(ip: IpAddr, wait: Duration) -> Option<Service> {
    if !port_open(ip, 80, wait) {
        return None;
    }
    let url = format!("http://{}:80/eSCL/ScannerCapabilities", host(ip));
    let agent = ureq::AgentBuilder::new().timeout(wait).redirects(0).build();
    let resp = agent.get(&url).call().ok()?;
    if resp.status() != 200 {
        return None;
    }
    let mut body = String::new();
    resp.into_reader()
        .take(256 * 1024)
        .read_to_string(&mut body)
        .ok()?;
    if !body.contains("ScannerCapabilities") {
        return None;
    }
    Some(Service {
        protocol: Protocol::Escl,
        source: "direct".into(),
        port: Some(80),
        endpoint: Some("eSCL".into()),
        attributes: BTreeMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_port_gives_nothing_quickly() {
        // Port 9 (discard) on loopback is closed on any normal machine.
        let start = std::time::Instant::now();
        assert!(!port_open(
            "127.0.0.1".parse().unwrap(),
            9,
            Duration::from_millis(300)
        ));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
