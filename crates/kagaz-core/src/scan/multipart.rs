//! MIME multipart parsing, enough for the MTOM/XOP replies WSD scanners
//! send: a SOAP part followed by the image part.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// Header names lower-cased.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Part {
    pub fn content_type(&self) -> &str {
        self.headers
            .get("content-type")
            .map(String::as_str)
            .unwrap_or("")
    }
    pub fn content_id(&self) -> Option<&str> {
        self.headers
            .get("content-id")
            .map(|s| s.trim_matches(|c| c == '<' || c == '>'))
    }
}

/// The `boundary` parameter of a Content-Type header.
pub fn boundary_of(content_type: &str) -> Option<String> {
    for param in content_type.split(';').skip(1) {
        let (k, v) = param.trim().split_once('=')?;
        if k.trim().eq_ignore_ascii_case("boundary") {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Split a multipart body into its parts.
pub fn parse(body: &[u8], boundary: &str) -> Vec<Part> {
    let delimiter = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut pos = match find(body, &delimiter, 0) {
        Some(p) => p + delimiter.len(),
        None => return parts,
    };
    loop {
        // After a delimiter: "--" closes, else CRLF (or LF) then the part.
        if body[pos..].starts_with(b"--") {
            break;
        }
        pos = skip_newline(body, pos);
        let Some(next) = find(body, &delimiter, pos) else {
            break;
        };
        // The CRLF before the next delimiter belongs to the delimiter.
        let mut end = next;
        if end >= 2 && &body[end - 2..end] == b"\r\n" {
            end -= 2;
        } else if end >= 1 && body[end - 1] == b'\n' {
            end -= 1;
        }
        if let Some(part) = parse_part(&body[pos..end]) {
            parts.push(part);
        }
        pos = next + delimiter.len();
        if pos >= body.len() {
            break;
        }
    }
    parts
}

fn parse_part(raw: &[u8]) -> Option<Part> {
    let (header_len, body_start) = if let Some(p) = find(raw, b"\r\n\r\n", 0) {
        (p, p + 4)
    } else {
        let p = find(raw, b"\n\n", 0)?;
        (p, p + 2)
    };
    let mut headers = BTreeMap::new();
    for line in String::from_utf8_lossy(&raw[..header_len]).lines() {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Some(Part {
        headers,
        body: raw[body_start..].to_vec(),
    })
}

fn skip_newline(body: &[u8], pos: usize) -> usize {
    if body[pos..].starts_with(b"\r\n") {
        pos + 2
    } else if body[pos..].starts_with(b"\n") {
        pos + 1
    } else {
        pos
    }
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_boundary_with_and_without_quotes() {
        assert_eq!(
            boundary_of(r#"Multipart/Related; boundary="abc-123"; type="application/xop+xml""#)
                .as_deref(),
            Some("abc-123")
        );
        assert_eq!(
            boundary_of("multipart/related; type=application/xop+xml; boundary=xyz").as_deref(),
            Some("xyz")
        );
        assert_eq!(boundary_of("application/soap+xml"), None);
    }

    #[test]
    fn splits_two_parts_and_strips_trailing_crlf() {
        let body = b"\r\n--B\r\nContent-Type: application/xop+xml\r\nContent-ID: <soap@uuid>\r\n\r\n<xml/>\r\n--B\r\nContent-Type: image/jpeg\r\n\r\n\xff\xd8\xff\xd9\r\n--B--";
        let parts = parse(body, "B");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].content_id(), Some("soap@uuid"));
        assert_eq!(parts[0].body, b"<xml/>");
        assert_eq!(parts[1].content_type(), "image/jpeg");
        assert_eq!(parts[1].body, b"\xff\xd8\xff\xd9");
    }

    #[test]
    fn tolerates_bare_lf_and_missing_close() {
        let body = b"--B\nContent-Type: text/plain\n\nhello\n--B\nContent-Type: image/png\n\nPNG";
        let parts = parse(body, "B");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].body, b"hello");
        assert!(parse(b"nothing here", "B").is_empty());
    }
}
