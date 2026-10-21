//! Driverless scanning. The protocol modules (`wsd`, later `escl`) turn a
//! `ScanRequest` into pages; the `output` module turns pages into files.

pub mod button_settings;
pub mod escl;
pub mod id;
pub mod multipart;
pub mod sane;
pub mod vendor;
pub mod wia;
pub mod wsd;

use crate::device::{Device, Protocol};
use serde::{Deserialize, Serialize};

/// Where the paper is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// Feeder if it has paper, else the glass.
    Auto,
    Glass,
    Feeder,
    FeederDuplex,
}

/// Colour mode the user asked for. The device may only offer colour; the
/// output stage converts in that case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMode {
    Color,
    Gray,
    BlackWhite,
}

/// Paper size to scan, in thousandths of an inch (the WSD unit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Paper {
    A4,
    Letter,
    Legal,
    /// The whole glass or the longest the feeder takes.
    Max,
}

impl Paper {
    /// (width, height) in 1/1000 inch.
    pub fn size_mils(self) -> Option<(u32, u32)> {
        match self {
            Paper::A4 => Some((8268, 11693)),
            Paper::Letter => Some((8500, 11000)),
            Paper::Legal => Some((8500, 14000)),
            Paper::Max => None,
        }
    }
}

/// Which software talks to the scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Engine {
    /// Kagaz itself, over eSCL or WSD.
    #[default]
    Driverless,
    /// The installed vendor driver: through SANE on Linux and macOS,
    /// through WIA on Windows.
    Driver,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRequest {
    pub source: Source,
    pub dpi: u32,
    pub color: ColorMode,
    pub paper: Paper,
    #[serde(default)]
    pub engine: Engine,
    /// Only the top half of the page, the half by the glass's corner mark:
    /// one side of an ID card or a passport page, as a printer's ID copy
    /// scans it (see [`id`]). No effect with [`Paper::Max`].
    #[serde(default)]
    pub half_page: bool,
}

impl ScanRequest {
    /// What to scan from the glass's corner, in 1/1000 inch: the paper, or
    /// its top half. None: as much as the device takes.
    pub fn area_mils(&self) -> Option<(u32, u32)> {
        let (w, h) = self.paper.size_mils()?;
        Some(if self.half_page { (w, h / 2) } else { (w, h) })
    }

    /// How big one page is once opened, in bytes: its pixels times colour
    /// channels. The whole glass counts as Legal, the largest common glass.
    pub fn image_bytes(&self) -> u64 {
        let (w, h) = self.area_mils().unwrap_or((8500, 14000));
        let px = |mils: u32| mils as u64 * self.dpi as u64 / 1000;
        let channels = if self.color == ColorMode::Color { 3 } else { 1 };
        px(w) * px(h) * channels
    }

    /// The highest common resolution whose page stays within [`MAX_IMAGE_BYTES`].
    pub fn largest_dpi(&self) -> u32 {
        [4800, 2400, 1200, 600, 400, 300, 200, 150, 100, 75]
            .into_iter()
            .find(|&dpi| ScanRequest { dpi, ..self.clone() }.image_bytes() <= MAX_IMAGE_BYTES)
            .unwrap_or(75)
    }
}

/// The largest page Kagaz scans, once opened (see [`ScanRequest::image_bytes`]).
/// Beyond it a scanner's driver runs out of memory (Brother's did at 9600
/// dpi), and so would Kagaz putting the pages together; a phone has less.
pub const MAX_IMAGE_BYTES: u64 = if cfg!(any(target_os = "android", target_os = "ios")) {
    200_000_000
} else {
    700_000_000
};

impl Default for ScanRequest {
    fn default() -> Self {
        Self {
            source: Source::Auto,
            dpi: 300,
            color: ColorMode::Color,
            paper: Paper::A4,
            engine: Engine::Driverless,
            half_page: false,
        }
    }
}

/// One scanned page as the device delivered it.
#[derive(Debug, Clone)]
pub struct Page {
    pub data: Vec<u8>,
    /// "image/jpeg" or "image/png".
    pub mime: &'static str,
    pub dpi: u32,
    /// Colour as scanned (the device may not offer what was asked for).
    pub color: ColorMode,
}

/// Something worth telling the user while a scan runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// About to ask the device for a job on this source.
    Starting { source: Source, dpi: u32 },
    /// The feeder had no paper; falling back to the glass.
    FeederEmpty,
    /// A page arrived (1-based number, size in bytes).
    Page { number: usize, bytes: usize },
    /// The device could not give what was asked and this was used instead.
    Substituted { note: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("{0} does not advertise driverless scanning (eSCL or WSD)")]
    NotSupported(String),
    #[error("cannot reach the scanner: {0}")]
    Transport(String),
    #[error("the scanner answered with something unexpected: {0}")]
    Protocol(String),
    #[error("the scanner refused: {0}")]
    Refused(String),
    #[error("the feeder is empty")]
    FeederEmpty,
    #[error("the scanner is busy or not ready: {0}")]
    Busy(String),
    #[error("at {dpi} dpi this would be about {size} once opened, more than this device can hold; choose {largest} dpi or less (300 dpi is plenty for documents and ID cards)")]
    TooBig { dpi: u32, size: String, largest: u32 },
}

/// What one engine offers, in the terms the window and the CLI present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineCaps {
    pub engine: Engine,
    /// "WSD", "eSCL", or the SANE backend name such as "brother4".
    pub via: String,
    /// Resolutions the device confirmed (driverless: advertised plus those it validated when asked).
    pub resolutions: Vec<u32>,
    pub glass: bool,
    pub feeder: bool,
    pub duplex: bool,
    /// Colour modes the engine itself produces (Kagaz converts to the others).
    pub colors: Vec<String>,
}

/// Everything available for a device: the driverless engine when the device
/// speaks eSCL or WSD, the driver engine when an installed backend knows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub driverless: Option<EngineCaps>,
    pub driver: Option<EngineCaps>,
}

/// What the driverless engine offers; `None` when the device has no eSCL or WSD.
pub fn driverless_capabilities(device: &Device) -> Result<Option<EngineCaps>, ScanError> {
    if let Some(url) = device
        .services
        .iter()
        .find(|s| s.protocol == Protocol::WsdScan)
        .and_then(|s| s.endpoint.clone())
    {
        let w = wsd::WsdScanner::connect(&url)?;
        let mut resolutions: Vec<u32> = w
            .caps
            .platen
            .iter()
            .chain(w.caps.feeder.iter())
            .flat_map(|c| c.resolutions.iter().copied())
            .collect();
        resolutions.sort_unstable();
        resolutions.dedup();
        let mut colors: Vec<String> = w
            .caps
            .platen
            .iter()
            .chain(w.caps.feeder.iter())
            .flat_map(|c| c.colors.iter().cloned())
            .collect();
        colors.sort();
        colors.dedup();
        return Ok(Some(EngineCaps {
            engine: Engine::Driverless,
            via: "WSD".into(),
            resolutions,
            glass: w.caps.platen.is_some(),
            feeder: w.caps.feeder.is_some(),
            duplex: w.caps.duplex,
            colors,
        }));
    }
    if let Some(base) = escl_base(device) {
        let e = escl::EsclScanner::connect(&base)?;
        let mut resolutions: Vec<u32> = e
            .caps
            .platen
            .iter()
            .chain(e.caps.feeder.iter())
            .flat_map(|c| c.resolutions.iter().copied())
            .collect();
        resolutions.sort_unstable();
        resolutions.dedup();
        let mut colors: Vec<String> = e
            .caps
            .platen
            .iter()
            .chain(e.caps.feeder.iter())
            .flat_map(|c| c.colors.iter().cloned())
            .collect();
        colors.sort();
        colors.dedup();
        return Ok(Some(EngineCaps {
            engine: Engine::Driverless,
            via: "eSCL".into(),
            resolutions,
            glass: e.caps.platen.is_some(),
            feeder: e.caps.feeder.is_some(),
            duplex: e.caps.duplex,
            colors,
        }));
    }
    Ok(None)
}

/// What the installed vendor driver offers for this device (SANE, or WIA on
/// Windows); `None` when none knows it.
pub fn driver_capabilities(device: &Device) -> Result<Option<EngineCaps>, ScanError> {
    if wia::available() {
        let devices = wia::list_devices()?;
        let Some(dev) = wia::device_for(&devices, device) else {
            return Ok(None);
        };
        let caps = wia::capabilities(&dev.id)?;
        return Ok(Some(EngineCaps {
            engine: Engine::Driver,
            via: format!("WIA ({})", dev.name),
            resolutions: caps.resolutions,
            glass: caps.flatbed,
            feeder: caps.feeder,
            duplex: caps.duplex,
            colors: caps.colors,
        }));
    }
    if !sane::available() {
        return Ok(None);
    }
    let devices = sane::list_devices()?;
    let Some(dev) = sane::device_for(&devices, device) else {
        return Ok(None);
    };
    let caps = sane::capabilities(&dev.name)?;
    Ok(Some(EngineCaps {
        engine: Engine::Driver,
        via: dev.name.split(':').next().unwrap_or("sane").to_string(),
        resolutions: caps.resolutions.clone(),
        glass: caps.sources.is_empty()
            || caps.sources.iter().any(|s| {
                !s.to_ascii_lowercase().contains("feeder")
                    && !s.to_ascii_lowercase().contains("adf")
            }),
        feeder: caps.feeder(),
        duplex: caps.duplex(),
        colors: caps.modes.clone(),
    }))
}

/// Ask `device` what it can do with each engine.
pub fn capabilities(device: &Device) -> Result<Capabilities, ScanError> {
    let driverless = driverless_capabilities(device)?;
    let driver = driver_capabilities(device).unwrap_or(None);
    if driverless.is_none() && driver.is_none() {
        return Err(ScanError::NotSupported(if device.name.is_empty() {
            "this device".to_string()
        } else {
            device.name.clone()
        }));
    }
    Ok(Capabilities { driverless, driver })
}

/// The eSCL base URL for a device, when it advertises eSCL over plain HTTP.
fn escl_base(device: &Device) -> Option<String> {
    let svc = device
        .services
        .iter()
        .find(|s| s.protocol == Protocol::Escl)?;
    let ip = device
        .addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or(device.addresses.first())?;
    let host = match ip {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
    };
    let path = svc
        .endpoint
        .as_deref()
        .map(|p| p.trim_matches('/'))
        .filter(|p| !p.is_empty())
        .unwrap_or("eSCL");
    Some(format!("http://{host}:{}/{path}", svc.port.unwrap_or(80)))
}

/// Scan from `device` using whichever driverless protocol it offers,
/// reporting progress through `on_event`.
pub fn scan(
    device: &Device,
    req: &ScanRequest,
    on_event: &mut dyn FnMut(Event),
) -> Result<Vec<Page>, ScanError> {
    // Say so now, not after minutes of scanning and a driver out of memory.
    if req.image_bytes() > MAX_IMAGE_BYTES {
        return Err(ScanError::TooBig {
            dpi: req.dpi,
            size: crate::output::human_size(req.image_bytes()),
            largest: req.largest_dpi(),
        });
    }
    if req.engine == Engine::Driver && cfg!(windows) {
        let devices = wia::list_devices()?;
        let dev = wia::device_for(&devices, device).ok_or_else(|| {
            ScanError::NotSupported(format!(
                "Windows has no scanner driver installed for {}",
                if device.name.is_empty() {
                    "this device"
                } else {
                    &device.name
                }
            ))
        })?;
        let caps = wia::capabilities(&dev.id)?;
        return wia::scan(&dev.id, &caps, req, on_event);
    }
    if req.engine == Engine::Driver {
        if !sane::available() {
            return Err(ScanError::NotSupported(
                "the driver engine needs SANE's scanimage, which is not installed".into(),
            ));
        }
        let devices = sane::list_devices()?;
        let dev = sane::device_for(&devices, device).ok_or_else(|| {
            ScanError::NotSupported(format!(
                "no installed driver knows {}; `kagaz driver` installs the vendor's",
                if device.name.is_empty() {
                    "this device"
                } else {
                    &device.name
                }
            ))
        })?;
        let caps = sane::capabilities(&dev.name)?;
        return sane::scan(&dev.name, &caps, req, on_event);
    }
    // WSD first while it is the protocol verified on real hardware; eSCL is
    // written from the specification and takes over once it has been
    // checked against a device.
    if let Some(svc) = device
        .services
        .iter()
        .find(|s| s.protocol == Protocol::WsdScan)
    {
        let url = svc
            .endpoint
            .clone()
            .ok_or_else(|| ScanError::Protocol("WSD scan service has no address".into()))?;
        let scanner = wsd::WsdScanner::connect(&url)?;
        return scanner.scan(req, on_event);
    }
    if let Some(svc) = device
        .services
        .iter()
        .find(|s| s.protocol == Protocol::Escl)
    {
        let ip = device
            .addresses
            .iter()
            .find(|a| a.is_ipv4())
            .or(device.addresses.first())
            .ok_or_else(|| ScanError::Protocol("eSCL service has no address".into()))?;
        let host = match ip {
            std::net::IpAddr::V4(v4) => v4.to_string(),
            std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        };
        let path = svc
            .endpoint
            .as_deref()
            .map(|p| p.trim_matches('/'))
            .filter(|p| !p.is_empty())
            .unwrap_or("eSCL");
        let base = format!("http://{host}:{}/{path}", svc.port.unwrap_or(80));
        let scanner = escl::EsclScanner::connect(&base)?;
        return scanner.scan(req, on_event);
    }
    if device.has_protocol(Protocol::Escls) {
        return Err(ScanError::NotSupported(
            "this scanner only offers eSCL over TLS, which Kagaz cannot speak yet".into(),
        ));
    }
    Err(ScanError::NotSupported(if device.name.is_empty() {
        "this device".to_string()
    } else {
        device.name.clone()
    }))
}

#[cfg(test)]
mod size_tests {
    use super::*;

    fn half(dpi: u32, color: ColorMode) -> ScanRequest {
        ScanRequest { dpi, color, half_page: true, ..Default::default() }
    }

    #[test]
    fn a_page_too_big_to_hold_is_refused_before_scanning() {
        // Half an A4 in colour at 9600 dpi: about 13 GB once opened.
        let req = half(9600, ColorMode::Color);
        assert!(req.image_bytes() > 13_000_000_000);
        match scan(&Device::default(), &req, &mut |_| {}) {
            // 1200 dpi is about 209 MB: within a computer's limit, not a phone's.
            Err(ScanError::TooBig { largest, .. }) => {
                assert_eq!(largest, if MAX_IMAGE_BYTES >= 300_000_000 { 1200 } else { 600 })
            }
            other => panic!("expected TooBig, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_scans_stay_within_the_limit() {
        assert!(half(300, ColorMode::Color).image_bytes() < 20_000_000);
        assert!(ScanRequest { dpi: 600, ..Default::default() }.image_bytes() <= MAX_IMAGE_BYTES);
        assert_eq!(ScanRequest { dpi: 300, color: ColorMode::Gray, ..Default::default() }.largest_dpi(), 2400);
    }
}
