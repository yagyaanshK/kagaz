//! What a printer says about itself over IPP, reduced to what a person
//! wants to know: is it ready, what is wrong, how much toner, what paper.

use super::{
    call, encode_request, out, urls_for, IppError, Response, Value, GROUP_JOB, OP_GET_JOBS,
    OP_GET_PRINTER_ATTRIBUTES, OP_IDENTIFY_PRINTER, TAG_KEYWORD,
};
use crate::Device;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrinterState {
    Idle,
    Processing,
    Stopped,
    Unknown,
}

impl PrinterState {
    pub fn label(self) -> &'static str {
        match self {
            PrinterState::Idle => "idle",
            PrinterState::Processing => "printing",
            PrinterState::Stopped => "stopped",
            PrinterState::Unknown => "unknown",
        }
    }
}

/// One printer-state-reason, split into what and how bad.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    /// The keyword without its severity suffix, e.g. "media-empty".
    pub keyword: String,
    /// "error", "warning", "report" or "" when the printer gave none.
    pub severity: String,
    /// Plain words, e.g. "out of paper".
    pub text: String,
}

/// A consumable: toner, ink, drum, ...
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supply {
    pub name: String,
    /// "toner", "ink", "opc" (drum), "fuser", ...
    pub kind: String,
    /// "#RRGGBB" when the printer gives one.
    pub color: Option<String>,
    /// Percent remaining, when known.
    pub level: Option<i32>,
    /// Percent at which the printer calls it low.
    pub low_at: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: i32,
    pub name: String,
    pub user: String,
    pub state: String,
    pub reasons: Vec<String>,
    pub impressions_completed: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrinterStatus {
    pub name: String,
    pub info: Option<String>,
    pub make_and_model: Option<String>,
    pub location: Option<String>,
    pub state: PrinterState,
    pub reasons: Vec<Reason>,
    pub message: Option<String>,
    pub accepting_jobs: bool,
    pub queued_jobs: i32,
    pub supplies: Vec<Supply>,
    /// Paper loaded now, as IPP media keywords ("iso_a4_210x297mm").
    pub media_ready: Vec<String>,
    pub media_default: Option<String>,
    pub color: bool,
    pub duplex: bool,
    pub pages_per_minute: Option<i32>,
    pub max_dpi: Option<i32>,
    pub document_formats: Vec<String>,
    /// Seconds since the printer was switched on.
    pub up_time: Option<i32>,
    pub uuid: Option<String>,
    pub more_info: Option<String>,
    pub jobs: Vec<Job>,
}

/// Ask `device` for its status and its queue.
pub fn fetch(device: &Device) -> Result<PrinterStatus, IppError> {
    let (http, uri) = urls_for(device)?;
    let request = encode_request(
        OP_GET_PRINTER_ATTRIBUTES,
        1,
        &uri,
        &[out(TAG_KEYWORD, "requested-attributes", &["all"])],
        &[],
        &[],
    );
    let printer = call(&http, &request, Duration::from_secs(15))?;
    if !printer.ok() {
        return Err(IppError::Status(
            printer
                .status_message()
                .map(str::to_string)
                .unwrap_or_else(|| super::status_text(printer.status)),
        ));
    }
    let request = encode_request(
        OP_GET_JOBS,
        2,
        &uri,
        &[
            out(TAG_KEYWORD, "which-jobs", &["not-completed"]),
            out(TAG_KEYWORD, "requested-attributes", &["all"]),
        ],
        &[],
        &[],
    );
    let jobs = call(&http, &request, Duration::from_secs(15)).ok();
    Ok(from_responses(&printer, jobs.as_ref()))
}

/// Make the printer flash or beep so you know which one it is.
pub fn identify(device: &Device) -> Result<(), IppError> {
    let (http, uri) = urls_for(device)?;
    let request = encode_request(
        OP_IDENTIFY_PRINTER,
        3,
        &uri,
        &[out(TAG_KEYWORD, "identify-actions", &["flash"])],
        &[],
        &[],
    );
    let r = call(&http, &request, Duration::from_secs(15))?;
    if r.ok() {
        Ok(())
    } else {
        Err(IppError::Status(
            r.status_message()
                .map(str::to_string)
                .unwrap_or_else(|| super::status_text(r.status)),
        ))
    }
}

/// Build the status from decoded replies (so it can be tested on recordings).
pub fn from_responses(printer: &Response, jobs: Option<&Response>) -> PrinterStatus {
    let text = |name: &str| -> Option<String> {
        printer
            .printer(name)?
            .first()?
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let int = |name: &str| -> Option<i32> { printer.printer(name)?.first()?.as_i32() };
    let strings = |name: &str| -> Vec<String> {
        printer
            .printer(name)
            .map(|a| a.strings())
            .unwrap_or_default()
    };
    let ints = |name: &str| -> Vec<Option<i32>> {
        printer
            .printer(name)
            .map(|a| a.values.iter().map(Value::as_i32).collect())
            .unwrap_or_default()
    };

    let state = match int("printer-state") {
        Some(3) => PrinterState::Idle,
        Some(4) => PrinterState::Processing,
        Some(5) => PrinterState::Stopped,
        _ => PrinterState::Unknown,
    };
    let reasons = strings("printer-state-reasons")
        .into_iter()
        .filter(|r| r != "none")
        .map(|r| parse_reason(&r))
        .collect();

    let names = strings("marker-names");
    let kinds = strings("marker-types");
    let colors = strings("marker-colors");
    let levels = ints("marker-levels");
    let lows = ints("marker-low-levels");
    let supplies = names
        .iter()
        .enumerate()
        .map(|(i, name)| Supply {
            name: name.clone(),
            kind: kinds.get(i).cloned().unwrap_or_default(),
            color: colors.get(i).cloned().filter(|c| c.starts_with('#')),
            level: levels.get(i).copied().flatten().filter(|l| *l >= 0),
            low_at: lows.get(i).copied().flatten().filter(|l| *l > 0),
        })
        .collect();

    let max_dpi = printer
        .printer("printer-resolution-supported")
        .map(|a| {
            a.values
                .iter()
                .filter_map(|v| match v {
                    Value::Resolution { x, y, .. } => Some((*x).max(*y)),
                    _ => None,
                })
                .max()
        })
        .unwrap_or(None);

    let jobs = jobs
        .map(|r| {
            r.groups_of(GROUP_JOB)
                .map(|g| {
                    let s = |n: &str| {
                        g.get(n)
                            .and_then(|a| a.first())
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    Job {
                        id: g
                            .get("job-id")
                            .and_then(|a| a.first())
                            .and_then(Value::as_i32)
                            .unwrap_or(0),
                        name: s("job-name"),
                        user: s("job-originating-user-name"),
                        state: job_state(
                            g.get("job-state")
                                .and_then(|a| a.first())
                                .and_then(Value::as_i32),
                        ),
                        reasons: g
                            .get("job-state-reasons")
                            .map(|a| a.strings())
                            .unwrap_or_default(),
                        impressions_completed: g
                            .get("job-impressions-completed")
                            .and_then(|a| a.first())
                            .and_then(Value::as_i32),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    PrinterStatus {
        name: text("printer-name").unwrap_or_default(),
        info: text("printer-info"),
        make_and_model: text("printer-make-and-model"),
        location: text("printer-location"),
        state,
        reasons,
        message: text("printer-state-message"),
        accepting_jobs: printer
            .printer("printer-is-accepting-jobs")
            .and_then(|a| a.first())
            .and_then(Value::as_bool)
            .unwrap_or(true),
        queued_jobs: int("queued-job-count").unwrap_or(0),
        supplies,
        media_ready: strings("media-ready"),
        media_default: text("media-default"),
        color: printer
            .printer("color-supported")
            .and_then(|a| a.first())
            .and_then(Value::as_bool)
            .unwrap_or(false),
        duplex: strings("sides-supported")
            .iter()
            .any(|s| s.starts_with("two-sided")),
        pages_per_minute: int("pages-per-minute"),
        max_dpi,
        document_formats: strings("document-format-supported"),
        up_time: int("printer-up-time"),
        uuid: text("printer-uuid"),
        more_info: text("printer-more-info"),
        jobs,
    }
}

fn job_state(v: Option<i32>) -> String {
    match v {
        Some(3) => "pending",
        Some(4) => "held",
        Some(5) => "processing",
        Some(6) => "stopped",
        Some(7) => "canceled",
        Some(8) => "aborted",
        Some(9) => "completed",
        _ => "unknown",
    }
    .to_string()
}

/// "media-empty-warning" -> keyword "media-empty", severity "warning", text "out of paper".
pub fn parse_reason(raw: &str) -> Reason {
    let (keyword, severity) = ["-error", "-warning", "-report"]
        .iter()
        .find_map(|s| raw.strip_suffix(s).map(|k| (k, &s[1..])))
        .unwrap_or((raw, ""));
    let text = match keyword {
        "media-empty" => "out of paper",
        "media-needed" => "needs paper",
        "media-jam" => "paper jam",
        "media-low" => "paper low",
        "toner-low" => "toner low",
        "toner-empty" => "toner empty",
        "marker-supply-low" => "ink or toner low",
        "marker-supply-empty" => "ink or toner empty",
        "marker-waste-almost-full" => "waste container almost full",
        "marker-waste-full" => "waste container full",
        "cover-open" => "a cover is open",
        "door-open" => "a door is open",
        "interlock-open" => "an interlock is open",
        "input-tray-missing" => "a paper tray is missing",
        "output-area-almost-full" => "output tray almost full",
        "output-area-full" => "output tray full",
        "paused" => "paused",
        "moving-to-paused" => "pausing",
        "spool-area-full" => "spool area full",
        "connecting-to-device" => "connecting to the print engine",
        "timed-out" => "timed out",
        "stopping" => "stopping",
        "stopped-partly" => "partly stopped",
        "shutdown" => "shut down",
        "developer-low" => "developer low",
        "developer-empty" => "developer empty",
        "fuser-over-temp" => "fuser too hot",
        "fuser-under-temp" => "fuser too cold",
        "opc-near-eol" => "drum near end of life",
        "opc-life-over" => "drum at end of life",
        "other" => "something else is wrong",
        _ => "",
    };
    let text = if text.is_empty() {
        keyword.replace('-', " ")
    } else {
        text.to_string()
    };
    Reason {
        keyword: keyword.to_string(),
        severity: severity.to_string(),
        text,
    }
}

/// "iso_a4_210x297mm" -> "A4", "na_letter_8.5x11in" -> "Letter", else the
/// size part tidied up.
pub fn media_name(keyword: &str) -> String {
    let mut parts = keyword.splitn(3, '_');
    let region = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or(keyword);
    let dims = parts.next().unwrap_or("");
    let known = match (region, name) {
        ("iso", "dl") => "DL envelope".into(),
        ("iso", n) if n.len() <= 3 && n.starts_with(|c: char| c.is_ascii_alphabetic()) => {
            n.to_ascii_uppercase()
        }
        ("na", "letter") => "Letter".into(),
        ("na", "legal") => "Legal".into(),
        ("na", "executive") => "Executive".into(),
        ("na", "monarch") => "Monarch envelope".into(),
        ("na", "number-10") => "No. 10 envelope".into(),
        ("na", "foolscap") => "Foolscap".into(),
        ("jis", n) => format!("JIS {}", n.to_ascii_uppercase()),
        ("custom", _) => format!("custom {}", dims.replace("mm", " mm")),
        _ => String::new(),
    };
    if known.is_empty() {
        let mut n = name.replace('-', " ");
        if let Some(first) = n.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        if dims.is_empty() {
            n
        } else {
            format!("{n} ({dims})")
        }
    } else {
        known
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipp::decode_response;

    const RESPONSE: &[u8] = include_bytes!(
        "../../tests/fixtures/ipp/brother-dcp-l2540dw.get-printer-attributes.response.bin"
    );
    const JOBS: &[u8] =
        include_bytes!("../../tests/fixtures/ipp/brother-dcp-l2540dw.get-jobs.response.bin");

    #[test]
    fn brother_status() {
        let printer = decode_response(RESPONSE).unwrap();
        let jobs = decode_response(JOBS).unwrap();
        let s = from_responses(&printer, Some(&jobs));
        assert_eq!(s.name, "BRWEXAMPLE00001");
        assert_eq!(
            s.make_and_model.as_deref(),
            Some("Brother DCP-L2540DW series")
        );
        assert_eq!(s.state, PrinterState::Idle);
        assert!(s.reasons.is_empty());
        assert!(s.accepting_jobs);
        assert_eq!(s.queued_jobs, 0);
        assert_eq!(
            s.supplies,
            vec![Supply {
                name: "BK".into(),
                kind: "toner".into(),
                color: Some("#000000".into()),
                level: Some(100),
                low_at: Some(10),
            }]
        );
        assert_eq!(s.media_ready, vec!["iso_a4_210x297mm"]);
        assert!(!s.color);
        assert!(s.duplex);
        assert_eq!(s.pages_per_minute, Some(30));
        assert_eq!(s.max_dpi, Some(2400));
        assert_eq!(s.document_formats.len(), 3);
        assert_eq!(s.up_time, Some(2746));
        assert!(s.jobs.is_empty());
        assert_eq!(s.location, None); // empty string becomes None
    }

    #[test]
    fn reasons_and_media_read_well() {
        let r = parse_reason("media-empty-warning");
        assert_eq!(
            (r.keyword.as_str(), r.severity.as_str(), r.text.as_str()),
            ("media-empty", "warning", "out of paper")
        );
        let r = parse_reason("cover-open");
        assert_eq!(
            (r.severity.as_str(), r.text.as_str()),
            ("", "a cover is open")
        );
        assert_eq!(
            parse_reason("wifi-not-configured-report").text,
            "wifi not configured"
        );
        assert_eq!(media_name("iso_a4_210x297mm"), "A4");
        assert_eq!(media_name("na_letter_8.5x11in"), "Letter");
        assert_eq!(media_name("na_number-10_4.125x9.5in"), "No. 10 envelope");
        assert_eq!(media_name("iso_dl_110x220mm"), "DL envelope");
        assert_eq!(
            media_name("custom_max_215.9x355.6mm"),
            "custom 215.9x355.6 mm"
        );
        assert_eq!(media_name("na_index-3x5_3x5in"), "Index 3x5 (3x5in)");
    }
}
