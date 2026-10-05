//! Driverless scanning. The protocol modules (`wsd`, later `escl`) turn a
//! `ScanRequest` into pages; the `output` module turns pages into files.

pub mod button_settings;
pub mod escl;
pub mod multipart;
pub mod vendor;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRequest {
    pub source: Source,
    pub dpi: u32,
    pub color: ColorMode,
    pub paper: Paper,
}

impl Default for ScanRequest {
    fn default() -> Self {
        Self {
            source: Source::Auto,
            dpi: 300,
            color: ColorMode::Color,
            paper: Paper::A4,
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
}

/// Scan from `device` using whichever driverless protocol it offers,
/// reporting progress through `on_event`.
pub fn scan(
    device: &Device,
    req: &ScanRequest,
    on_event: &mut dyn FnMut(Event),
) -> Result<Vec<Page>, ScanError> {
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
