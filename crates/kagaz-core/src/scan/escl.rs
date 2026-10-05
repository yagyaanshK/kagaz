//! eSCL (the Mopria/AirScan scanning protocol): plain HTTP under the
//! device's `rs` path, usually `/eSCL`.
//!
//! 1. `GET ScannerCapabilities` for sources, resolutions, colours, formats,
//! 2. `GET ScannerStatus` for idle/busy and whether the feeder has paper,
//! 3. `POST ScanJobs` with ScanSettings; `201 Created` with the job URL in `Location`,
//! 4. `GET <job>/NextDocument` once per page: `200` carries the image,
//!    `404` (or `410`) means the job has no more pages, `503` means wait,
//! 5. `DELETE <job>` when done.
//!
//! Written from the eSCL specification and sane-airscan's backend; this
//! module has NOT yet been run against a real eSCL device (the test printer
//! here only speaks WSD). Fixtures are hand-built from the specification.

use super::{ColorMode, Event, Page, ScanError, ScanRequest, Source};
use roxmltree::Document;
use std::io::Read;
use std::time::Duration;

/// Capabilities of one input source. Sizes are in 1/300 inch, as in eSCL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceCaps {
    pub resolutions: Vec<u32>,
    /// "RGB24", "Grayscale8", "BlackAndWhite1", ...
    pub colors: Vec<String>,
    /// MIME types: "image/jpeg", "image/png", "application/pdf", ...
    pub formats: Vec<String>,
    pub max_width: u32,
    pub max_height: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub make_and_model: Option<String>,
    pub platen: Option<SourceCaps>,
    pub feeder: Option<SourceCaps>,
    pub duplex: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// "Idle", "Processing", "Stopped", "Down", "Testing".
    pub state: String,
    /// "ScannerAdfLoaded", "ScannerAdfEmpty", "ScannerAdfProcessing", "ScannerAdfJam", ...
    pub adf_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobParams {
    pub source: Source,
    pub dpi: u32,
    pub color: String,
    pub format: String,
    /// Scan region in 1/300 inch.
    pub width: u32,
    pub height: u32,
}

pub struct EsclScanner {
    base: String,
    agent: ureq::Agent,
    pub caps: Capabilities,
    pub status: Status,
}

impl EsclScanner {
    /// Read the capabilities of the scanner at `base` (e.g. `http://192.168.1.20:80/eSCL`).
    pub fn connect(base: &str) -> Result<EsclScanner, ScanError> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(180))
            .build();
        let mut scanner = EsclScanner {
            base: base.trim_end_matches('/').to_string(),
            agent,
            caps: Capabilities::default(),
            status: Status::default(),
        };
        let (status, _, body) = scanner.get(&format!("{}/ScannerCapabilities", scanner.base))?;
        if status != 200 {
            return Err(ScanError::Protocol(format!(
                "ScannerCapabilities answered HTTP {status}"
            )));
        }
        scanner.caps = parse_capabilities(&String::from_utf8_lossy(&body))?;
        scanner.status = scanner.fetch_status().unwrap_or_default();
        Ok(scanner)
    }

    pub fn fetch_status(&self) -> Result<Status, ScanError> {
        let (status, _, body) = self.get(&format!("{}/ScannerStatus", self.base))?;
        if status != 200 {
            return Err(ScanError::Protocol(format!(
                "ScannerStatus answered HTTP {status}"
            )));
        }
        parse_status(&String::from_utf8_lossy(&body))
    }

    pub fn scan(
        &self,
        req: &ScanRequest,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<Vec<Page>, ScanError> {
        let try_feeder = match req.source {
            Source::Auto => {
                if self.caps.feeder.is_none() {
                    false
                } else {
                    // The feeder says whether it has paper; believe it when it does.
                    match self.fetch_status().ok().and_then(|s| s.adf_state) {
                        Some(ref s) if s == "ScannerAdfLoaded" => true,
                        Some(ref s) if s == "ScannerAdfEmpty" || s == "ScannerAdfProcessing" => {
                            on_event(Event::FeederEmpty);
                            false
                        }
                        _ => true,
                    }
                }
            }
            Source::Feeder | Source::FeederDuplex => {
                if self.caps.feeder.is_none() {
                    return Err(ScanError::Refused("this scanner has no feeder".into()));
                }
                true
            }
            Source::Glass => {
                if self.caps.platen.is_none() {
                    return Err(ScanError::Refused("this scanner has no glass".into()));
                }
                false
            }
        };
        if try_feeder {
            let source = if req.source == Source::FeederDuplex {
                if !self.caps.duplex {
                    return Err(ScanError::Refused(
                        "this feeder does not scan both sides".into(),
                    ));
                }
                Source::FeederDuplex
            } else {
                Source::Feeder
            };
            match self.run_job(req, source, on_event) {
                Ok(pages) => return Ok(pages),
                Err(ScanError::FeederEmpty)
                    if req.source == Source::Auto && self.caps.platen.is_some() =>
                {
                    on_event(Event::FeederEmpty);
                }
                Err(e) => return Err(e),
            }
        }
        if self.caps.platen.is_none() {
            return Err(ScanError::FeederEmpty);
        }
        self.run_job(req, Source::Glass, on_event)
    }

    pub fn params_for(
        &self,
        req: &ScanRequest,
        source: Source,
    ) -> Result<(JobParams, Vec<String>), ScanError> {
        let caps = match source {
            Source::Glass => self.caps.platen.as_ref(),
            _ => self.caps.feeder.as_ref(),
        }
        .ok_or_else(|| ScanError::Refused("that source is not available".into()))?;
        let mut notes = Vec::new();
        let dpi = caps
            .resolutions
            .iter()
            .copied()
            .filter(|r| *r <= req.dpi)
            .max()
            .or_else(|| caps.resolutions.iter().copied().min())
            .unwrap_or(req.dpi);
        if dpi != req.dpi {
            notes.push(format!("{} dpi is not offered; using {dpi} dpi", req.dpi));
        }
        let wanted = match req.color {
            ColorMode::Color => "RGB24",
            ColorMode::Gray => "Grayscale8",
            ColorMode::BlackWhite => "BlackAndWhite1",
        };
        let color = if caps.colors.is_empty() || caps.colors.iter().any(|c| c == wanted) {
            wanted.to_string()
        } else {
            let fallback = caps
                .colors
                .iter()
                .find(|c| c.as_str() == "RGB24")
                .or_else(|| caps.colors.first())
                .cloned()
                .unwrap_or_else(|| "RGB24".to_string());
            notes.push(format!(
                "the scanner only scans in {fallback}; converting afterwards"
            ));
            fallback
        };
        let format = ["image/jpeg", "image/png"]
            .iter()
            .find(|f| caps.formats.is_empty() || caps.formats.iter().any(|have| have == *f))
            .map(|f| f.to_string())
            .ok_or_else(|| {
                ScanError::Protocol(format!(
                    "the scanner offers none of the image formats Kagaz reads (it lists: {})",
                    caps.formats.join(", ")
                ))
            })?;
        // Paper sizes are kept in 1/1000 inch; eSCL wants 1/300 inch.
        let to300 = |mils: u32| (mils as u64 * 300 / 1000) as u32;
        let (width, height) = match req.paper.size_mils() {
            Some((w, h)) => (to300(w), to300(h)),
            None => (caps.max_width, caps.max_height),
        };
        let (width, height) = (
            if caps.max_width > 0 {
                width.min(caps.max_width)
            } else {
                width
            },
            if caps.max_height > 0 {
                height.min(caps.max_height)
            } else {
                height
            },
        );
        Ok((
            JobParams {
                source,
                dpi,
                color,
                format,
                width,
                height,
            },
            notes,
        ))
    }

    fn run_job(
        &self,
        req: &ScanRequest,
        source: Source,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<Vec<Page>, ScanError> {
        let (params, notes) = self.params_for(req, source)?;
        for n in notes {
            on_event(Event::Substituted(n));
        }
        on_event(Event::Starting {
            source,
            dpi: params.dpi,
        });
        let body = scan_settings(&params);
        let response = self
            .agent
            .post(&format!("{}/ScanJobs", self.base))
            .set("Content-Type", "text/xml")
            .send_string(&body);
        let response = match response {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let text = r.into_string().unwrap_or_default();
                return Err(match code {
                    409 => ScanError::FeederEmpty,
                    503 => ScanError::Busy(format!("HTTP 503 {text}")),
                    _ => {
                        ScanError::Refused(format!("ScanJobs answered HTTP {code} {}", text.trim()))
                    }
                });
            }
            Err(ureq::Error::Transport(t)) => return Err(ScanError::Transport(t.to_string())),
        };
        if response.status() != 201 {
            return Err(ScanError::Protocol(format!(
                "ScanJobs answered HTTP {} instead of 201 Created",
                response.status()
            )));
        }
        let location = response
            .header("Location")
            .map(str::to_string)
            .filter(|l| !l.is_empty())
            .ok_or_else(|| ScanError::Protocol("ScanJobs gave no job Location".into()))?;
        let job = job_url(&self.base, &location);

        let mime = if params.format == "image/png" {
            "image/png"
        } else {
            "image/jpeg"
        };
        let color_scanned = match params.color.as_str() {
            "Grayscale8" | "Grayscale16" => ColorMode::Gray,
            "BlackAndWhite1" => ColorMode::BlackWhite,
            _ => ColorMode::Color,
        };
        let mut pages = Vec::new();
        let mut waits = 0;
        loop {
            let (status, _, body) = self.get(&format!("{job}/NextDocument"))?;
            match status {
                200 => {
                    pages.push(Page {
                        data: body,
                        mime,
                        dpi: params.dpi,
                        color: color_scanned,
                    });
                    on_event(Event::Page {
                        number: pages.len(),
                        bytes: pages.last().map(|p| p.data.len()).unwrap_or(0),
                    });
                    if source == Source::Glass {
                        break;
                    }
                }
                404 | 410 => {
                    if pages.is_empty() {
                        let _ = self.delete(&job);
                        return Err(if source == Source::Glass {
                            ScanError::Refused("the scanner ended the job without a page".into())
                        } else {
                            ScanError::FeederEmpty
                        });
                    }
                    break;
                }
                503 => {
                    waits += 1;
                    if waits > 120 {
                        let _ = self.delete(&job);
                        return Err(ScanError::Busy(
                            "the scanner kept saying 'not ready'".into(),
                        ));
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
                other => {
                    let _ = self.delete(&job);
                    return Err(ScanError::Protocol(format!(
                        "NextDocument answered HTTP {other}"
                    )));
                }
            }
        }
        let _ = self.delete(&job);
        Ok(pages)
    }

    fn get(&self, url: &str) -> Result<(u16, String, Vec<u8>), ScanError> {
        let response = match self.agent.get(url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => return Err(ScanError::Transport(t.to_string())),
        };
        let status = response.status();
        let content_type = response
            .header("Content-Type")
            .unwrap_or_default()
            .to_string();
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| ScanError::Transport(e.to_string()))?;
        Ok((status, content_type, bytes))
    }

    fn delete(&self, url: &str) -> Result<(), ScanError> {
        match self.agent.delete(url).call() {
            Ok(_) | Err(ureq::Error::Status(_, _)) => Ok(()),
            Err(ureq::Error::Transport(t)) => Err(ScanError::Transport(t.to_string())),
        }
    }
}

/// The job URL from a `Location` header. Hostnames in it are not trusted
/// (devices often get their own wrong), so only its path is used.
pub fn job_url(base: &str, location: &str) -> String {
    let origin_end = base
        .find("://")
        .and_then(|i| base[i + 3..].find('/').map(|j| i + 3 + j))
        .unwrap_or(base.len());
    let origin = &base[..origin_end];
    let path = if let Some(i) = location.find("://") {
        match location[i + 3..].find('/') {
            Some(j) => &location[i + 3 + j..],
            None => "/",
        }
    } else {
        location
    };
    let path = path.trim_end_matches('/');
    if path.starts_with('/') {
        format!("{origin}{path}")
    } else {
        format!("{base}/{path}")
    }
}

/// The ScanSettings document for `p`.
pub fn scan_settings(p: &JobParams) -> String {
    let (source, duplex) = match p.source {
        Source::Glass => ("Platen", None),
        Source::Feeder | Source::Auto => ("Feeder", Some(false)),
        Source::FeederDuplex => ("Feeder", Some(true)),
    };
    let duplex = duplex
        .map(|d| format!("<scan:Duplex>{d}</scan:Duplex>"))
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><scan:ScanSettings xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm" xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03"><pwg:Version>2.0</pwg:Version><pwg:ScanRegions><pwg:ScanRegion><pwg:ContentRegionUnits>escl:ThreeHundredthsOfInches</pwg:ContentRegionUnits><pwg:XOffset>0</pwg:XOffset><pwg:YOffset>0</pwg:YOffset><pwg:Width>{w}</pwg:Width><pwg:Height>{h}</pwg:Height></pwg:ScanRegion></pwg:ScanRegions><pwg:InputSource>{source}</pwg:InputSource><scan:ColorMode>{c}</scan:ColorMode><pwg:DocumentFormat>{f}</pwg:DocumentFormat><scan:DocumentFormatExt>{f}</scan:DocumentFormatExt><scan:XResolution>{d}</scan:XResolution><scan:YResolution>{d}</scan:YResolution>{duplex}</scan:ScanSettings>"#,
        w = p.width,
        h = p.height,
        c = p.color,
        f = p.format,
        d = p.dpi,
    )
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.descendants().find(|c| c.has_tag_name(tag))
}

fn number(n: roxmltree::Node, tag: &str) -> u32 {
    child(n, tag)
        .and_then(|c| c.text())
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

fn texts(n: roxmltree::Node, tag: &str) -> Vec<String> {
    let mut out: Vec<String> = n
        .descendants()
        .filter(|c| c.has_tag_name(tag))
        .filter_map(|c| c.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    out.dedup();
    out
}

fn source_caps(node: roxmltree::Node) -> SourceCaps {
    let mut resolutions: Vec<u32> = node
        .descendants()
        .filter(|c| c.has_tag_name("DiscreteResolution"))
        .map(|r| number(r, "XResolution"))
        .filter(|r| *r > 0)
        .collect();
    resolutions.sort_unstable();
    resolutions.dedup();
    let mut formats = texts(node, "DocumentFormat");
    formats.extend(texts(node, "DocumentFormatExt"));
    formats.sort();
    formats.dedup();
    SourceCaps {
        resolutions,
        colors: texts(node, "ColorMode"),
        formats,
        max_width: number(node, "MaxWidth"),
        max_height: number(node, "MaxHeight"),
    }
}

/// Parse a ScannerCapabilities document.
pub fn parse_capabilities(text: &str) -> Result<Capabilities, ScanError> {
    let doc = Document::parse(text).map_err(|e| ScanError::Protocol(e.to_string()))?;
    let root = doc.root_element();
    if !root.has_tag_name("ScannerCapabilities") {
        return Err(ScanError::Protocol(
            "the reply is not a ScannerCapabilities document".into(),
        ));
    }
    let platen = child(root, "PlatenInputCaps").map(source_caps);
    let simplex = child(root, "AdfSimplexInputCaps").map(source_caps);
    let duplex_caps = child(root, "AdfDuplexInputCaps").map(source_caps);
    let duplex = duplex_caps.is_some()
        || child(root, "AdfOptions")
            .map(|o| texts(o, "AdfOption").iter().any(|v| v == "Duplex"))
            .unwrap_or(false);
    Ok(Capabilities {
        make_and_model: child(root, "MakeAndModel")
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string()),
        platen,
        feeder: simplex.or(duplex_caps),
        duplex,
    })
}

/// Parse a ScannerStatus document.
pub fn parse_status(text: &str) -> Result<Status, ScanError> {
    let doc = Document::parse(text).map_err(|e| ScanError::Protocol(e.to_string()))?;
    let root = doc.root_element();
    Ok(Status {
        state: child(root, "State")
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
            .unwrap_or_default(),
        adf_state: child(root, "AdfState")
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Paper;

    // Hand-built from the eSCL specification's examples (no eSCL device was
    // available to record from); see tests/fixtures/escl/README.md.
    const CAPS: &str =
        include_str!("../../tests/fixtures/escl/spec-example.scannercapabilities.xml");
    const STATUS_LOADED: &str =
        include_str!("../../tests/fixtures/escl/spec-example.scannerstatus-adf-loaded.xml");
    const STATUS_EMPTY: &str =
        include_str!("../../tests/fixtures/escl/spec-example.scannerstatus-adf-empty.xml");

    fn scanner() -> EsclScanner {
        EsclScanner {
            base: "http://192.168.1.20:80/eSCL".into(),
            agent: ureq::agent(),
            caps: parse_capabilities(CAPS).unwrap(),
            status: Status::default(),
        }
    }

    #[test]
    fn parses_capabilities() {
        let s = scanner();
        assert_eq!(
            s.caps.make_and_model.as_deref(),
            Some("Example AirScan MFP")
        );
        let platen = s.caps.platen.as_ref().unwrap();
        assert_eq!(platen.resolutions, vec![75, 100, 200, 300, 600]);
        assert_eq!(platen.colors, vec!["BlackAndWhite1", "Grayscale8", "RGB24"]);
        assert_eq!(platen.formats, vec!["application/pdf", "image/jpeg"]);
        assert_eq!((platen.max_width, platen.max_height), (2550, 3508));
        let feeder = s.caps.feeder.as_ref().unwrap();
        assert_eq!(feeder.resolutions, vec![100, 200, 300]);
        assert_eq!((feeder.max_width, feeder.max_height), (2550, 4200));
        assert!(s.caps.duplex);
    }

    #[test]
    fn chooses_parameters_in_escl_units() {
        let s = scanner();
        let (p, notes) = s
            .params_for(&ScanRequest::default(), Source::Glass)
            .unwrap();
        assert_eq!(p.dpi, 300);
        assert_eq!(p.color, "RGB24");
        assert_eq!(p.format, "image/jpeg");
        assert_eq!((p.width, p.height), (2480, 3507)); // A4 in 1/300 inch
        assert!(notes.is_empty());
        let req = ScanRequest {
            dpi: 1200,
            color: ColorMode::Gray,
            paper: Paper::Max,
            ..Default::default()
        };
        let (p, notes) = s.params_for(&req, Source::Feeder).unwrap();
        assert_eq!(p.dpi, 300);
        assert_eq!(p.color, "Grayscale8");
        assert_eq!((p.width, p.height), (2550, 4200));
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn scan_settings_follow_the_spec() {
        let p = JobParams {
            source: Source::FeederDuplex,
            dpi: 300,
            color: "RGB24".into(),
            format: "image/jpeg".into(),
            width: 2480,
            height: 3507,
        };
        let xml = scan_settings(&p);
        let doc = Document::parse(&xml).unwrap();
        let root = doc.root_element();
        assert!(root.has_tag_name("ScanSettings"));
        let text = |tag: &str| child(root, tag).and_then(|n| n.text()).unwrap().to_string();
        assert_eq!(text("InputSource"), "Feeder");
        assert_eq!(text("Duplex"), "true");
        assert_eq!(text("ContentRegionUnits"), "escl:ThreeHundredthsOfInches");
        assert_eq!(text("Width"), "2480");
        assert_eq!(text("XResolution"), "300");
        assert_eq!(text("DocumentFormat"), "image/jpeg");
        let glass = scan_settings(&JobParams {
            source: Source::Glass,
            ..p
        });
        assert!(!glass.contains("Duplex"));
        assert!(glass.contains("<pwg:InputSource>Platen</pwg:InputSource>"));
    }

    #[test]
    fn reads_status_and_adf_state() {
        let s = parse_status(STATUS_LOADED).unwrap();
        assert_eq!(s.state, "Idle");
        assert_eq!(s.adf_state.as_deref(), Some("ScannerAdfLoaded"));
        let s = parse_status(STATUS_EMPTY).unwrap();
        assert_eq!(s.adf_state.as_deref(), Some("ScannerAdfEmpty"));
    }

    #[test]
    fn job_url_ignores_untrusted_hostnames() {
        let base = "http://192.168.1.20:80/eSCL";
        assert_eq!(
            job_url(base, "http://wrong-host/eSCL/ScanJobs/123"),
            "http://192.168.1.20:80/eSCL/ScanJobs/123"
        );
        assert_eq!(
            job_url(base, "/eSCL/ScanJobs/123/"),
            "http://192.168.1.20:80/eSCL/ScanJobs/123"
        );
        assert_eq!(
            job_url(base, "ScanJobs/123"),
            "http://192.168.1.20:80/eSCL/ScanJobs/123"
        );
        // Xerox-style broken IPv6 Location.
        assert_eq!(
            job_url(base, "http://[fe80/eSCL/ScanJobs/4060"),
            "http://192.168.1.20:80/eSCL/ScanJobs/4060"
        );
    }
}
