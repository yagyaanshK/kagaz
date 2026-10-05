//! `kagaz`: the command-line face of Kagaz. Everything the desktop app can do,
//! scriptable and fast.

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use kagaz_core::ipp::status::PrinterStatus;
use kagaz_core::output::{human_size, parse_size, Format, OutputOptions};
use kagaz_core::print::{Event as PrintEvent, PrintRequest, Sides};
use kagaz_core::scan::{ColorMode, Event, Paper, ScanRequest, Source};
use kagaz_core::{
    discover, explain, open_ports, Device, DiscoverOptions, Host, Protocol, SelectError,
};
use std::path::PathBuf;
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
    /// Find printers and scanners on the network and USB, and say what they can do
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
    /// Scan without a driver (WSD or eSCL) to a PDF, JPEG or PNG file
    Scan {
        /// Its number in `kagaz discover`, an IP address, hostname, or part of its name
        device: String,
        /// Where the paper is; auto = the feeder if it has paper, else the glass
        #[arg(long, value_enum, default_value_t = SourceArg::Auto)]
        source: SourceArg,
        /// Resolution in dots per inch
        #[arg(long, default_value_t = 300)]
        dpi: u32,
        /// Colour, grey or black-and-white (converted here if the scanner only does colour)
        #[arg(long, value_enum, default_value_t = ModeArg::Color)]
        mode: ModeArg,
        /// Output format; defaults to the extension of --output, else pdf
        #[arg(long, value_enum)]
        format: Option<FormatArg>,
        /// Paper size to scan
        #[arg(long, value_enum, default_value_t = PaperArg::A4)]
        paper: PaperArg,
        /// JPEG quality 1-100 when pages are re-encoded (default 85; untouched pages keep the scanner's quality)
        #[arg(long)]
        quality: Option<u8>,
        /// Keep each output file at or under this size, e.g. 2M or 500K; pages are re-encoded and shrunk as needed
        #[arg(long)]
        max_size: Option<String>,
        /// Output file (default: scan-<date>-<time>.<ext> in the current directory)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Seconds to listen for answers while finding the device
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
    /// Ask a printer how it is doing over IPP: state, problems, toner, paper, queue
    Status {
        /// Its number in `kagaz discover`, an IP address, hostname, or part of its name
        device: String,
        /// Seconds to listen for answers while finding the device
        #[arg(long, default_value_t = 3)]
        timeout: u64,
        /// Print machine-readable JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Print a PDF, JPEG or PNG without a driver (IPP Everywhere)
    Print {
        /// Its number in `kagaz discover`, an IP address, hostname, or part of its name
        device: String,
        /// The file to print
        file: PathBuf,
        /// Number of copies
        #[arg(long, default_value_t = 1)]
        copies: u32,
        /// one, long (two-sided, flip on the long edge) or short
        #[arg(long, value_enum, default_value_t = SidesArg::One)]
        sides: SidesArg,
        /// Paper size; the printer's default when not given
        #[arg(long, value_enum)]
        paper: Option<PaperArg>,
        /// Print in black and white even on a colour printer
        #[arg(long)]
        gray: bool,
        /// Do everything except send the job: render, and ask the printer to validate it
        #[arg(long)]
        dry_run: bool,
        /// Seconds to listen for answers while finding the device
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
    /// Make a printer flash its display so you know which one it is
    Identify {
        /// Its number in `kagaz discover`, an IP address, hostname, or part of its name
        device: String,
        /// Seconds to listen for answers while finding the device
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum SidesArg {
    One,
    Long,
    Short,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum SourceArg {
    Auto,
    Glass,
    Feeder,
    Duplex,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum ModeArg {
    Color,
    Gray,
    Bw,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum FormatArg {
    Pdf,
    Jpeg,
    Png,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum PaperArg {
    A4,
    Letter,
    Legal,
    Max,
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
                if d.protocols() == [Protocol::Snmp] {
                    println!("  print      unknown: answers SNMP but advertises no print service");
                    println!("  scan       unknown: answers SNMP but advertises no scan service");
                    println!();
                    continue;
                }
                let driver_print = d.has_protocol(Protocol::PdlDataStream)
                    || d.has_protocol(Protocol::Lpd)
                    || d.has_protocol(Protocol::UsbPrinter);
                let driver_print_note = if d.has_protocol(Protocol::UsbPrinter) {
                    "needs the vendor's printer driver (classic USB printer port)"
                } else {
                    "needs a driver (raw/LPD only)"
                };
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
            println!("Run `kagaz explain <number>` for what this computer can do with a device, or `kagaz scan <number>` to scan.");
            Ok(())
        }
        Command::Explain {
            device,
            timeout,
            json,
        } => {
            let devices = discover(&DiscoverOptions {
                timeout: Duration::from_secs(timeout),
                ..Default::default()
            })?;
            let d = select_device(&devices, &device)?;
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
        Command::Scan {
            device,
            source,
            dpi,
            mode,
            format,
            paper,
            quality,
            max_size,
            output,
            timeout,
        } => {
            let max_bytes = match &max_size {
                Some(s) => Some(parse_size(s).ok_or_else(|| {
                    anyhow::anyhow!("--max-size: cannot read \"{s}\" (try 2M or 500K)")
                })?),
                None => None,
            };
            if let Some(q) = quality {
                anyhow::ensure!((1..=100).contains(&q), "--quality must be 1-100");
            }
            let format = match (format, &output) {
                (Some(f), _) => match f {
                    FormatArg::Pdf => Format::Pdf,
                    FormatArg::Jpeg => Format::Jpeg,
                    FormatArg::Png => Format::Png,
                },
                (None, Some(p)) => p
                    .extension()
                    .and_then(|e| e.to_str())
                    .and_then(Format::from_extension)
                    .unwrap_or(Format::Pdf),
                (None, None) => Format::Pdf,
            };
            let path = output.unwrap_or_else(|| {
                PathBuf::from(format!(
                    "scan-{}.{}",
                    kagaz_core::localtime::now().file_stamp(),
                    format.extension()
                ))
            });
            let color = match mode {
                ModeArg::Color => ColorMode::Color,
                ModeArg::Gray => ColorMode::Gray,
                ModeArg::Bw => ColorMode::BlackWhite,
            };
            let req = ScanRequest {
                source: match source {
                    SourceArg::Auto => Source::Auto,
                    SourceArg::Glass => Source::Glass,
                    SourceArg::Feeder => Source::Feeder,
                    SourceArg::Duplex => Source::FeederDuplex,
                },
                dpi,
                color,
                paper: match paper {
                    PaperArg::A4 => Paper::A4,
                    PaperArg::Letter => Paper::Letter,
                    PaperArg::Legal => Paper::Legal,
                    PaperArg::Max => Paper::Max,
                },
            };

            println!("Looking for \"{device}\"...");
            let devices = discover(&DiscoverOptions {
                timeout: Duration::from_secs(timeout),
                ..Default::default()
            })?;
            let d = select_device(&devices, &device)?;
            println!("Scanning from {}", title(d));
            let pages = kagaz_core::scan::scan(d, &req, &mut |e| match e {
                Event::Starting { source, dpi } => {
                    let where_ = match source {
                        Source::Glass => "the glass",
                        Source::Feeder => "the feeder",
                        Source::FeederDuplex => "the feeder, both sides",
                        Source::Auto => "the scanner",
                    };
                    println!("  from {where_} at {dpi} dpi...");
                }
                Event::FeederEmpty => println!("  the feeder is empty; using the glass instead"),
                Event::Page { number, bytes } => {
                    println!("  page {number} received ({})", human_size(bytes as u64))
                }
                Event::Substituted(note) => println!("  note: {note}"),
            })?;
            let written = kagaz_core::output::write(
                &pages,
                &OutputOptions {
                    format,
                    color,
                    quality,
                    max_bytes,
                },
                &path,
            )?;
            for w in &written {
                println!(
                    "Saved {} ({} page{}, {})",
                    w.path.display(),
                    w.pages,
                    if w.pages == 1 { "" } else { "s" },
                    human_size(w.bytes)
                );
                if let Some(warning) = &w.warning {
                    println!("  warning: {warning}");
                }
            }
            Ok(())
        }
        Command::Status {
            device,
            timeout,
            json,
        } => {
            let devices = discover(&DiscoverOptions {
                timeout: Duration::from_secs(timeout),
                ..Default::default()
            })?;
            let d = select_device(&devices, &device)?;
            let s = kagaz_core::ipp::status::fetch(d)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&s)?);
                return Ok(());
            }
            print_status(d, &s);
            Ok(())
        }
        Command::Print {
            device,
            file,
            copies,
            sides,
            paper,
            gray,
            dry_run,
            timeout,
        } => {
            anyhow::ensure!(file.is_file(), "{} is not a file", file.display());
            let devices = discover(&DiscoverOptions {
                timeout: Duration::from_secs(timeout),
                ..Default::default()
            })?;
            let d = select_device(&devices, &device)?;
            let req = PrintRequest {
                copies,
                sides: match sides {
                    SidesArg::One => Sides::OneSided,
                    SidesArg::Long => Sides::TwoSidedLongEdge,
                    SidesArg::Short => Sides::TwoSidedShortEdge,
                },
                media: paper.map(|p| {
                    match p {
                        PaperArg::A4 => "iso_a4_210x297mm",
                        PaperArg::Letter => "na_letter_8.5x11in",
                        PaperArg::Legal => "na_legal_8.5x14in",
                        PaperArg::Max => "iso_a4_210x297mm",
                    }
                    .to_string()
                }),
                color: !gray,
                job_name: None,
            };
            println!("Printing {} on {}", file.display(), title(d));
            let result = kagaz_core::print::print(d, &file, &req, dry_run, &mut |e| match e {
                PrintEvent::Planned(p) => println!(
                    "  {} at {} dpi, {}, {}{}",
                    kagaz_core::ipp::status::media_name(&p.media),
                    p.dpi,
                    if p.gray { "black and white" } else { "colour" },
                    p.sides.keyword(),
                    if p.copies > 1 {
                        format!(", {} copies", p.copies)
                    } else {
                        String::new()
                    }
                ),
                PrintEvent::Rendering { page, of } => {
                    println!("  rendering page {page} of {of}...")
                }
                PrintEvent::Validated => println!("  the printer accepts the job settings"),
                PrintEvent::Sending { bytes } => {
                    println!("  sending {}...", human_size(bytes as u64))
                }
                PrintEvent::Submitted { job_id, state } => println!("  job {job_id} {state}"),
            })?;
            match result {
                None => println!("Dry run: nothing was sent."),
                Some((id, _)) => {
                    println!("Sent as job {id}. `kagaz status {device}` shows the queue.")
                }
            }
            Ok(())
        }
        Command::Identify { device, timeout } => {
            let devices = discover(&DiscoverOptions {
                timeout: Duration::from_secs(timeout),
                ..Default::default()
            })?;
            let d = select_device(&devices, &device)?;
            kagaz_core::ipp::status::identify(d)?;
            println!("{} should be flashing its display now.", title(d));
            Ok(())
        }
    }
}

fn print_status(d: &Device, s: &PrinterStatus) {
    let model = s
        .make_and_model
        .clone()
        .or_else(|| s.info.clone())
        .unwrap_or_else(|| title(d));
    let addr = d
        .addresses
        .first()
        .map(|a| format!(" at {a}"))
        .unwrap_or_default();
    println!("{model} ({}){addr}", s.name);
    if let Some(loc) = &s.location {
        println!("  location   {loc}");
    }
    let mut state = s.state.label().to_string();
    if !s.accepting_jobs {
        state.push_str(", not accepting jobs");
    }
    match s.queued_jobs {
        0 => state.push_str(", nothing queued"),
        1 => state.push_str(", 1 job queued"),
        n => state.push_str(&format!(", {n} jobs queued")),
    }
    println!("  state      {state}");
    for r in &s.reasons {
        let sev = match r.severity.as_str() {
            "error" => "problem",
            "warning" => "warning",
            "report" => "note",
            _ => "note",
        };
        println!("  {sev:<10} {}", r.text);
    }
    if let Some(m) = &s.message {
        println!("  message    {m}");
    }
    for sup in &s.supplies {
        let level = match sup.level {
            Some(l) => format!("{l}%"),
            None => "level unknown".to_string(),
        };
        let low = sup
            .low_at
            .map(|l| format!(" (low at {l}%)"))
            .unwrap_or_default();
        println!("  {:<10} {} {level}{low}", sup.kind, sup.name);
    }
    if !s.media_ready.is_empty() {
        let names: Vec<String> = s
            .media_ready
            .iter()
            .map(|m| kagaz_core::ipp::status::media_name(m))
            .collect();
        println!("  paper      {} loaded", names.join(", "));
    }
    let mut prints = vec![if s.color { "colour" } else { "black and white" }.to_string()];
    if let Some(ppm) = s.pages_per_minute {
        prints.push(format!("{ppm} pages/min"));
    }
    if s.duplex {
        prints.push("two-sided".into());
    }
    if let Some(dpi) = s.max_dpi {
        prints.push(format!("up to {dpi} dpi"));
    }
    println!("  prints     {}", prints.join(", "));
    let formats: Vec<&str> = s
        .document_formats
        .iter()
        .map(|f| match f.as_str() {
            "image/pwg-raster" => "PWG Raster (IPP Everywhere)",
            "image/urf" => "Apple Raster (AirPrint)",
            "application/pdf" => "PDF",
            "application/postscript" => "PostScript",
            "application/octet-stream" => "raw",
            "image/jpeg" => "JPEG",
            other => other,
        })
        .collect();
    if !formats.is_empty() {
        println!("  accepts    {}", formats.join(", "));
    }
    if let Some(up) = s.up_time {
        let (h, m) = (up / 3600, up % 3600 / 60);
        println!(
            "  on for     {}",
            if h > 0 {
                format!("{h} h {m} min")
            } else {
                format!("{m} min")
            }
        );
    }
    for j in &s.jobs {
        println!(
            "  job {:<6} {} ({}, {}{})",
            j.id,
            if j.name.is_empty() {
                "untitled"
            } else {
                &j.name
            },
            j.state,
            j.user,
            j.impressions_completed
                .map(|n| format!(", {n} pages done"))
                .unwrap_or_default()
        );
    }
    if let Some(url) = &s.more_info {
        println!("  web page   {url}");
    }
}

/// Pick one device by what the user typed, or explain why that failed.
fn select_device<'a>(devices: &'a [Device], selector: &str) -> Result<&'a Device> {
    match kagaz_core::find(devices, selector) {
        Ok(d) => Ok(d),
        Err(SelectError::Ambiguous { candidates, .. }) => {
            eprintln!("\"{selector}\" matches several devices:");
            for i in candidates {
                eprintln!("  {i}. {}", title(&devices[i - 1]));
            }
            anyhow::bail!("pick one by number")
        }
        Err(e) => {
            if devices.is_empty() {
                anyhow::bail!("{e}: nothing was found at all (see `kagaz discover`)");
            }
            eprintln!("Found:");
            for (i, d) in devices.iter().enumerate() {
                eprintln!("  {}. {}", i + 1, title(d));
            }
            anyhow::bail!("{e}")
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
