//! `kagaz`: the command-line face of Kagaz. Everything the desktop app can do,
//! scriptable and fast.

use anyhow::Result;
use clap::{Parser, Subcommand};
use kagaz_core::{discover, DiscoverOptions, Protocol};
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
        #[arg(long)]
        json: bool,
        /// Skip mDNS/DNS-SD (AirPrint, IPP Everywhere, eSCL announcements)
        #[arg(long)]
        no_mdns: bool,
        /// Skip WS-Discovery (the Windows "Web Services" protocol)
        #[arg(long)]
        no_wsd: bool,
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
        } => {
            let opts = DiscoverOptions {
                timeout: Duration::from_secs(timeout),
                use_mdns: !no_mdns,
                use_wsd: !no_wsd,
            };
            let devices = discover(&opts)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&devices)?);
                return Ok(());
            }
            if devices.is_empty() {
                println!("No printers or scanners answered within {timeout} s.");
                println!("Try a longer --timeout, and check that this computer and the device are on the same network.");
                return Ok(());
            }
            for d in &devices {
                let title = if d.name.is_empty() {
                    d.model.clone().unwrap_or_else(|| "Unknown device".into())
                } else {
                    d.name.clone()
                };
                println!("{title}");
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
                let addrs: Vec<String> = d.addresses.iter().map(|a| a.to_string()).collect();
                println!(
                    "  address    {}{}",
                    addrs.join(", "),
                    d.hostname
                        .as_deref()
                        .map(|h| format!("  ({h})"))
                        .unwrap_or_default()
                );
                let protos: Vec<&str> = d.protocols().iter().map(|p| p.label()).collect();
                println!("  protocols  {}", protos.join(", "));
                println!(
                    "  print      {}",
                    verdict(
                        d.can_print_driverless(),
                        d.has_protocol(Protocol::PdlDataStream) || d.has_protocol(Protocol::Lpd),
                        "driverless (IPP/WSD)",
                        "needs a driver (raw/LPD only)"
                    )
                );
                println!(
                    "  scan       {}",
                    verdict(
                        d.can_scan_driverless(),
                        d.has_protocol(Protocol::SaneNet),
                        "driverless (eSCL/WSD)",
                        "needs the vendor's scanner driver"
                    )
                );
                println!();
            }
            Ok(())
        }
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
