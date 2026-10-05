//! Scanning through an installed vendor driver, via SANE's `scanimage`
//! (Linux and macOS). This is the "driver" engine: the official backend
//! (for Brother, brscan4) does the talking, Kagaz only chooses the options
//! the backend itself lists and files the result. Nothing here is
//! vendor-specific beyond matching the device.

use super::{ColorMode, Event, Page, Paper, ScanError, ScanRequest, Source};
use crate::Device;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A SANE device as `scanimage -L` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaneDevice {
    /// e.g. `brother4:net1;dev0`
    pub name: String,
    /// e.g. "Brother DCP-L2540DW DCP-L2540DW"
    pub description: String,
}

/// What the backend offers, parsed from `scanimage -A`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SaneCaps {
    pub device: String,
    pub resolutions: Vec<u32>,
    pub modes: Vec<String>,
    pub sources: Vec<String>,
    /// Scan area limits in mm, when the backend gives them.
    pub max_width_mm: Option<f32>,
    pub max_height_mm: Option<f32>,
}

impl SaneCaps {
    pub fn feeder(&self) -> bool {
        self.sources.iter().any(|s| is_feeder(s))
    }
    pub fn duplex(&self) -> bool {
        self.sources
            .iter()
            .any(|s| s.to_ascii_lowercase().contains("duplex"))
    }
}

fn is_feeder(source: &str) -> bool {
    let s = source.to_ascii_lowercase();
    s.contains("feeder") || s.contains("adf")
}

pub fn available() -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("scanimage").is_file()))
        .unwrap_or(false)
}

/// Parse `scanimage -L` output.
pub fn parse_device_list(text: &str) -> Vec<SaneDevice> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("device `")?;
            let (name, desc) = rest.split_once("' is a ")?;
            Some(SaneDevice {
                name: name.to_string(),
                description: desc.trim().to_string(),
            })
        })
        .collect()
}

pub fn list_devices() -> Result<Vec<SaneDevice>, ScanError> {
    let out = Command::new("scanimage")
        .arg("-L")
        .output()
        .map_err(|e| ScanError::Transport(format!("cannot run scanimage: {e}")))?;
    Ok(parse_device_list(&String::from_utf8_lossy(&out.stdout)))
}

/// The vendor-backend device for `device`, if the installed driver knows it:
/// a non-airscan SANE device whose description names the same model.
pub fn device_for<'a>(sane: &'a [SaneDevice], device: &Device) -> Option<&'a SaneDevice> {
    let model = device
        .model
        .as_deref()
        .or(Some(device.name.as_str()))
        .map(|m| crate::drivers::model_key(device.manufacturer.as_deref(), m))?;
    if model.is_empty() {
        return None;
    }
    let ip = device.addresses.first().map(|a| a.to_string());
    sane.iter()
        .filter(|d| {
            !d.name.starts_with("airscan:")
                && !d.name.starts_with("escl:")
                && !d.name.starts_with("wsd:")
        })
        .find(|d| {
            let desc_key: String = d
                .description
                .to_ascii_uppercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            desc_key.contains(&model) || ip.as_deref().is_some_and(|ip| d.description.contains(ip))
        })
}

/// Parse `scanimage -A` output for the options Kagaz uses.
pub fn parse_options(device: &str, text: &str) -> SaneCaps {
    let mut caps = SaneCaps {
        device: device.to_string(),
        ..Default::default()
    };
    let list = |rest: &str| -> Vec<String> {
        // "A|B|C [default]" -> A, B, C
        let spec = rest.split(" [").next().unwrap_or(rest).trim();
        spec.split('|')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let max_of = |rest: &str| -> Option<f32> {
        // "0..211.9mm (in steps of 0.1) [211.881]"
        let range = rest.split("mm").next()?;
        range.split("..").nth(1)?.trim().parse().ok()
    };
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("--resolution ") {
            caps.resolutions = list(rest)
                .iter()
                .filter_map(|s| s.trim_end_matches("dpi").parse().ok())
                .collect();
        } else if let Some(rest) = l.strip_prefix("--mode ") {
            caps.modes = list(rest);
        } else if let Some(rest) = l.strip_prefix("--source ") {
            caps.sources = list(rest);
        } else if let Some(rest) = l.strip_prefix("-x ") {
            caps.max_width_mm = max_of(rest);
        } else if let Some(rest) = l.strip_prefix("-y ") {
            caps.max_height_mm = max_of(rest);
        }
    }
    caps
}

pub fn capabilities(device: &str) -> Result<SaneCaps, ScanError> {
    let out = Command::new("scanimage")
        .args(["-d", device, "-A"])
        .output()
        .map_err(|e| ScanError::Transport(format!("cannot run scanimage: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let caps = parse_options(device, &text);
    if caps.resolutions.is_empty() {
        return Err(ScanError::Protocol(format!(
            "scanimage listed no options for {device}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(caps)
}

/// Pick the backend's own names for what was asked.
pub fn choose(
    caps: &SaneCaps,
    req: &ScanRequest,
    source: Source,
) -> Result<(Vec<String>, Vec<String>), ScanError> {
    let mut notes = Vec::new();
    let mut args: Vec<String> = Vec::new();
    if !caps.resolutions.contains(&req.dpi) {
        return Err(ScanError::Refused(format!(
            "{} dpi is not offered by the driver; it offers {}",
            req.dpi,
            caps.resolutions
                .iter()
                .map(|r| r.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    args.extend(["--resolution".into(), req.dpi.to_string()]);

    let want = |m: &str| -> bool {
        let m = m.to_ascii_lowercase();
        match req.color {
            ColorMode::Color => m.contains("color") || m.contains("colour"),
            ColorMode::Gray => m.contains("gray") || m.contains("grey"),
            ColorMode::BlackWhite => m.contains("black") || m.contains("lineart"),
        }
    };
    // Prefer plain modes ("True Gray") over dithered ones ("Gray[Error Diffusion]").
    let mode = caps
        .modes
        .iter()
        .filter(|m| want(m))
        .min_by_key(|m| m.contains('[') as u8)
        .cloned()
        .or_else(|| {
            caps.modes
                .iter()
                .find(|m| m.to_ascii_lowercase().contains("color"))
                .cloned()
        });
    if let Some(m) = mode {
        if !want(&m) {
            notes.push(format!(
                "the driver has no matching colour mode; scanning in \"{m}\" and converting"
            ));
        }
        args.extend(["--mode".into(), m]);
    }

    let src = match source {
        Source::Glass => caps.sources.iter().find(|s| !is_feeder(s)),
        Source::FeederDuplex => caps
            .sources
            .iter()
            .find(|s| is_feeder(s) && s.to_ascii_lowercase().contains("duplex")),
        _ => caps.sources.iter().find(|s| is_feeder(s)),
    };
    match src {
        Some(s) => args.extend(["--source".into(), s.clone()]),
        None if caps.sources.is_empty() => {}
        None => {
            return Err(ScanError::Refused(format!(
                "the driver offers no source for {source:?}; it offers {}",
                caps.sources.join(", ")
            )))
        }
    }

    if let Some((w, h)) = req
        .paper
        .size_mils()
        .map(|(w, h)| (w as f32 * 25.4 / 1000.0, h as f32 * 25.4 / 1000.0))
    {
        let w = caps.max_width_mm.map(|m| w.min(m)).unwrap_or(w);
        let h = caps.max_height_mm.map(|m| h.min(m)).unwrap_or(h);
        args.extend([
            "-x".into(),
            format!("{w:.1}"),
            "-y".into(),
            format!("{h:.1}"),
        ]);
    } else if req.paper == Paper::Max {
        if let (Some(w), Some(h)) = (caps.max_width_mm, caps.max_height_mm) {
            args.extend([
                "-x".into(),
                format!("{w:.1}"),
                "-y".into(),
                format!("{h:.1}"),
            ]);
        }
    }
    Ok((args, notes))
}

/// Run the scan: one PNG from the glass, or a batch of PNGs from the feeder.
pub fn scan(
    device: &str,
    caps: &SaneCaps,
    req: &ScanRequest,
    on_event: &mut dyn FnMut(Event),
) -> Result<Vec<Page>, ScanError> {
    let source = match req.source {
        Source::Auto => {
            if caps.feeder() {
                Source::Feeder
            } else {
                Source::Glass
            }
        }
        s => s,
    };
    let work = std::env::temp_dir().join(format!("kagaz-sane-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| ScanError::Transport(e.to_string()))?;
    let result = run(device, caps, req, source, &work, on_event);
    let _ = std::fs::remove_dir_all(&work);
    match result {
        // Auto with an empty feeder: the backend says so; fall back to the glass.
        Err(ScanError::FeederEmpty) if req.source == Source::Auto => {
            on_event(Event::FeederEmpty);
            let work = std::env::temp_dir().join(format!("kagaz-sane-{}-g", std::process::id()));
            std::fs::create_dir_all(&work).map_err(|e| ScanError::Transport(e.to_string()))?;
            let r = run(device, caps, req, Source::Glass, &work, on_event);
            let _ = std::fs::remove_dir_all(&work);
            r
        }
        other => other,
    }
}

fn run(
    device: &str,
    caps: &SaneCaps,
    req: &ScanRequest,
    source: Source,
    work: &Path,
    on_event: &mut dyn FnMut(Event),
) -> Result<Vec<Page>, ScanError> {
    let (args, notes) = choose(caps, req, source)?;
    for n in notes {
        on_event(Event::Substituted { note: n });
    }
    on_event(Event::Starting {
        source,
        dpi: req.dpi,
    });
    let mut cmd = Command::new("scanimage");
    cmd.args(["-d", device, "--format=png"]).args(&args);
    if source == Source::Glass {
        cmd.args(["-o", &work.join("page001.png").display().to_string()]);
    } else {
        cmd.arg(format!("--batch={}", work.join("page%03d.png").display()));
    }
    let out = cmd
        .output()
        .map_err(|e| ScanError::Transport(format!("cannot run scanimage: {e}")))?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(work)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files.retain(|p| std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false));
    if files.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
        return Err(
            if err.contains("no documents") || err.contains("document feeder out of documents") {
                ScanError::FeederEmpty
            } else {
                ScanError::Refused(format!(
                    "scanimage produced no pages: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            },
        );
    }
    let mut pages = Vec::with_capacity(files.len());
    for f in &files {
        let data = std::fs::read(f).map_err(|e| ScanError::Transport(e.to_string()))?;
        pages.push(Page {
            data,
            mime: "image/png",
            dpi: req.dpi,
            color: req.color,
        });
        on_event(Event::Page {
            number: pages.len(),
            bytes: pages.last().map(|p| p.data.len()).unwrap_or(0),
        });
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "device `brother4:net1;dev0' is a Brother DCP-L2540DW DCP-L2540DW\ndevice `airscan:w0:Brother DCP-L2540DW series' is a WSD Brother DCP-L2540DW series ip=198.51.100.167\n";
    const OPTIONS: &str = "All options specific to device `brother4:net1;dev0':\n    --mode Black & White|Gray[Error Diffusion]|True Gray|24bit Color[Fast] [24bit Color[Fast]]\n        Select the scan mode\n    --resolution 100|150|200|300|400|600|1200|2400|4800|9600dpi [200]\n        Sets the resolution of the scanned image.\n    --source FlatBed|Automatic Document Feeder(left aligned)|Automatic Document Feeder(centrally aligned) [Automatic Document Feeder(left aligned)]\n    -x 0..211.9mm (in steps of 0.0999908) [211.881]\n    -y 0..355.6mm (in steps of 0.0999908) [355.567]\n";

    #[test]
    fn finds_the_vendor_backend_not_airscan() {
        let devs = parse_device_list(LIST);
        assert_eq!(devs.len(), 2);
        let d = Device {
            name: "Brother DCP-L2540DW series".into(),
            manufacturer: Some("Brother".into()),
            model: Some("DCP-L2540DW series".into()),
            ..Device::default()
        };
        assert_eq!(device_for(&devs, &d).unwrap().name, "brother4:net1;dev0");
        let other = Device {
            name: "HP LaserJet".into(),
            manufacturer: Some("HP".into()),
            ..Device::default()
        };
        assert!(device_for(&devs, &other).is_none());
    }

    #[test]
    fn parses_options_and_chooses_backend_names() {
        let caps = parse_options("brother4:net1;dev0", OPTIONS);
        assert_eq!(
            caps.resolutions,
            vec![100, 150, 200, 300, 400, 600, 1200, 2400, 4800, 9600]
        );
        assert_eq!(caps.modes.len(), 4);
        assert!(caps.feeder());
        assert!(!caps.duplex());
        assert_eq!(caps.max_width_mm, Some(211.9));
        let req = ScanRequest {
            dpi: 600,
            color: ColorMode::Gray,
            ..Default::default()
        };
        let (args, notes) = choose(&caps, &req, Source::Glass).unwrap();
        assert_eq!(
            args,
            vec![
                "--resolution",
                "600",
                "--mode",
                "True Gray",
                "--source",
                "FlatBed",
                "-x",
                "210.0",
                "-y",
                "297.0"
            ]
        );
        assert!(notes.is_empty());
        let (args, _) = choose(&caps, &ScanRequest::default(), Source::Feeder).unwrap();
        assert!(args.contains(&"Automatic Document Feeder(left aligned)".to_string()));
        assert!(args.contains(&"24bit Color[Fast]".to_string()));
        let bad = ScanRequest {
            dpi: 250,
            ..Default::default()
        };
        assert!(matches!(
            choose(&caps, &bad, Source::Glass),
            Err(ScanError::Refused(_))
        ));
    }
}
