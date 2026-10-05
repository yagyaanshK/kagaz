//! IPP, the Internet Printing Protocol (RFC 8010 encoding, RFC 8011
//! semantics): a binary message over HTTP POST. Just enough of it here to
//! ask a printer about itself and its jobs, and later to send it a job.

pub mod status;

use std::io::Read;
use std::time::Duration;

pub const OP_PRINT_JOB: u16 = 0x0002;
pub const OP_VALIDATE_JOB: u16 = 0x0004;
pub const OP_GET_JOBS: u16 = 0x000a;
pub const OP_GET_PRINTER_ATTRIBUTES: u16 = 0x000b;
pub const OP_IDENTIFY_PRINTER: u16 = 0x003c;

// Value tags.
pub const TAG_INTEGER: u8 = 0x21;
pub const TAG_BOOLEAN: u8 = 0x22;
pub const TAG_ENUM: u8 = 0x23;
pub const TAG_OCTETS: u8 = 0x30;
pub const TAG_DATETIME: u8 = 0x31;
pub const TAG_RESOLUTION: u8 = 0x32;
pub const TAG_RANGE: u8 = 0x33;
pub const TAG_BEG_COLLECTION: u8 = 0x34;
pub const TAG_TEXT_LANG: u8 = 0x35;
pub const TAG_NAME_LANG: u8 = 0x36;
pub const TAG_END_COLLECTION: u8 = 0x37;
pub const TAG_TEXT: u8 = 0x41;
pub const TAG_NAME: u8 = 0x42;
pub const TAG_KEYWORD: u8 = 0x44;
pub const TAG_URI: u8 = 0x45;
pub const TAG_CHARSET: u8 = 0x47;
pub const TAG_LANGUAGE: u8 = 0x48;
pub const TAG_MIME: u8 = 0x49;
pub const TAG_MEMBER_NAME: u8 = 0x4a;

// Group tags.
pub const GROUP_OPERATION: u8 = 0x01;
pub const GROUP_JOB: u8 = 0x02;
pub const GROUP_END: u8 = 0x03;
pub const GROUP_PRINTER: u8 = 0x04;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Integer(i32),
    Boolean(bool),
    Enum(i32),
    /// Every string-like tag: text, name, keyword, uri, charset, language, mime type.
    Text(String),
    Octets(Vec<u8>),
    Resolution {
        x: i32,
        y: i32,
        units: u8,
    },
    Range(i32, i32),
    Collection(Vec<Attribute>),
    /// unknown, no-value, unsupported, or something this code does not decode.
    Absent,
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::Integer(i) | Value::Enum(i) => Some(*i),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub tag: u8,
    pub name: String,
    pub values: Vec<Value>,
}

impl Attribute {
    pub fn first(&self) -> Option<&Value> {
        self.values.first()
    }
    pub fn strings(&self) -> Vec<String> {
        self.values
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    }
    pub fn member(&self, name: &str) -> Option<&Attribute> {
        self.values.iter().find_map(|v| match v {
            Value::Collection(members) => members.iter().find(|m| m.name == name),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub tag: u8,
    pub attributes: Vec<Attribute>,
}

impl Group {
    pub fn get(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub version: (u8, u8),
    pub status: u16,
    pub request_id: u32,
    pub groups: Vec<Group>,
}

impl Response {
    pub fn ok(&self) -> bool {
        self.status < 0x0100
    }
    /// The first group with `tag`.
    pub fn group(&self, tag: u8) -> Option<&Group> {
        self.groups.iter().find(|g| g.tag == tag)
    }
    pub fn groups_of(&self, tag: u8) -> impl Iterator<Item = &Group> {
        self.groups.iter().filter(move |g| g.tag == tag)
    }
    /// A printer attribute by name.
    pub fn printer(&self, name: &str) -> Option<&Attribute> {
        self.group(GROUP_PRINTER).and_then(|g| g.get(name))
    }
    /// The status-message from the operation group, when the printer sent one.
    pub fn status_message(&self) -> Option<&str> {
        self.group(GROUP_OPERATION)
            .and_then(|g| g.get("status-message"))
            .and_then(|a| a.first())
            .and_then(Value::as_str)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IppError {
    #[error("cannot reach the printer: {0}")]
    Transport(String),
    #[error("the printer sent something that is not IPP: {0}")]
    Malformed(String),
    #[error("the printer refused ({0})")]
    Status(String),
    #[error("{0} does not offer IPP")]
    NotSupported(String),
}

/// One attribute to send, with its tag and raw values.
#[derive(Debug, Clone)]
pub struct OutAttribute {
    pub tag: u8,
    pub name: String,
    pub values: Vec<Vec<u8>>,
}

pub fn out(tag: u8, name: &str, values: &[&str]) -> OutAttribute {
    OutAttribute {
        tag,
        name: name.to_string(),
        values: values.iter().map(|v| v.as_bytes().to_vec()).collect(),
    }
}

pub fn out_int(tag: u8, name: &str, value: i32) -> OutAttribute {
    OutAttribute {
        tag,
        name: name.to_string(),
        values: vec![value.to_be_bytes().to_vec()],
    }
}

/// Encode a request: version 2.0, the standard operation attributes
/// (charset, language, printer-uri) followed by `operation` extras, then
/// `job` attributes in a job group when there are any, then `document`.
pub fn encode_request(
    op: u16,
    request_id: u32,
    printer_uri: &str,
    operation: &[OutAttribute],
    job: &[OutAttribute],
    document: &[u8],
) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&[2, 0]);
    b.extend_from_slice(&op.to_be_bytes());
    b.extend_from_slice(&request_id.to_be_bytes());
    b.push(GROUP_OPERATION);
    push_attr(&mut b, &out(TAG_CHARSET, "attributes-charset", &["utf-8"]));
    push_attr(
        &mut b,
        &out(TAG_LANGUAGE, "attributes-natural-language", &["en"]),
    );
    push_attr(&mut b, &out(TAG_URI, "printer-uri", &[printer_uri]));
    for a in operation {
        push_attr(&mut b, a);
    }
    if !job.is_empty() {
        b.push(GROUP_JOB);
        for a in job {
            push_attr(&mut b, a);
        }
    }
    b.push(GROUP_END);
    b.extend_from_slice(document);
    b
}

fn push_attr(b: &mut Vec<u8>, a: &OutAttribute) {
    for (i, v) in a.values.iter().enumerate() {
        b.push(a.tag);
        let name: &[u8] = if i == 0 { a.name.as_bytes() } else { &[] };
        b.extend_from_slice(&(name.len() as u16).to_be_bytes());
        b.extend_from_slice(name);
        b.extend_from_slice(&(v.len() as u16).to_be_bytes());
        b.extend_from_slice(v);
    }
}

/// Decode a response, collections included.
pub fn decode_response(data: &[u8]) -> Result<Response, IppError> {
    if data.len() < 9 {
        return Err(IppError::Malformed("shorter than a header".into()));
    }
    let status = u16::from_be_bytes([data[2], data[3]]);
    let request_id = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let mut groups: Vec<Group> = Vec::new();
    // Collections under construction: each frame is the member list of one
    // open collection.
    let mut stack: Vec<Vec<Attribute>> = Vec::new();
    let mut pos = 8;
    while pos < data.len() {
        let tag = data[pos];
        pos += 1;
        if tag == GROUP_END {
            break;
        }
        if tag < 0x10 {
            groups.push(Group {
                tag,
                attributes: Vec::new(),
            });
            continue;
        }
        let read_u16 = |p: usize| -> Result<usize, IppError> {
            data.get(p..p + 2)
                .map(|s| u16::from_be_bytes([s[0], s[1]]) as usize)
                .ok_or_else(|| IppError::Malformed("truncated attribute".into()))
        };
        let name_len = read_u16(pos)?;
        pos += 2;
        let name = data
            .get(pos..pos + name_len)
            .ok_or_else(|| IppError::Malformed("truncated attribute name".into()))?;
        let name = String::from_utf8_lossy(name).to_string();
        pos += name_len;
        let value_len = read_u16(pos)?;
        pos += 2;
        let raw = data
            .get(pos..pos + value_len)
            .ok_or_else(|| IppError::Malformed("truncated attribute value".into()))?;
        pos += value_len;

        // Where a new attribute or an additional value goes.
        let group = groups
            .last_mut()
            .ok_or_else(|| IppError::Malformed("attribute before any group".into()))?;
        if !name.is_empty() && stack.is_empty() {
            group.attributes.push(Attribute {
                tag,
                name: name.clone(),
                values: Vec::new(),
            });
        }
        match tag {
            TAG_BEG_COLLECTION => stack.push(Vec::new()),
            TAG_MEMBER_NAME => {
                if let Some(frame) = stack.last_mut() {
                    frame.push(Attribute {
                        tag,
                        name: String::from_utf8_lossy(raw).to_string(),
                        values: Vec::new(),
                    });
                }
            }
            TAG_END_COLLECTION => {
                let members = stack.pop().unwrap_or_default();
                let target = match stack.last_mut() {
                    Some(parent) => parent.last_mut(),
                    None => group.attributes.last_mut(),
                };
                if let Some(t) = target {
                    t.values.push(Value::Collection(members));
                }
            }
            _ => {
                let value = decode_value(tag, raw);
                let target = match stack.last_mut() {
                    Some(frame) => frame.last_mut(),
                    None => group.attributes.last_mut(),
                };
                if let Some(t) = target {
                    t.values.push(value);
                }
            }
        }
    }
    Ok(Response {
        version: (data[0], data[1]),
        status,
        request_id,
        groups,
    })
}

fn decode_value(tag: u8, raw: &[u8]) -> Value {
    let int = |b: &[u8]| i32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    match tag {
        TAG_INTEGER if raw.len() == 4 => Value::Integer(int(raw)),
        TAG_ENUM if raw.len() == 4 => Value::Enum(int(raw)),
        TAG_BOOLEAN if raw.len() == 1 => Value::Boolean(raw[0] != 0),
        TAG_RESOLUTION if raw.len() == 9 => Value::Resolution {
            x: int(&raw[0..4]),
            y: int(&raw[4..8]),
            units: raw[8],
        },
        TAG_RANGE if raw.len() == 8 => Value::Range(int(&raw[0..4]), int(&raw[4..8])),
        TAG_TEXT_LANG | TAG_NAME_LANG => {
            // language-length, language, text-length, text
            if raw.len() >= 4 {
                let ll = u16::from_be_bytes([raw[0], raw[1]]) as usize;
                let start = 2 + ll + 2;
                if raw.len() >= start {
                    return Value::Text(String::from_utf8_lossy(&raw[start..]).to_string());
                }
            }
            Value::Absent
        }
        TAG_TEXT | TAG_NAME | TAG_KEYWORD | TAG_URI | TAG_CHARSET | TAG_LANGUAGE | TAG_MIME
        | 0x46 => Value::Text(String::from_utf8_lossy(raw).to_string()),
        TAG_OCTETS | TAG_DATETIME => Value::Octets(raw.to_vec()),
        _ => Value::Absent,
    }
}

/// POST an IPP request to `http_url` and decode the reply. HTTP error
/// statuses are decoded too when they carry IPP, since some printers answer
/// refusals that way.
pub fn call(http_url: &str, request: &[u8], timeout: Duration) -> Result<Response, IppError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(timeout)
        .build();
    let response = match agent
        .post(http_url)
        .set("Content-Type", "application/ipp")
        .send_bytes(request)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let ct = r.header("Content-Type").unwrap_or_default().to_string();
            if ct.contains("application/ipp") {
                r
            } else {
                return Err(IppError::Transport(format!(
                    "HTTP {code} {}",
                    r.status_text()
                )));
            }
        }
        Err(ureq::Error::Transport(t)) => return Err(IppError::Transport(t.to_string())),
    };
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| IppError::Transport(e.to_string()))?;
    decode_response(&bytes)
}

/// The `http://` URL and `ipp://` URI for a device's IPP service.
pub fn urls_for(device: &crate::Device) -> Result<(String, String), IppError> {
    let svc = device
        .services
        .iter()
        .find(|s| s.protocol == crate::Protocol::Ipp)
        .ok_or_else(|| IppError::NotSupported(device.name.clone()))?;
    let ip = device
        .addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or(device.addresses.first())
        .ok_or_else(|| IppError::NotSupported(device.name.clone()))?;
    let host = match ip {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
    };
    let port = svc.port.unwrap_or(631);
    let path = svc
        .endpoint
        .as_deref()
        .map(|p| p.trim_matches('/'))
        .filter(|p| !p.is_empty())
        .unwrap_or("ipp/print");
    Ok((
        format!("http://{host}:{port}/{path}"),
        format!("ipp://{host}:{port}/{path}"),
    ))
}

/// A readable form of an IPP status code.
pub fn status_text(status: u16) -> String {
    let known = match status {
        0x0000 => "ok",
        0x0001 => "ok, some attributes ignored",
        0x0400 => "bad request",
        0x0401 => "forbidden",
        0x0402 => "not authenticated",
        0x0403 => "not authorized",
        0x0404 => "not possible",
        0x0405 => "timeout",
        0x0406 => "not found",
        0x0407 => "gone",
        0x0408 => "request too large",
        0x0409 => "request value too long",
        0x040a => "document format not supported",
        0x040b => "attributes or values not supported",
        0x040c => "uri scheme not supported",
        0x040d => "charset not supported",
        0x040e => "conflicting attributes",
        0x0500 => "internal error",
        0x0501 => "operation not supported",
        0x0502 => "service unavailable",
        0x0503 => "version not supported",
        0x0504 => "device error",
        0x0505 => "temporary error",
        0x0506 => "not accepting jobs",
        0x0507 => "printer busy",
        0x0508 => "job cancelled",
        _ => "",
    };
    if known.is_empty() {
        format!("status 0x{status:04x}")
    } else {
        format!("{known} (0x{status:04x})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded from the Brother DCP-L2540DW on 2026-10-05; see tests/fixtures/ipp/README.md.
    const REQUEST: &[u8] = include_bytes!(
        "../../tests/fixtures/ipp/brother-dcp-l2540dw.get-printer-attributes.request.bin"
    );
    const RESPONSE: &[u8] = include_bytes!(
        "../../tests/fixtures/ipp/brother-dcp-l2540dw.get-printer-attributes.response.bin"
    );
    const JOBS: &[u8] =
        include_bytes!("../../tests/fixtures/ipp/brother-dcp-l2540dw.get-jobs.response.bin");

    #[test]
    fn encodes_the_recorded_request() {
        let body = encode_request(
            OP_GET_PRINTER_ATTRIBUTES,
            0x4b41,
            "ipp://198.51.100.167:631/ipp/print",
            &[out(TAG_KEYWORD, "requested-attributes", &["all"])],
            &[],
            &[],
        );
        assert_eq!(body, REQUEST);
    }

    #[test]
    fn decodes_the_brother_attributes() {
        let r = decode_response(RESPONSE).unwrap();
        assert_eq!(r.version, (2, 0));
        assert!(r.ok());
        assert_eq!(r.request_id, 0x4b41);
        assert_eq!(r.groups.len(), 2);
        assert_eq!(
            r.printer("printer-make-and-model")
                .unwrap()
                .first()
                .unwrap()
                .as_str(),
            Some("Brother DCP-L2540DW series")
        );
        assert_eq!(
            r.printer("printer-state")
                .unwrap()
                .first()
                .unwrap()
                .as_i32(),
            Some(3)
        );
        assert_eq!(
            r.printer("marker-levels")
                .unwrap()
                .first()
                .unwrap()
                .as_i32(),
            Some(100)
        );
        assert_eq!(r.printer("marker-names").unwrap().strings(), vec!["BK"]);
        assert_eq!(r.printer("sides-supported").unwrap().values.len(), 3);
        assert_eq!(
            r.printer("printer-resolution-supported").unwrap().first(),
            Some(&Value::Resolution {
                x: 600,
                y: 600,
                units: 3
            })
        );
        assert_eq!(
            r.printer("copies-supported").unwrap().first(),
            Some(&Value::Range(1, 99))
        );
        assert_eq!(
            r.printer("printer-is-accepting-jobs")
                .unwrap()
                .first()
                .unwrap()
                .as_bool(),
            Some(true)
        );
        // Collections: media-col-ready has a nested media-size collection.
        let ready = r.printer("media-col-ready").unwrap();
        assert_eq!(
            ready.member("media-source").unwrap().strings(),
            vec!["tray-1"]
        );
        let size = ready.member("media-size").unwrap();
        assert_eq!(
            size.member("x-dimension")
                .unwrap()
                .first()
                .unwrap()
                .as_i32(),
            Some(21000)
        );
        assert_eq!(
            size.member("y-dimension")
                .unwrap()
                .first()
                .unwrap()
                .as_i32(),
            Some(29700)
        );
        // 13 fixed sizes plus a custom range in media-size-supported.
        assert_eq!(r.printer("media-size-supported").unwrap().values.len(), 14);
        // The attribute after the collections must still be parsed correctly.
        assert_eq!(
            r.printer("printer-kind").unwrap().strings(),
            vec!["document", "envelope", "label"]
        );
    }

    #[test]
    fn decodes_an_empty_job_list() {
        let r = decode_response(JOBS).unwrap();
        assert!(r.ok());
        assert_eq!(r.groups_of(GROUP_JOB).count(), 0);
        assert_eq!(decode_response(&RESPONSE[..5]).ok(), None);
        assert!(decode_response(&RESPONSE[..40]).is_err());
    }

    #[test]
    fn status_codes_read_well() {
        assert_eq!(status_text(0), "ok (0x0000)");
        assert_eq!(status_text(0x0506), "not accepting jobs (0x0506)");
        assert_eq!(status_text(0x0999), "status 0x0999");
    }
}
