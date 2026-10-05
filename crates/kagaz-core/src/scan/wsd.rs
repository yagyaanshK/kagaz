//! WSD-Scan (Microsoft "Web Services on Devices" scanning, the protocol
//! Windows uses for network scanners). SOAP 1.2 over HTTP to the device's
//! hosted ScannerService:
//!
//! 1. `GetScannerElements` for what the scanner can do and whether it is idle,
//! 2. `CreateScanJob` with a scan ticket, which returns a job id and token,
//! 3. `RetrieveImage` once per page; the reply is MTOM multipart with the
//!    image as its second part. On the glass that is one page; from the
//!    feeder it repeats until the device says the job is gone.
//!
//! Behaviour checked against a Brother DCP-L2540DW (recordings in
//! tests/fixtures/wsd-scan) and against sane-airscan's WSD backend.

use super::multipart;
use super::{ColorMode, Event, Page, ScanError, ScanRequest, Source};
use roxmltree::Document;
use std::io::Read;
use std::time::Duration;

const NS_SCAN: &str = "http://schemas.microsoft.com/windows/2006/08/wdp/scan";
const ANONYMOUS: &str = "http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous";

/// Capabilities of one input source, in the units the protocol uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceCaps {
    pub resolutions: Vec<u32>,
    /// WSD colour names: "RGB24", "Grayscale8", "BlackAndWhite1", ...
    pub colors: Vec<String>,
    /// Largest scan area, 1/1000 inch.
    pub max_width: u32,
    pub max_height: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub name: Option<String>,
    /// WSD format names: "jfif", "exif", "png", "tiff-single-g4", "pdf-a", ...
    pub formats: Vec<String>,
    pub platen: Option<SourceCaps>,
    pub feeder: Option<SourceCaps>,
    pub duplex: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// "Idle", "Processing", "Stopped", ...
    pub state: String,
    /// Active device conditions, e.g. "CoverOpen", "MediaJam", "InputTrayEmpty".
    pub conditions: Vec<String>,
}

/// What was actually asked of the device for one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobParams {
    pub source: Source,
    pub dpi: u32,
    pub color: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

pub struct WsdScanner {
    service: String,
    agent: ureq::Agent,
    pub caps: Capabilities,
    pub status: Status,
}

impl WsdScanner {
    /// Ask the scanner service at `service` what it can do.
    pub fn connect(service: &str) -> Result<WsdScanner, ScanError> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(180))
            .build();
        let mut scanner = WsdScanner {
            service: service.to_string(),
            agent,
            caps: Capabilities::default(),
            status: Status::default(),
        };
        let body = r#"<sca:GetScannerElementsRequest><sca:RequestedElements><sca:Name>sca:ScannerConfiguration</sca:Name><sca:Name>sca:ScannerDescription</sca:Name><sca:Name>sca:ScannerStatus</sca:Name></sca:RequestedElements></sca:GetScannerElementsRequest>"#;
        let (_, reply) = scanner.post("GetScannerElements", body)?;
        let text = String::from_utf8_lossy(&reply);
        check_fault(&text)?;
        let (caps, status) = parse_elements(&text)?;
        scanner.caps = caps;
        scanner.status = status;
        Ok(scanner)
    }

    /// Run one scan job, or two when the feeder turns out to be empty and
    /// the request allowed the glass.
    pub fn scan(
        &self,
        req: &ScanRequest,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<Vec<Page>, ScanError> {
        if self.caps.platen.is_none() && self.caps.feeder.is_none() {
            return Err(ScanError::Protocol(
                "the scanner lists neither a glass nor a feeder".into(),
            ));
        }
        let try_feeder = match req.source {
            Source::Auto => self.caps.feeder.is_some(),
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
                Err(ScanError::FeederEmpty) | Err(ScanError::Busy(_))
                    if req.source == Source::Auto && self.caps.platen.is_some() =>
                {
                    on_event(Event::FeederEmpty);
                }
                Err(e) => return Err(e),
            }
        }
        self.run_job(req, Source::Glass, on_event)
    }

    /// Choose what to ask for, given the request and what the device offers.
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
        let color = if caps.colors.iter().any(|c| c == wanted) || caps.colors.is_empty() {
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
                "the scanner only scans in {}; converting to {} afterwards",
                fallback,
                match req.color {
                    ColorMode::Color => "colour",
                    ColorMode::Gray => "grey",
                    ColorMode::BlackWhite => "black and white",
                }
            ));
            fallback
        };

        let format = ["jfif", "exif", "png"]
            .iter()
            .find(|f| self.caps.formats.iter().any(|have| have == *f))
            .map(|f| f.to_string())
            .ok_or_else(|| {
                ScanError::Protocol(format!(
                    "the scanner offers none of the image formats Kagaz reads (it lists: {})",
                    self.caps.formats.join(", ")
                ))
            })?;

        let (width, height) = match req.paper.size_mils() {
            Some((w, h)) => (w.min(caps.max_width), h.min(caps.max_height)),
            None => (caps.max_width, caps.max_height),
        };

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
        let (_, reply) = self.post("CreateScanJob", &create_job_body(&params))?;
        let text = String::from_utf8_lossy(&reply);
        check_fault(&text)?;
        let (job_id, token) = parse_create_job(&text)?;

        let mime = if params.format == "png" {
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
        loop {
            let body = format!(
                "<sca:RetrieveImageRequest><sca:JobId>{job_id}</sca:JobId><sca:JobToken>{token}</sca:JobToken><sca:DocumentDescription><sca:DocumentName>IMAGE{:03}.JPG</sca:DocumentName></sca:DocumentDescription></sca:RetrieveImageRequest>",
                pages.len()
            );
            let (content_type, reply) = match self.post("RetrieveImage", &body) {
                Ok(r) => r,
                Err(e) => {
                    // The job vanishing after the last page is how the
                    // feeder says "no more paper".
                    if !pages.is_empty()
                        && matches!(e, ScanError::Refused(_) | ScanError::FeederEmpty)
                    {
                        break;
                    }
                    return Err(e);
                }
            };
            let Some(boundary) = multipart::boundary_of(&content_type) else {
                let text = String::from_utf8_lossy(&reply);
                match check_fault(&text) {
                    Err(ScanError::Refused(_)) | Err(ScanError::FeederEmpty)
                        if !pages.is_empty() =>
                    {
                        break
                    }
                    Err(e) => return Err(e),
                    Ok(()) => {
                        return Err(ScanError::Protocol(
                            "RetrieveImage reply is not multipart".into(),
                        ))
                    }
                }
            };
            let parts = multipart::parse(&reply, &boundary);
            let Some(image) = parts.iter().find(|p| !p.content_type().contains("xml")) else {
                return Err(ScanError::Protocol(
                    "RetrieveImage reply has no image part".into(),
                ));
            };
            pages.push(Page {
                data: image.body.clone(),
                mime,
                dpi: params.dpi,
                color: color_scanned,
            });
            on_event(Event::Page {
                number: pages.len(),
                bytes: image.body.len(),
            });
            if source == Source::Glass {
                break;
            }
        }
        Ok(pages)
    }

    /// POST one SOAP action; returns the reply's content type and raw body.
    /// HTTP error statuses are returned as bodies too, since WSD faults ride on them.
    fn post(&self, action: &str, body: &str) -> Result<(String, Vec<u8>), ScanError> {
        let envelope = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope" xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:sca="{NS_SCAN}"><soap:Header><wsa:To>{}</wsa:To><wsa:Action>{NS_SCAN}/{action}</wsa:Action><wsa:MessageID>urn:uuid:{}</wsa:MessageID><wsa:ReplyTo><wsa:Address>{ANONYMOUS}</wsa:Address></wsa:ReplyTo></soap:Header><soap:Body>{body}</soap:Body></soap:Envelope>"#,
            self.service,
            uuid::Uuid::new_v4()
        );
        let request = self
            .agent
            .post(&self.service)
            .set("Content-Type", "application/soap+xml; charset=utf-8")
            .set("User-Agent", "WSDAPI");
        let response = match request.send_string(&envelope) {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => return Err(ScanError::Transport(t.to_string())),
        };
        let content_type = response
            .header("Content-Type")
            .unwrap_or_default()
            .to_string();
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| ScanError::Transport(e.to_string()))?;
        if let Ok(text) = std::str::from_utf8(&bytes) {
            if !content_type.to_ascii_lowercase().contains("multipart") {
                check_fault(text)?;
            }
        }
        Ok((content_type, bytes))
    }
}

/// The scan ticket for `params`.
pub fn create_job_body(p: &JobParams) -> String {
    let (source, sides) = match p.source {
        Source::Glass => ("Platen", vec!["MediaFront"]),
        Source::Feeder | Source::Auto => ("ADF", vec!["MediaFront"]),
        Source::FeederDuplex => ("ADFDuplex", vec!["MediaFront", "MediaBack"]),
    };
    // 1 image from the glass; 0 = "until the feeder is empty".
    let images = if p.source == Source::Glass { 1 } else { 0 };
    let mut media = String::new();
    for side in sides {
        media.push_str(&format!(
            "<sca:{side}><sca:ScanRegion><sca:ScanRegionXOffset>0</sca:ScanRegionXOffset><sca:ScanRegionYOffset>0</sca:ScanRegionYOffset><sca:ScanRegionWidth>{w}</sca:ScanRegionWidth><sca:ScanRegionHeight>{h}</sca:ScanRegionHeight></sca:ScanRegion><sca:ColorProcessing>{c}</sca:ColorProcessing><sca:Resolution><sca:Width>{d}</sca:Width><sca:Height>{d}</sca:Height></sca:Resolution></sca:{side}>",
            w = p.width,
            h = p.height,
            c = p.color,
            d = p.dpi
        ));
    }
    format!(
        "<sca:CreateScanJobRequest><sca:ScanTicket><sca:JobDescription><sca:JobName>Kagaz scan</sca:JobName><sca:JobOriginatingUserName>kagaz</sca:JobOriginatingUserName></sca:JobDescription><sca:DocumentParameters><sca:Format>{f}</sca:Format><sca:ImagesToTransfer>{images}</sca:ImagesToTransfer><sca:InputSource>{source}</sca:InputSource><sca:ContentType>Auto</sca:ContentType><sca:InputSize><sca:InputMediaSize><sca:Width>{w}</sca:Width><sca:Height>{h}</sca:Height></sca:InputMediaSize></sca:InputSize><sca:MediaSides>{media}</sca:MediaSides></sca:DocumentParameters></sca:ScanTicket></sca:CreateScanJobRequest>",
        f = p.format,
        w = p.width,
        h = p.height,
    )
}

/// Turn a SOAP fault into the matching `ScanError`; `Ok` when `text` is not a fault.
pub fn check_fault(text: &str) -> Result<(), ScanError> {
    if !text.contains("addressing/fault") && !text.contains(":Fault") {
        return Ok(());
    }
    let doc = Document::parse(text).map_err(|e| ScanError::Protocol(e.to_string()))?;
    let Some(fault) = doc.descendants().find(|n| n.has_tag_name("Fault")) else {
        return Ok(());
    };
    let subcode = fault
        .descendants()
        .find(|n| n.has_tag_name("Subcode"))
        .and_then(|s| s.descendants().find(|n| n.has_tag_name("Value")))
        .and_then(|n| n.text())
        .map(|t| t.trim().rsplit(':').next().unwrap_or(t).to_string())
        .unwrap_or_default();
    let reason = fault
        .descendants()
        .find(|n| n.has_tag_name("Text"))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    let detail = fault
        .descendants()
        .find(|n| n.has_tag_name("Detail"))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    let message = match (reason.is_empty(), detail.is_empty()) {
        (false, false) => format!("{subcode}: {reason} ({detail})"),
        (false, true) => format!("{subcode}: {reason}"),
        _ => subcode.clone(),
    };
    Err(match subcode.as_str() {
        "ClientErrorNoImagesAvailable"
        | "ClientErrorJobIdNotFound"
        | "ClientErrorJobTokenInvalid" => ScanError::Refused(message),
        // What the Brother sends when the feeder has no paper.
        "ServerErrorNotAcceptingJobs" => ScanError::Busy(message),
        "ServerErrorScannerBusy" | "ServerErrorDeviceBusy" => ScanError::Busy(message),
        "ClientErrorInputTrayEmpty" => ScanError::FeederEmpty,
        _ => ScanError::Refused(message),
    })
}

/// The first descendant of `n` with tag `tag`.
fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.descendants().find(|c| c.has_tag_name(tag))
}

/// Parse a GetScannerElementsResponse.
pub fn parse_elements(text: &str) -> Result<(Capabilities, Status), ScanError> {
    let doc = Document::parse(text).map_err(|e| ScanError::Protocol(e.to_string()))?;
    let root = doc.root_element();
    let texts = |n: roxmltree::Node, tag: &str| -> Vec<String> {
        n.descendants()
            .filter(|c| c.has_tag_name(tag))
            .filter_map(|c| c.text())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    };
    let number = |n: roxmltree::Node, tag: &str| -> u32 {
        child(n, tag)
            .and_then(|c| c.text())
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(0)
    };
    let source_caps = |node: roxmltree::Node, res: &str, color: &str, max: &str| SourceCaps {
        resolutions: child(node, res)
            .and_then(|r| child(r, "Widths"))
            .map(|w| {
                texts(w, "Width")
                    .iter()
                    .filter_map(|t| t.parse().ok())
                    .collect()
            })
            .unwrap_or_default(),
        colors: child(node, color)
            .map(|c| texts(c, "ColorEntry"))
            .unwrap_or_default(),
        max_width: child(node, max).map(|m| number(m, "Width")).unwrap_or(0),
        max_height: child(node, max).map(|m| number(m, "Height")).unwrap_or(0),
    };

    let config = child(root, "ScannerConfiguration")
        .ok_or_else(|| ScanError::Protocol("no ScannerConfiguration in the reply".into()))?;
    let platen = child(config, "Platen")
        .map(|p| source_caps(p, "PlatenResolutions", "PlatenColor", "PlatenMaximumSize"));
    let feeder = child(config, "ADFFront")
        .map(|a| source_caps(a, "ADFResolutions", "ADFColor", "ADFMaximumSize"));
    let duplex = child(config, "ADFSupportsDuplex")
        .and_then(|n| n.text())
        .map(|t| matches!(t.trim(), "1" | "true"))
        .unwrap_or(false);
    let caps = Capabilities {
        name: child(root, "ScannerName")
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string()),
        formats: child(config, "FormatsSupported")
            .map(|f| texts(f, "FormatValue"))
            .unwrap_or_default(),
        platen,
        feeder,
        duplex,
    };
    let status = child(root, "ScannerStatus")
        .map(|s| Status {
            state: child(s, "ScannerState")
                .and_then(|n| n.text())
                .map(|t| t.trim().to_string())
                .unwrap_or_default(),
            conditions: child(s, "ActiveConditions")
                .map(|a| texts(a, "Name"))
                .unwrap_or_default(),
        })
        .unwrap_or_default();
    Ok((caps, status))
}

/// Parse a CreateScanJobResponse into (job id, job token).
pub fn parse_create_job(text: &str) -> Result<(String, String), ScanError> {
    let doc = Document::parse(text).map_err(|e| ScanError::Protocol(e.to_string()))?;
    let get = |tag: &str| {
        doc.descendants()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    match (get("JobId"), get("JobToken")) {
        (Some(id), Some(token)) => Ok((id, token)),
        _ => Err(ScanError::Protocol(
            "CreateScanJob reply has no job id or token".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Paper;

    // Recorded from the Brother DCP-L2540DW on 2026-10-05; see
    // tests/fixtures/wsd-scan/README.md.
    const ELEMENTS: &str = include_str!(
        "../../tests/fixtures/wsd-scan/brother-dcp-l2540dw.getscannerelements.response.xml"
    );
    const CREATE_JOB: &str = include_str!(
        "../../tests/fixtures/wsd-scan/brother-dcp-l2540dw.createscanjob.response.xml"
    );
    const RETRIEVE: &[u8] = include_bytes!(
        "../../tests/fixtures/wsd-scan/brother-dcp-l2540dw.retrieveimage.response.bin"
    );
    const AFTER_LAST: &str = include_str!(
        "../../tests/fixtures/wsd-scan/brother-dcp-l2540dw.retrieveimage-after-last.response.xml"
    );
    const ADF_EMPTY: &str = include_str!(
        "../../tests/fixtures/wsd-scan/brother-dcp-l2540dw.createscanjob-adf-empty.response.xml"
    );

    fn brother() -> WsdScanner {
        let (caps, status) = parse_elements(ELEMENTS).unwrap();
        WsdScanner {
            service: "http://198.51.100.167:80/WebServices/ScannerService".into(),
            agent: ureq::agent(),
            caps,
            status,
        }
    }

    #[test]
    fn parses_the_brother_capabilities() {
        let s = brother();
        assert_eq!(
            s.caps.name.as_deref(),
            Some("Brother DCP-L2540DW series Scanner")
        );
        assert_eq!(s.caps.formats, vec!["exif"]);
        let platen = s.caps.platen.as_ref().unwrap();
        assert_eq!(platen.resolutions, vec![100, 200, 300]);
        assert_eq!(platen.colors, vec!["RGB24"]);
        assert_eq!((platen.max_width, platen.max_height), (8500, 11700));
        let feeder = s.caps.feeder.as_ref().unwrap();
        assert_eq!((feeder.max_width, feeder.max_height), (8500, 14000));
        assert!(!s.caps.duplex);
        assert_eq!(s.status.state, "Idle");
        assert!(s.status.conditions.is_empty());
    }

    #[test]
    fn picks_parameters_the_device_offers() {
        let s = brother();
        let req = ScanRequest::default();
        let (p, notes) = s.params_for(&req, Source::Glass).unwrap();
        assert_eq!(p.dpi, 300);
        assert_eq!(p.color, "RGB24");
        assert_eq!(p.format, "exif");
        assert_eq!((p.width, p.height), (8268, 11693)); // A4 fits the glass
        assert!(notes.is_empty());

        let req = ScanRequest {
            dpi: 600,
            color: ColorMode::Gray,
            paper: Paper::Legal,
            ..Default::default()
        };
        let (p, notes) = s.params_for(&req, Source::Feeder).unwrap();
        assert_eq!(p.dpi, 300);
        assert_eq!(p.color, "RGB24");
        assert_eq!((p.width, p.height), (8500, 14000));
        assert_eq!(notes.len(), 2);
        let (p, _) = s.params_for(&req, Source::Glass).unwrap();
        assert_eq!((p.width, p.height), (8500, 11700)); // legal clipped to the glass

        let req = ScanRequest {
            dpi: 50,
            ..Default::default()
        };
        assert_eq!(s.params_for(&req, Source::Glass).unwrap().0.dpi, 100);
    }

    #[test]
    fn ticket_matches_what_the_brother_accepted() {
        let p = JobParams {
            source: Source::Glass,
            dpi: 100,
            color: "RGB24".into(),
            format: "exif".into(),
            width: 2000,
            height: 2000,
        };
        let body = create_job_body(&p);
        assert!(body.contains("<sca:Format>exif</sca:Format><sca:ImagesToTransfer>1</sca:ImagesToTransfer><sca:InputSource>Platen</sca:InputSource>"));
        assert!(body.contains("<sca:ScanRegionWidth>2000</sca:ScanRegionWidth>"));
        assert!(body.contains("<sca:Resolution><sca:Width>100</sca:Width><sca:Height>100</sca:Height></sca:Resolution>"));
        assert!(!body.contains("MediaBack"));
        let feeder = create_job_body(&JobParams {
            source: Source::FeederDuplex,
            ..p
        });
        assert!(feeder.contains("<sca:ImagesToTransfer>0</sca:ImagesToTransfer><sca:InputSource>ADFDuplex</sca:InputSource>"));
        assert!(feeder.contains("<sca:MediaBack>"));
    }

    #[test]
    fn reads_job_id_and_token() {
        let (id, token) = parse_create_job(CREATE_JOB).unwrap();
        assert_eq!(id, "1");
        assert_eq!(token, "urn:uuid:dc57d0e4-e22d-470b-875b-000000000001");
        assert!(check_fault(CREATE_JOB).is_ok());
    }

    #[test]
    fn extracts_the_jpeg_from_the_multipart_reply() {
        // The fixture is "Content-Type: ...\r\n\r\n" followed by the raw body.
        let split = RETRIEVE.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let content_type = std::str::from_utf8(&RETRIEVE[..split]).unwrap();
        let content_type = content_type.strip_prefix("Content-Type: ").unwrap();
        let body = &RETRIEVE[split + 4..];
        let boundary = multipart::boundary_of(content_type).unwrap();
        let parts = multipart::parse(body, &boundary);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].content_type().contains("xop+xml"));
        let image = parts
            .iter()
            .find(|p| !p.content_type().contains("xml"))
            .unwrap();
        assert_eq!(image.content_type(), "image/jpeg");
        assert_eq!(&image.body[..2], b"\xff\xd8");
        assert_eq!(&image.body[image.body.len() - 2..], b"\xff\xd9");
        let (w, h, comps) = crate::output::jpeg::dimensions(&image.body).unwrap();
        assert_eq!((w, h, comps), (176, 189, 3)); // 2x2 inch at 100 dpi, less the margin
    }

    #[test]
    fn classifies_faults() {
        assert!(
            matches!(check_fault(AFTER_LAST), Err(ScanError::Refused(m)) if m.starts_with("ClientErrorJobIdNotFound"))
        );
        assert!(
            matches!(check_fault(ADF_EMPTY), Err(ScanError::Busy(m)) if m.contains("temporarily blocked"))
        );
        assert!(check_fault("<a>no fault here</a>").is_ok());
    }
}
