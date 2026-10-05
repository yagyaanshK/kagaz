//! `kagaz`: the command-line face of Kagaz. Everything the desktop app can do,
//! scriptable and fast.

use anyhow::Result;
use clap::{Parser, Subcommand};
use kagaz_core::{
    discover, explain, open_ports, Device, DiscoverOptions, Host, Protocol, SelectError,
};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "kagaz",
    version,
    about = "Printers and scanners that just work"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Find printers and scanners on the network and say what they can do
    Discover {
        /// Seconds to listen for answers
        #[arg(long, default_value_t = 3)]
        timeout: u64,
        /// Print machine-readable JSON instead of a table
        #[arg(long, alias = "report")]
        json: bool,
        /// Skip mDNS/DNS-SD (AirPrint, IPP Everywhere, eSCL announcements)
        #[arg(long)]
        no_mdns: bool,
        /// Skip WS-Discovery (the Windows "Web Services" protocol)
        #[arg(long)]
        no_wsd: bool,
        /// Skip the USB bus
        #[arg(long)]
        no_usb: bool,
        /// Skip SNMP (the broadcast question for printers that announce nothing)
        #[arg(long)]
        no_snmp: bool,
    },
    /// Say in plain words whether this computer can print to and scan from a device, and what would fix it
    Explain {
        /// Its number in `kagaz discover`, an IP address, hostname, USB vendor:product id, or part of its name
        device: String,
        /// Seconds to listen for answers while finding the device
        #[arg(long, default_value_t = 3)]
        timeout: u64,
        /// Print machine-readable JSON instead of prose
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Discover {
            timeout,
            json,
            no_mdns,
            no_wsd,
            no_usb,
            no_snmp,
        } => {
            let opts = DiscoverOptions {
                timeout: Duration::from_secs(timeout),
                use_mdns: !no_mdns,
                use_wsd: !no_wsd,
                use_usb: !no_usb,
                use_snmp: !no_snmp,
            };
            let devices = discover(&opts)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&devices)?);
                return Ok(());
            }
            if devices.is_empty() {
                let network = opts.use_mdns || opts.use_wsd || opts.use_snmp;
                match (opts.use_usb, network) {
                    (true, true) => println!(
                        "No printers or scanners found on USB or within {timeout} s on the network."
                    ),
                    (true, false) => println!("No printers or scanners found on USB."),
                    _ => println!("No printers or scanners answered within {timeout} s."),
                }
                if network {
                    println!("Try a longer --timeout, and check that this computer and the device are on the same network.");
                }
                return Ok(());
            }
            for (i, d) in devices.iter().enumerate() {
                println!("{}. {}", i + 1, title(d));
                if let Some(m) = &d.model {
                    println!(
                        "  model      {}{}",
                        d.manufacturer
                            .as_deref()
                            .filter(|mf| !m.starts_with(mf))
                            .map(|mf| format!("{mf} "))
                            .unwrap_or_default(),
                        m
                    );
                }
                if !d.addresses.is_empty() {
                    let addrs: Vec<String> = d.addresses.iter().map(|a| a.to_string()).collect();
                    println!(
                        "  address    {}{}",
                        addrs.join(", "),
                        d.hostname
                            .as_deref()
                            .map(|h| format!("  ({h})"))
                            .unwrap_or_default()
                    );
                }
                if let Some(u) = &d.usb {
                    println!(
                        "  usb        bus {} device {}  ({:04x}:{:04x}){}",
                        u.bus,
                        u.address,
                        u.vendor_id,
                        u.product_id,
                        u.serial
                            .as_deref()
                            .map(|s| format!("  serial {s}"))
                            .unwrap_or_default()
                    );
                }
                if let Some(serial) = d.attribute("serial") {
                    println!("  serial     {serial}");
                }
                if let Some(pages) = d.attribute("pages_printed") {
                    println!("  pages      {pages} printed so far");
                }
                let protos: Vec<&str> = d.protocols().iter().map(|p| p.label()).collect();
                println!("  protocols  {}", protos.join(", "));
                let print_std = d.driverless_print_standards().join(", ");
                let scan_std = d.driverless_scan_standards().join(", ");
                let driver_print = d.has_protocol(Protocol::PdlDataStream)
                    || d.has_protocol(Protocol::Lpd)
                    || d.has_protocol(Protocol::UsbPrinter);
                let driver_print_note = if d.has_protocol(Protocol::UsbPrinter) {
                    "needs the vendor's printer driver (classic USB printer port)"
                } else {
                    "needs a driver (raw/LPD only)"
                };
                let snmp_only = d.protocols() == [Protocol::Snmp];
                if snmp_only {
                    println!("  print      unknown: answers SNMP but advertises no print service");
                    println!("  scan       unknown: answers SNMP but advertises no scan service");
                    println!();
                    continue;
                }
                println!(
                    "  print      {}",
                    verdict(
                        d.can_print_driverless(),
                        driver_print,
                        &format!("driverless ({print_std})"),
                        driver_print_note
                    )
                );
                let scan = if d.may_scan_over_ipp_usb() {
                    "likely, driverless over IPP-USB (eSCL); not checked over the cable yet"
                        .to_string()
                } else {
                    verdict(
                        d.can_scan_driverless(),
                        d.has_protocol(Protocol::SaneNet),
                        &format!("driverless ({scan_std})"),
                        "needs the vendor's scanner driver",
                    )
                };
                println!("  scan       {scan}");
                if let Some(note) = d.identity_note() {
                    println!("  note       {note}");
                }
                println!();
            }
            println!("Run `kagaz explain <number>` for what this computer can do with a device and what would fix it.");
            Ok(())
        }
        Command::Explain {
            device,
            timeout,
            json,
        } => {
            let opts = DiscoverOptions {
                timeout: Duration::from_secs(timeout),
                ..Default::default()
            };
            let devices = discover(&opts)?;
            let d = match kagaz_core::find(&devices, &device) {
                Ok(d) => d,
                Err(SelectError::Ambiguous { candidates, .. }) => {
                    eprintln!("\"{device}\" matches several devices:");
                    for i in candidates {
                        eprintln!("  {i}. {}", title(&devices[i - 1]));
                    }
                    anyhow::bail!("pick one by number: kagaz explain <number>");
                }
                Err(e) => {
                    if devices.is_empty() {
                        anyhow::bail!("{e}: nothing was found at all (see `kagaz discover`)");
                    }
                    eprintln!("Found:");
                    for (i, d) in devices.iter().enumerate() {
                        eprintln!("  {}. {}", i + 1, title(d));
                    }
                    anyhow::bail!("{e}");
                }
            };
            let host = Host::detect();
            let ports = d
                .addresses
                .iter()
                .find(|a| a.is_ipv4())
                .map(|ip| open_ports(*ip, Duration::from_millis(800)))
                .unwrap_or_default();
            let e = explain(d, &host, &ports);
            if json {
                println!("{}", serde_json::to_string_pretty(&e)?);
                return Ok(());
            }
            let place = if let Some(ip) = d.addresses.first() {
                format!("at {ip}")
            } else if let Some(u) = &d.usb {
                format!("on USB ({:04x}:{:04x})", u.vendor_id, u.product_id)
            } else {
                String::new()
            };
            println!(
                "{} {place}  (this computer runs {})",
                e.device,
                e.host.os.label()
            );
            println!();
            println!("Print: {}", e.print.summary);
            for line in &e.print.details {
                println!("  {line}");
            }
            println!();
            println!("Scan: {}", e.scan.summary);
            for line in &e.scan.details {
                println!("  {line}");
            }
            if !e.notes.is_empty() {
                println!();
                println!("Also:");
                for line in &e.notes {
                    println!("  {line}");
                }
            }
            Ok(())
        }
    }
}

fn title(d: &Device) -> String {
    if d.name.is_empty() {
        d.model.clone().unwrap_or_else(|| "Unknown device".into())
    } else {
        d.name.clone()
    }
}

fn verdict(driverless: bool, driver_only: bool, yes: &str, needs_driver: &str) -> String {
    if driverless {
        format!("yes, {yes}")
    } else if driver_only {
        format!("possible, {needs_driver}")
    } else {
        "not advertised".to_string()
    }
}
