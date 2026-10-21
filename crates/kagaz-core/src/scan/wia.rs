//! Scanning through the installed vendor driver on Windows, via WIA
//! (Windows Image Acquisition), the scanner layer every Windows driver plugs
//! into. This is the Windows twin of `sane`: the driver does the talking,
//! Kagaz picks from the options the driver itself reports and files the
//! pages. WIA is reached through its scripting objects in PowerShell, which
//! ships with every Windows, the way `sane` runs `scanimage`.
//!
//! Building the scripts and reading their answers is plain text work and
//! is tested on every platform; running them needs Windows.

use super::{ColorMode, ScanError, ScanRequest, Source};
use crate::Device;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A scanner as WIA lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WiaDevice {
    /// WIA's device id, e.g. `{6BDD1FC6-810F-11D0-BEC7-08002BE2092F}\0001`.
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub description: String,
    /// Where the driver reaches it; for network scanners often an address.
    #[serde(default)]
    pub port: String,
}

/// What the driver offers, as WIA reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WiaCaps {
    pub resolutions: Vec<u32>,
    pub flatbed: bool,
    pub feeder: bool,
    pub duplex: bool,
    /// "Color", "Gray", "BlackWhite": the intents the driver accepts.
    pub colors: Vec<String>,
}

// WIA property ids (wiadef.h).
const DOCUMENT_HANDLING_CAPABILITIES: u32 = 3086;
const DOCUMENT_HANDLING_SELECT: u32 = 3088;
const CURRENT_INTENT: u32 = 6146;
const HORIZONTAL_RESOLUTION: u32 = 6147;
const VERTICAL_RESOLUTION: u32 = 6148;
const HORIZONTAL_EXTENT: u32 = 6151;
const VERTICAL_EXTENT: u32 = 6152;
// Document handling flags.
const FEEDER: u32 = 1;
const FLATBED: u32 = 2;
const DUPLEX: u32 = 4;
// Intents.
const INTENT_COLOR: u32 = 1;
const INTENT_GRAY: u32 = 2;
const INTENT_TEXT: u32 = 4;
/// WIA_ERROR_PAPER_EMPTY as PowerShell reports an HRESULT.
const PAPER_EMPTY: i64 = -2145320957;
const FORMAT_PNG: &str = "{B96B3CAF-0728-11D3-9D7B-0000F81EF32E}";
const FORMAT_JPEG: &str = "{B96B3CAE-0728-11D3-9D7B-0000F81EF32E}";

/// PowerShell single-quoted string literal.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Lists WIA scanners (type 1) as JSON.
pub fn list_script() -> String {
    r#"$ErrorActionPreference = 'Stop'
$m = New-Object -ComObject WIA.DeviceManager
$out = @()
foreach ($i in $m.DeviceInfos) {
  if ($i.Type -ne 1) { continue }
  $p = @{}
  foreach ($x in $i.Properties) { $p[$x.Name] = [string]$x.Value }
  $out += [pscustomobject]@{
    id = $i.DeviceID; name = $p['Name']; manufacturer = $p['Manufacturer']
    description = $p['Description']; port = $p['Port']
  }
}
ConvertTo-Json -InputObject @($out) -Compress
"#
    .to_string()
}

fn connect_script(id: &str) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$m = New-Object -ComObject WIA.DeviceManager
$info = $null
foreach ($i in $m.DeviceInfos) {{ if ($i.DeviceID -eq {id}) {{ $info = $i }} }}
if ($null -eq $info) {{ throw 'KAGAZ: the scanner is no longer listed by Windows' }}
$dev = $info.Connect()
function Find-Prop($props, $id) {{ foreach ($p in $props) {{ if ($p.PropertyID -eq $id) {{ return $p }} }}; return $null }}
"#,
        id = quote(id)
    )
}

/// Reports the driver's options as JSON.
pub fn caps_script(id: &str) -> String {
    connect_script(id)
        + &format!(
            r#"$item = $dev.Items.Item(1)
$handling = 0
$h = Find-Prop $dev.Properties {DOCUMENT_HANDLING_CAPABILITIES}
if ($null -ne $h) {{ $handling = [int]$h.Value }}
$res = @()
$r = Find-Prop $item.Properties {HORIZONTAL_RESOLUTION}
if ($null -ne $r) {{
  if ($r.SubType -eq 2) {{ foreach ($v in $r.SubTypeValues) {{ $res += [int]$v }} }}
  elseif ($r.SubType -eq 1) {{
    foreach ($v in 75,100,150,200,300,400,600,1200) {{ if ($v -ge $r.SubTypeMin -and $v -le $r.SubTypeMax) {{ $res += $v }} }}
  }}
  else {{ $res += [int]$r.Value }}
}}
$intents = 0
$ip = Find-Prop $item.Properties {CURRENT_INTENT}
if ($null -ne $ip) {{
  if ($ip.SubType -eq 2 -or $ip.SubType -eq 3) {{ foreach ($v in $ip.SubTypeValues) {{ $intents = $intents -bor [int]$v }} }}
  else {{ $intents = 7 }}
}}
ConvertTo-Json -Compress -InputObject ([pscustomobject]@{{ handling = $handling; resolutions = @($res); intents = $intents }})
"#
        )
}

#[derive(Debug, Deserialize)]
struct RawCaps {
    #[serde(default)]
    handling: u32,
    #[serde(default)]
    resolutions: Vec<u32>,
    #[serde(default)]
    intents: u32,
}

/// Read the JSON from [`caps_script`].
pub fn parse_caps(json: &str) -> Result<WiaCaps, ScanError> {
    let raw: RawCaps = serde_json::from_str(json.trim())
        .map_err(|e| ScanError::Protocol(format!("WIA capabilities: {e}")))?;
    let mut resolutions = raw.resolutions;
    resolutions.sort_unstable();
    resolutions.dedup();
    let intents = if raw.intents == 0 { 7 } else { raw.intents };
    let mut colors = Vec::new();
    if intents & INTENT_COLOR != 0 {
        colors.push("Color".to_string());
    }
    if intents & INTENT_GRAY != 0 {
        colors.push("Gray".to_string());
    }
    if intents & INTENT_TEXT != 0 {
        colors.push("BlackWhite".to_string());
    }
    Ok(WiaCaps {
        resolutions,
        // A driver that reports no handling flags has only a glass.
        flatbed: raw.handling == 0 || raw.handling & FLATBED != 0,
        feeder: raw.handling & FEEDER != 0,
        duplex: raw.handling & DUPLEX != 0,
        colors,
    })
}

/// Read the JSON from [`list_script`].
pub fn parse_device_list(json: &str) -> Result<Vec<WiaDevice>, ScanError> {
    let json = json.trim();
    if json.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(json).map_err(|e| ScanError::Protocol(format!("WIA device list: {e}")))
}

fn letters_and_digits(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The WIA scanner that is `device`: same address in its port, else its
/// model in the WIA name.
pub fn device_for<'a>(wia: &'a [WiaDevice], device: &Device) -> Option<&'a WiaDevice> {
    if let Some(d) = wia.iter().find(|w| {
        device
            .addresses
            .iter()
            .any(|a| w.port.contains(&a.to_string()))
    }) {
        return Some(d);
    }
    let model = device
        .model
        .as_deref()
        .map(|m| {
            letters_and_digits(
                m.trim_end_matches(" series")
                    .trim_start_matches(device.manufacturer.as_deref().unwrap_or("")),
            )
        })
        .filter(|m| m.len() >= 4)?;
    wia.iter()
        .find(|w| letters_and_digits(&format!("{} {}", w.name, w.description)).contains(&model))
}

/// What the WIA scan uses for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub dpi: u32,
    pub intent: u32,
    /// Document handling select: feeder, flatbed, or feeder + duplex.
    pub handling: u32,
    /// Scan area in thousandths of an inch; None = the driver's whole area.
    pub size_mils: Option<(u32, u32)>,
    pub png: bool,
}

/// Fit a request to what the driver offers.
pub fn choose(caps: &WiaCaps, req: &ScanRequest) -> Result<Choice, ScanError> {
    let dpi = if caps.resolutions.is_empty() || caps.resolutions.contains(&req.dpi) {
        req.dpi
    } else {
        return Err(ScanError::Refused(format!(
            "the driver does not scan at {} dpi; it offers {}",
            req.dpi,
            caps.resolutions
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };
    let handling = match req.source {
        Source::Glass => FLATBED,
        Source::Feeder => FEEDER,
        Source::FeederDuplex => FEEDER | DUPLEX,
        Source::Auto => 0,
    };
    if handling & FEEDER != 0 && !caps.feeder {
        return Err(ScanError::Refused("this scanner has no feeder".into()));
    }
    if handling & DUPLEX != 0 && !caps.duplex {
        return Err(ScanError::Refused(
            "this scanner cannot scan both sides".into(),
        ));
    }
    let intent = match req.color {
        ColorMode::Color => INTENT_COLOR,
        ColorMode::Gray => INTENT_GRAY,
        ColorMode::BlackWhite => INTENT_TEXT,
    };
    Ok(Choice {
        dpi,
        intent,
        handling,
        size_mils: req.area_mils(),
        png: req.color == ColorMode::BlackWhite,
    })
}

/// Scans pages into `dir`, printing "PAGE <n> <path>" as each lands.
pub fn scan_script(id: &str, c: &Choice, dir: &Path) -> String {
    let (w, h) = c.size_mils.unwrap_or((0, 0));
    let format = if c.png { FORMAT_PNG } else { FORMAT_JPEG };
    let ext = if c.png { "png" } else { "jpg" };
    connect_script(id)
        + &format!(
            r#"function Set-Prop($props, $id, $value) {{
  $p = Find-Prop $props $id
  if ($null -eq $p) {{ return }}
  if ($p.SubType -eq 1 -and $value -gt $p.SubTypeMax) {{ $value = $p.SubTypeMax }}
  try {{ $p.Value = $value }} catch {{ }}
}}
$handling = {handling}
if ($handling -ne 0) {{ Set-Prop $dev.Properties {DOCUMENT_HANDLING_SELECT} $handling }}
$item = $dev.Items.Item(1)
Set-Prop $item.Properties {CURRENT_INTENT} {intent}
Set-Prop $item.Properties {HORIZONTAL_RESOLUTION} {dpi}
Set-Prop $item.Properties {VERTICAL_RESOLUTION} {dpi}
if ({w} -gt 0) {{
  Set-Prop $item.Properties {HORIZONTAL_EXTENT} ([int]({w} * {dpi} / 1000))
  Set-Prop $item.Properties {VERTICAL_EXTENT} ([int]({h} * {dpi} / 1000))
}}
$n = 0
while ($true) {{
  try {{ $img = $item.Transfer({format}) }}
  catch {{
    if ($_.Exception.HResult -eq {PAPER_EMPTY}) {{
      if ($n -gt 0) {{ break }}
      Write-Output 'EMPTY'
      exit 3
    }}
    throw
  }}
  $n++
  $path = Join-Path {dir} ('page{{0:D3}}.{ext}' -f $n)
  $img.SaveFile($path)
  Write-Output ("PAGE $n " + $path)
  if (($handling -band {FEEDER}) -eq 0) {{ break }}
}}
"#,
            handling = c.handling,
            intent = c.intent,
            dpi = c.dpi,
            format = quote(format),
            dir = quote(&dir.display().to_string()),
        )
}

#[cfg(windows)]
mod run {
    use super::*;
    use crate::scan::{Event, Page};
    use std::io::{BufRead, BufReader};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn powershell(script: &str) -> Command {
        let utf16: Vec<u8> = script
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, utf16);
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .creation_flags(CREATE_NO_WINDOW);
        cmd
    }

    fn failure(stderr: &[u8]) -> ScanError {
        let text = String::from_utf8_lossy(stderr);
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("WIA failed")
            .trim_start_matches("KAGAZ: ")
            .to_string();
        if line.contains("busy") || line.contains("0x80210006") {
            ScanError::Busy(line)
        } else {
            ScanError::Transport(line)
        }
    }

    fn output(script: &str) -> Result<String, ScanError> {
        let out = powershell(script)
            .output()
            .map_err(|e| ScanError::NotSupported(format!("cannot run PowerShell for WIA ({e})")))?;
        if !out.status.success() {
            return Err(failure(&out.stderr));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn available() -> bool {
        true
    }

    pub fn list_devices() -> Result<Vec<WiaDevice>, ScanError> {
        parse_device_list(&output(&list_script())?)
    }

    pub fn capabilities(id: &str) -> Result<WiaCaps, ScanError> {
        parse_caps(&output(&caps_script(id))?)
    }

    pub fn scan(
        id: &str,
        caps: &WiaCaps,
        req: &ScanRequest,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<Vec<Page>, ScanError> {
        let choice = choose(caps, req)?;
        let dir = std::env::temp_dir().join(format!("kagaz-wia-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).map_err(|e| ScanError::Transport(e.to_string()))?;
        on_event(Event::Starting {
            source: req.source,
            dpi: choice.dpi,
        });
        let result = (|| {
            let mut child = powershell(&scan_script(id, &choice, &dir))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| ScanError::NotSupported(format!("cannot run PowerShell ({e})")))?;
            let mut pages = Vec::new();
            let mut empty = false;
            let stdout = child.stdout.take().expect("piped");
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.trim() == "EMPTY" {
                    empty = true;
                    continue;
                }
                let Some(rest) = line.strip_prefix("PAGE ") else {
                    continue;
                };
                let Some((_, path)) = rest.split_once(' ') else {
                    continue;
                };
                let data = std::fs::read(path).map_err(|e| ScanError::Transport(e.to_string()))?;
                on_event(Event::Page {
                    number: pages.len() + 1,
                    bytes: data.len(),
                });
                pages.push(Page {
                    data,
                    mime: if choice.png {
                        "image/png"
                    } else {
                        "image/jpeg"
                    },
                    dpi: choice.dpi,
                    color: req.color,
                });
            }
            let out = child
                .wait_with_output()
                .map_err(|e| ScanError::Transport(e.to_string()))?;
            if empty {
                return Err(ScanError::FeederEmpty);
            }
            if !out.status.success() {
                return Err(failure(&out.stderr));
            }
            Ok(pages)
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }
}

#[cfg(not(windows))]
mod run {
    use super::*;
    use crate::scan::{Event, Page};

    pub fn available() -> bool {
        false
    }

    fn windows_only() -> ScanError {
        ScanError::NotSupported("WIA is part of Windows".into())
    }

    pub fn list_devices() -> Result<Vec<WiaDevice>, ScanError> {
        Err(windows_only())
    }

    pub fn capabilities(_: &str) -> Result<WiaCaps, ScanError> {
        Err(windows_only())
    }

    pub fn scan(
        _: &str,
        _: &WiaCaps,
        _: &ScanRequest,
        _: &mut dyn FnMut(Event),
    ) -> Result<Vec<Page>, ScanError> {
        Err(windows_only())
    }
}

pub use run::{available, capabilities, list_devices, scan};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Paper;

    #[test]
    fn reads_the_device_list() {
        let json = r#"[{"id":"{6BDD1FC6-810F-11D0-BEC7-08002BE2092F}\\0001","name":"Example LX-200 LAN","manufacturer":"Example","description":"Example LX-200 LAN","port":"192.0.2.40"}]"#;
        let list = parse_device_list(json).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Example LX-200 LAN");
        assert!(parse_device_list("").unwrap().is_empty());
        assert!(parse_device_list("[]").unwrap().is_empty());
    }

    #[test]
    fn finds_the_device_by_address_then_model() {
        let list = vec![
            WiaDevice {
                id: "a".into(),
                name: "Other Scanner".into(),
                manufacturer: String::new(),
                description: String::new(),
                port: "192.0.2.41".into(),
            },
            WiaDevice {
                id: "b".into(),
                name: "Example LX-200 LAN".into(),
                manufacturer: "Example".into(),
                description: String::new(),
                port: "WSD-1234".into(),
            },
        ];
        let mut d = Device {
            manufacturer: Some("Example".into()),
            model: Some("Example LX-200 series".into()),
            ..Default::default()
        };
        assert_eq!(device_for(&list, &d).unwrap().id, "b");
        d.addresses = vec!["192.0.2.41".parse().unwrap()];
        assert_eq!(device_for(&list, &d).unwrap().id, "a");
        d.addresses.clear();
        d.model = Some("ZZ".into());
        assert!(device_for(&list, &d).is_none());
    }

    #[test]
    fn reads_the_capabilities() {
        let c =
            parse_caps(r#"{"handling":7,"resolutions":[300,100,600,300],"intents":3}"#).unwrap();
        assert_eq!(c.resolutions, vec![100, 300, 600]);
        assert!(c.flatbed && c.feeder && c.duplex);
        assert_eq!(c.colors, vec!["Color", "Gray"]);
        let glass = parse_caps(r#"{"handling":0,"resolutions":[300],"intents":0}"#).unwrap();
        assert!(glass.flatbed && !glass.feeder);
        assert_eq!(glass.colors.len(), 3);
    }

    #[test]
    fn fits_a_request_to_the_driver() {
        let caps = WiaCaps {
            resolutions: vec![150, 300],
            flatbed: true,
            feeder: false,
            duplex: false,
            colors: vec!["Color".into()],
        };
        let mut req = ScanRequest {
            source: Source::Glass,
            dpi: 300,
            color: ColorMode::Gray,
            paper: Paper::A4,
            engine: crate::scan::Engine::Driver,
            half_page: false,
            cancel: Default::default(),
        };
        let c = choose(&caps, &req).unwrap();
        assert_eq!((c.dpi, c.intent, c.handling), (300, INTENT_GRAY, FLATBED));
        assert!(!c.png);
        req.dpi = 600;
        assert!(choose(&caps, &req).is_err());
        req.dpi = 300;
        req.source = Source::Feeder;
        assert!(choose(&caps, &req).is_err());
    }

    #[test]
    fn the_scan_script_quotes_what_it_is_given() {
        let c = Choice {
            dpi: 300,
            intent: INTENT_COLOR,
            handling: FEEDER,
            size_mils: Some((8268, 11693)),
            png: false,
        };
        let s = scan_script("dev'id", &c, Path::new("C:\\Temp\\it's"));
        assert!(s.contains("'dev''id'"));
        assert!(s.contains("'C:\\Temp\\it''s'"));
        assert!(s.contains(FORMAT_JPEG));
        assert!(s.contains("$handling = 1"));
        assert!(s.contains("[int](8268 * 300 / 1000)"));
    }
}
