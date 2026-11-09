//! Reading a OneTouch sign-in QR code.
//!
//! The welcome email (and the phone app's Me screen) carry a QR code that the
//! official app scans to sign in. Kagaz decodes the code here, in Rust, from a
//! grayscale frame the window hands over (a camera picture or a chosen image,
//! drawn to a canvas and reduced to one luma byte per pixel). The decoder is
//! `quircs`, pure Rust, so it works the same on every build.
//!
//! What the code holds: the OEM app encrypts the payload (AES, a key compiled
//! into its own native library), so a scanned OneTouch code is not readable
//! offline — Kagaz says so and points the user at the username and password
//! printed in the same email. Some setups instead put a plain JSON of
//! credentials in the code; those we can sign in with directly.

use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;

/// What a scanned code turned out to be.
#[derive(Serialize)]
#[serde(tag = "kind")]
pub enum QrRead {
    /// Nothing decoded in this frame; keep scanning.
    #[serde(rename = "none")]
    None,
    /// A OneTouch sign-in code whose contents are encrypted (the usual case).
    #[serde(rename = "encrypted")]
    Encrypted,
    /// A code carrying readable credentials: sign in with these.
    #[serde(rename = "credentials")]
    Credentials { username: String, password: String },
    /// A code we read but did not recognise.
    #[serde(rename = "other")]
    Other { text: String },
}

fn b64(s: &str) -> bool {
    !s.is_empty()
        && s.len() % 4 == 0
        && s.len() >= 24
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')
}

/// Make sense of a decoded QR string.
fn classify(text: &str) -> QrRead {
    let text = text.trim();
    // Plain JSON of credentials (account/pwd), the shape the OEM app parses
    // after decrypting. If a code carries it in the clear, use it.
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        let account = pick(&v, &["account", "user", "username"]);
        let pwd = pick(&v, &["pwd", "password", "passwd"]);
        if !account.is_empty() && !pwd.is_empty() {
            return QrRead::Credentials { username: account, password: pwd };
        }
    }
    // The OEM sign-in code: an AES/base64 blob, not JSON. We cannot read it
    // without the vendor's embedded key.
    if b64(text) {
        return QrRead::Encrypted;
    }
    QrRead::Other { text: text.to_string() }
}

fn pick(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// Decode a QR code from a grayscale frame: `gray` is base64 of one luma byte
/// per pixel, row-major, `width` by `height`.
#[tauri::command]
pub async fn onetouch_qr_decode(width: u32, height: u32, gray: String) -> Result<QrRead, String> {
    super::blocking(move || {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(gray.as_bytes())
            .map_err(|_| "The scan frame was not valid.".to_string())?;
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || bytes.len() != w * h {
            return Err("The scan frame size did not match.".to_string());
        }
        let mut q = quircs::Quirc::default();
        let codes = q.identify(w, h, &bytes);
        for code in codes.flatten() {
            if let Ok(decoded) = code.decode() {
                if let Ok(text) = std::str::from_utf8(&decoded.payload) {
                    return Ok(classify(text));
                }
            }
        }
        Ok(QrRead::None)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_payloads() {
        assert!(matches!(
            classify(r#"{"account":"a@b.test","pwd":"secret"}"#),
            QrRead::Credentials { .. }
        ));
        // A long base64 blob (the encrypted OEM form).
        let blob = "A".repeat(64);
        assert!(matches!(classify(&blob), QrRead::Encrypted));
        assert!(matches!(classify("https://example.test/x"), QrRead::Other { .. }));
    }
}
