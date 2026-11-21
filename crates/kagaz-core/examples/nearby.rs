//! Everything on the local network, as Home's "Found on your network" sees
//! it: `cargo run -p kagaz-core --example nearby`.
fn main() {
    let start = std::time::Instant::now();
    let hosts = kagaz_core::nearby::sweep(std::time::Duration::from_secs(4));
    for h in &hosts {
        println!(
            "{:<15} {:<10} {:<28} {:<20} {:<24} {:?} {:?}",
            h.ip.to_string(),
            h.kind,
            h.name.as_deref().unwrap_or("-"),
            if h.private_mac { "(private address)" } else { h.maker.as_deref().unwrap_or("-") },
            h.model.as_deref().unwrap_or("-"),
            h.services,
            h.open
        );
    }
    println!("{} in {:.1} s", hosts.len(), start.elapsed().as_secs_f32());
}
