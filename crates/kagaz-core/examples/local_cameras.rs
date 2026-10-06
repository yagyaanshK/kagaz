//! Diagnostic: cameras answering ONVIF discovery on this network, with the
//! hardware address from the neighbour table and, for Tapo cameras on the
//! account, which cloud entry they are.
use kagaz_core::cameras::local;
use kagaz_core::cameras::tapo::Session;
use std::time::Duration;
fn main() {
    let found = local::probe(Duration::from_secs(3)).expect("probe");
    let table = local::neighbours();
    println!("{} camera(s) answered", found.len());
    for c in &found {
        let mac = c
            .mac_from_scopes()
            .or_else(|| table.get(&c.address).cloned());
        println!(
            "  {} hardware={:?} name={:?} mac={} xaddrs={}",
            c.address,
            c.hardware(),
            c.name().map(|_| "<set>"),
            mac.as_deref()
                .map(|m| format!("<{} hex>", m.len()))
                .unwrap_or_else(|| "none".into()),
            c.xaddrs.len()
        );
    }
    if let Ok(session) = Session::load(&Session::default_path()) {
        let cams = session.cameras().unwrap_or_default();
        let macs: Vec<String> = cams.iter().map(|c| c.mac.clone()).collect();
        let paired = local::find_by_mac(&macs, Duration::from_secs(3));
        for c in &cams {
            if let Some(ip) = paired.get(&c.mac) {
                println!(
                    "  cloud camera #{} ({}) is local at {ip}",
                    cams.iter()
                        .position(|x| x.device_id == c.device_id)
                        .unwrap(),
                    c.model
                );
            }
        }
        println!(
            "{} of {} account cameras found locally",
            paired.len(),
            cams.len()
        );
    }
}
