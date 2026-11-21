//! Everything on the local network, as Home's "Found on your network" sees
//! it: `cargo run -p kagaz-core --example nearby`. With an address after
//! it, every port of that device, each asked what it is:
//! `cargo run -p kagaz-core --example nearby -- 192.0.2.10`.
fn main() {
    let start = std::time::Instant::now();
    if let Some(ip) = std::env::args().nth(1).and_then(|a| a.parse().ok()) {
        let c = kagaz_core::nearby::every_port(kagaz_core::nearby::Host { ip, ..Default::default() });
        for p in &c.host.ports {
            println!("{:>5}  {:<26} {}", p.port, p.what, p.detail.as_deref().unwrap_or(""));
        }
        println!("{} ports checked, {} open, {:?} {:?}, in {:.1} s", c.checked, c.host.ports.len(), c.host.kind, c.host.os, start.elapsed().as_secs_f32());
        return;
    }
    let hosts = kagaz_core::nearby::sweep(std::time::Duration::from_secs(4));
    for h in &hosts {
        println!(
            "{:<15} {:<10} {:<24} {:<18} {:<14} {:<8} {:<20} {:?}",
            h.ip.to_string(),
            h.kind,
            h.name.as_deref().unwrap_or("-"),
            if h.private_mac { "(private address)" } else { h.maker.as_deref().unwrap_or("-") },
            h.os.as_deref().unwrap_or("-"),
            h.form.as_deref().unwrap_or("-"),
            h.model.as_deref().unwrap_or("-"),
            h.ports.iter().map(|p| format!("{} {}", p.port, p.what)).collect::<Vec<_>>()
        );
    }
    println!("{} in {:.1} s", hosts.len(), start.elapsed().as_secs_f32());
}
