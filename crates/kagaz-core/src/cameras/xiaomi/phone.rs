//! Signing in with a phone number and a texted code, no password, the way
//! Xiaomi's own sign-in page does it ("sign in with a verification code").
//! Phones signed in to Mi Home this way are what most accounts have; it
//! avoids the password route's extra checks for a new device.
//!
//! The page sends the number wrapped for Xiaomi: AES-128-CBC under a fresh
//! key, the key itself sealed with the page's public RSA key and sent in an
//! `EUI` header.
//!
//! The pass token it ends with is for the whole account, so
//! [`Session::connect`] opens the Mi Home cloud with it as usual.

use super::account::{describe, enc, now_secs, str_field, unexpected, write_private, Client, Jar, Session, Step, Trace, XiaomiError};
use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use base64::Engine;
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

const ACCOUNT: &str = "https://account.xiaomi.com";
const SID: &str = "passport";
const LOCALE: &str = "en_US";
const BROWSER: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
/// The sign-in page's public key for the wrapped fields (RSA 1024,
/// SubjectPublicKeyInfo DER, as the page carries it).
const PAGE_KEY: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCYEVrK/4Mahiv0pUJgTybx4J9P5dUT/Y0PuwMbk+gMU+jrZnBiXGv6/hCH1avIhoBcE535F8nJQQN3UavZdFkYidsoXuEnat3+eVTp3FslyhRwIBDF09v4vDhRtxFOT+R7uH7h/mzmyA2/+lfIMWGIrffXprYizbV76+YQKhoqFQIDAQAB";

/// A phone sign-in in progress; kept between CLI runs like [`super::Login`].
#[derive(Serialize, Deserialize)]
pub struct PhoneLogin {
    /// The account name Xiaomi takes: `+<country><number>`, or the bare
    /// number for China.
    user: String,
    calling: String,
    national: String,
    device_id: String,
    jar: Jar,
    #[serde(default)]
    policy: String,
    started: u64,
}

impl PhoneLogin {
    /// `phone` with its country code: `+91 98765 43210`, `+44 7700 900123`.
    pub fn new(phone: &str) -> Result<PhoneLogin, XiaomiError> {
        let (calling, national) = split_number(phone).ok_or_else(|| {
            XiaomiError::Refused("write the phone number with its country code, such as +91 98765 43210".into())
        })?;
        let user = if calling == "+86" { national.clone() } else { format!("{calling}{national}") };
        Ok(PhoneLogin {
            user,
            calling,
            national,
            device_id: format!("wb_{}", uuid::Uuid::new_v4()),
            jar: Jar::default(),
            policy: String::new(),
            started: now_secs(),
        })
    }

    pub fn pending_path() -> PathBuf {
        crate::paths::config_dir().join("xiaomi-phone-pending.json")
    }

    /// See [`super::Login::cookies`].
    pub fn cookies(&self) -> Vec<(String, String)> {
        self.jar.pairs()
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn save(&self, path: &Path) -> Result<(), XiaomiError> {
        write_private(path, &serde_json::to_vec(self).expect("json"))
    }

    pub fn load(path: &Path) -> Option<PhoneLogin> {
        let l: PhoneLogin = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        (now_secs().saturating_sub(l.started) < 15 * 60).then_some(l)
    }

    fn client(&self) -> Client {
        let mut c = Client::new(&self.device_id);
        c.agent_name = BROWSER.into();
        c.headers = vec![
            ("Accept".into(), "application/json, text/plain, */*".into()),
            ("Origin".into(), ACCOUNT.into()),
            ("Referer".into(), format!("{ACCOUNT}/fe/service/login/phone")),
            ("X-Requested-With".into(), "XMLHttpRequest".into()),
        ];
        if self.jar.get("uLocale").is_none() {
            c.jar.set("uLocale", LOCALE);
            c.jar.set("pass_ua", "web");
        } else {
            c.jar = self.jar.clone();
        }
        c
    }

    /// The code page's address, which the last two steps name as their referrer.
    fn code_page(&self) -> String {
        format!(
            "{ACCOUNT}/fe/service/login/challenge/ticket?_locale={LOCALE}&_callingCode={}&_user={}&_agreementChecked=true",
            enc(&self.calling),
            enc(&self.user)
        )
    }

    /// Ask Xiaomi to text a code to the number.
    pub fn send(&mut self) -> Result<Step, XiaomiError> {
        Trace::start("phone");
        let mut c = self.client();
        // Which account system the page belongs to decides a field later on.
        let page = c.get("phone-page", &format!("{ACCOUNT}/fe/service/login/phone?_locale={LOCALE}"))?;
        let region = page
            .text()
            .split("window.region=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap_or("")
            .to_ascii_lowercase();
        self.policy = if region.contains("cnbj") || region.contains("staging") {
            "miaccount".into()
        } else {
            "globalmiaccount".into()
        };
        let (phone, eui) = wrap("phone", &self.user);
        let r = c.send("phone-kind", "POST", &format!("{ACCOUNT}/pass/preference"), Some(&[("phone", phone.as_str())]), &[("EUI", eui.as_str())])?;
        self.jar = c.jar.clone();
        let v = r.json().ok_or_else(|| r.page_error("phone-kind"))?;
        if v.get("code").and_then(Value::as_i64) != Some(0) {
            return Err(refusal(&v));
        }
        let kind = v.pointer("/data/type").and_then(Value::as_str).unwrap_or("");
        if kind != "ticket" {
            return Err(XiaomiError::Refused(format!(
                "Xiaomi does not offer a texted code for this number (it offers \"{kind}\"); sign in with the password instead"
            )));
        }
        self.request_code("")
    }

    /// Answer a captcha Xiaomi showed before sending the text.
    pub fn captcha(&mut self, text: &str) -> Result<Step, XiaomiError> {
        self.request_code(text.trim())
    }

    fn request_code(&mut self, captcha: &str) -> Result<Step, XiaomiError> {
        let mut c = self.client();
        let (user, eui) = wrap("user", &self.user);
        let r = c.send(
            "send-text",
            "POST",
            &format!("{ACCOUNT}/pass/sendServiceLoginTicket"),
            Some(&[("sid", SID), ("user", user.as_str()), ("captCode", captcha)]),
            &[("EUI", eui.as_str())],
        )?;
        self.jar = c.jar.clone();
        let v = r.json().ok_or_else(|| r.page_error("send-text"))?;
        if let Some(url) = str_field(&v, "captchaUrl").filter(|u| !u.is_empty() && u != "null") {
            let full = if url.starts_with('/') { format!("{ACCOUNT}{url}") } else { url };
            let pic = c.get("captcha", &full)?;
            self.jar = c.jar.clone();
            return Ok(Step::Captcha(pic.body));
        }
        match v.get("code").and_then(Value::as_i64) {
            Some(0) => Ok(Step::Code {
                by_email: false,
                to: masked(&self.calling, &self.national),
            }),
            Some(87001) => Err(XiaomiError::WrongCode),
            _ => Err(refusal(&v)),
        }
    }

    /// Answer the texted code.
    pub fn code(&mut self, code: &str) -> Result<Step, XiaomiError> {
        let code = code.trim();
        let mut c = self.client();
        let page = self.code_page();
        let r = c.send(
            "check-number",
            "POST",
            &format!("{ACCOUNT}/pass/phoneInfo"),
            Some(&[("sid", SID), ("user", self.user.as_str()), ("ticket", code), ("policyName", self.policy.as_str())]),
            &[("Referer", page.as_str())],
        )?;
        self.jar = c.jar.clone();
        let v = r.json().ok_or_else(|| r.page_error("check-number"))?;
        match v.get("code").and_then(Value::as_i64) {
            Some(0) => {}
            Some(70016..=70018) => return Err(XiaomiError::WrongCode),
            _ => return Err(refusal(&v)),
        }
        if v.pointer("/data/status").and_then(Value::as_i64) != Some(2) {
            return Err(XiaomiError::Refused("no Xiaomi account uses this number".into()));
        }
        let fingerprint: String = uuid::Uuid::new_v4().simple().to_string();
        let r = c.send(
            "sign-in",
            "POST",
            &format!("{ACCOUNT}/pass/serviceLoginTicketAuth"),
            Some(&[
                ("callback", ACCOUNT),
                ("sid", SID),
                ("qs", "%3Fsid%3Dpassport"),
                ("serviceParam", ""),
                ("_sign", ""),
                ("user", self.user.as_str()),
                ("ticket", code),
                ("_json", "true"),
                ("_locale", LOCALE),
                ("deviceFingerprint", fingerprint.as_str()),
            ]),
            &[("Referer", page.as_str())],
        )?;
        self.jar = c.jar.clone();
        let v = r.json().ok_or_else(|| r.page_error("sign-in"))?;
        match v.get("code").and_then(Value::as_i64) {
            Some(0) => {}
            Some(70016..=70018) => return Err(XiaomiError::WrongCode),
            Some(70069 | 70070) => {
                return Err(XiaomiError::Page {
                    url: format!("{ACCOUNT}/fe/service/login/phone"),
                    title: "additional verification".into(),
                })
            }
            _ => return Err(refusal(&v)),
        }
        if let Some(loc) = str_field(&v, "location").filter(|l| !l.is_empty()) {
            let end = c.follow("sign-in-end", &loc)?;
            if c.jar.get("passToken").is_none() && str_field(&v, "passToken").is_none() {
                return Err(XiaomiError::Page { url: end.url, title: end.title });
            }
        }
        self.jar = c.jar.clone();
        let user_id = c.jar.get("userId").or_else(|| str_field(&v, "userId"));
        let pass_token = c.jar.get("passToken").or_else(|| str_field(&v, "passToken"));
        match (user_id, pass_token) {
            (Some(user_id), Some(pass_token)) if !pass_token.is_empty() => {
                Trace::note("signed in by phone");
                Ok(Step::Done(Session {
                    user_id,
                    pass_token,
                    device_id: self.device_id.clone(),
                    regions: Vec::new(),
                }))
            }
            _ => Err(unexpected("sign-in", "no pass token after the sign-in")),
        }
    }
}

fn refusal(v: &Value) -> XiaomiError {
    match v.get("code").and_then(Value::as_i64) {
        Some(70022) => XiaomiError::Refused("Xiaomi has sent this number as many texts as it allows today; try tomorrow".into()),
        _ => XiaomiError::Refused(describe(v)),
    }
}

/// `+91 98765-43210` → (`+91`, `9876543210`). Country codes are one to
/// three digits; the length is read from the numbering plan's first digits.
fn split_number(phone: &str) -> Option<(String, String)> {
    let digits: String = phone.chars().filter(|c| !matches!(c, ' ' | '-' | '(' | ')' | '.')).collect();
    let rest = digits.strip_prefix('+').or_else(|| digits.strip_prefix("00"))?;
    if rest.len() < 6 || rest.len() > 15 || !rest.chars().all(|c| c.is_ascii_digit()) || rest.starts_with('0') {
        return None;
    }
    let three = ["21", "22", "23", "24", "25", "26", "29", "35", "37", "38", "42", "50", "59", "67", "68", "69", "80", "85", "87", "88", "96", "97", "99"];
    let len = if rest.starts_with('1') || rest.starts_with('7') {
        1
    } else if three.iter().any(|p| rest.starts_with(p)) {
        3
    } else {
        2
    };
    Some((format!("+{}", &rest[..len]), rest[len..].to_string()))
}

fn masked(calling: &str, national: &str) -> String {
    let tail = &national[national.len().saturating_sub(2)..];
    format!("{calling} {}{tail}", "*".repeat(national.len().saturating_sub(2)))
}

/// A field as the sign-in page sends it: (the field's value, the EUI header).
fn wrap(name: &str, value: &str) -> (String, String) {
    let b64 = base64::engine::general_purpose::STANDARD;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*";
    let key: Vec<u8> = random_bytes(16).iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()]).collect();
    let sealed = cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, b"0102030405060708")
        .expect("16-byte key and IV")
        .encrypt_padded_vec_mut::<Pkcs7>(value.as_bytes());
    let der = b64.decode(PAGE_KEY).expect("page key");
    let (n, e) = spki_rsa(&der).expect("page key is RSA");
    let wrapped_key = rsa_encrypt(&n, &e, b64.encode(&key).as_bytes());
    (
        b64.encode(sealed),
        format!("{}.{}", b64.encode(wrapped_key), b64.encode(name)),
    )
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n + 16);
    while out.len() < n {
        out.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    out.truncate(n);
    out
}

/// RSA encryption with PKCS #1 v1.5 padding (block type 2).
fn rsa_encrypt(n: &BigUint, e: &BigUint, message: &[u8]) -> Vec<u8> {
    let k = n.to_bytes_be().len();
    assert!(message.len() + 11 <= k, "message too long for the key");
    let mut block = vec![0u8, 2];
    let mut fill: Vec<u8> = Vec::new();
    while fill.len() < k - 3 - message.len() {
        fill.extend(random_bytes(32).into_iter().filter(|b| *b != 0));
    }
    fill.truncate(k - 3 - message.len());
    block.extend(fill);
    block.push(0);
    block.extend_from_slice(message);
    let c = BigUint::from_bytes_be(&block).modpow(e, n).to_bytes_be();
    let mut out = vec![0u8; k - c.len()];
    out.extend(c);
    out
}

/// The modulus and exponent of an RSA SubjectPublicKeyInfo.
fn spki_rsa(der: &[u8]) -> Option<(BigUint, BigUint)> {
    fn read(d: &[u8]) -> Option<(u8, &[u8], &[u8])> {
        let tag = *d.first()?;
        let first = *d.get(1)? as usize;
        let (len, head) = if first < 0x80 {
            (first, 2)
        } else {
            let n = first & 0x7f;
            let mut len = 0usize;
            for i in 0..n {
                len = (len << 8) | *d.get(2 + i)? as usize;
            }
            (len, 2 + n)
        };
        let body = d.get(head..head + len)?;
        Some((tag, body, &d[head + len..]))
    }
    let (_, spki, _) = read(der)?; // SEQUENCE
    let (_, _alg, rest) = read(spki)?; // AlgorithmIdentifier
    let (tag, bits, _) = read(rest)?; // BIT STRING
    if tag != 0x03 {
        return None;
    }
    let (_, key, _) = read(bits.get(1..)?)?; // RSAPublicKey SEQUENCE
    let (_, n, rest) = read(key)?;
    let (_, e, _) = read(rest)?;
    Some((BigUint::from_bytes_be(n), BigUint::from_bytes_be(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_split_into_country_and_number() {
        assert_eq!(split_number("+91 98765 43210"), Some(("+91".into(), "9876543210".into())));
        assert_eq!(split_number("+1 (202) 555-0100"), Some(("+1".into(), "2025550100".into())));
        assert_eq!(split_number("+971 50 123 4567"), Some(("+971".into(), "501234567".into())));
        assert_eq!(split_number("0091 98765 43210"), Some(("+91".into(), "9876543210".into())));
        assert_eq!(split_number("98765 43210"), None);
        assert_eq!(PhoneLogin::new("+86 138 0013 8000").unwrap().user, "13800138000");
        assert_eq!(PhoneLogin::new("+91 98765 43210").unwrap().user, "+919876543210");
        assert_eq!(masked("+91", "9876543210"), "+91 ********10");
    }

    #[test]
    fn the_page_key_is_a_1024_bit_rsa_key() {
        let der = base64::engine::general_purpose::STANDARD.decode(PAGE_KEY).unwrap();
        let (n, e) = spki_rsa(&der).unwrap();
        assert_eq!(n.bits(), 1024);
        assert_eq!(e, BigUint::from(65537u32));
    }

    /// Seal with a throwaway test key, open with its private exponent.
    #[test]
    fn a_wrapped_key_opens_with_the_private_key() {
        let der = base64::engine::general_purpose::STANDARD.decode(
            "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDGkEncPZTuHetFqZ0daXhfpqoXJ9DlxyenNP49zzbivdtkcN2NxIzLlUB5KxXU2QyhfbN4WXu/mgXxeklVJw1SbFwVA8YlvazXc4pHwpiheqG9vll5eTG1HbL2MOuXybiwMmqZXfVr1+7JE6VfOD7I9ePYK5KZ/jrWo9rjKrc06wIDAQAB",
        )
        .unwrap();
        let d = BigUint::parse_bytes(b"b38705a0c175040dbd08912062d557667413d937c0a1dffcb42b2fabc6cc4319916130c2c048f35ba62852e567a9b84cb93e23a42e93ba5a920110a578fe0405e4ec53a477f18bd6e99b4f52afad671704573da4708b833f6b5ad125e89bf1da2521a391c2e921f829349d4b313b8cd85841aa63873b78b3b62381fc64da7ed1", 16).unwrap();
        let (n, e) = spki_rsa(&der).unwrap();
        let sealed = rsa_encrypt(&n, &e, b"a secret key");
        assert_eq!(sealed.len(), 128);
        let mut block = BigUint::from_bytes_be(&sealed).modpow(&d, &n).to_bytes_be();
        // to_bytes_be drops the leading zero: 02 <fill> 00 <message>
        assert_eq!(block[0], 2);
        let zero = block.iter().skip(1).position(|b| *b == 0).unwrap() + 1;
        assert!(zero >= 9, "at least eight filler bytes");
        assert_eq!(block.split_off(zero + 1), b"a secret key");
    }

    #[test]
    fn a_wrapped_field_has_the_page_shape() {
        let (value, eui) = wrap("phone", "+919876543210");
        let b64 = base64::engine::general_purpose::STANDARD;
        assert_eq!(b64.decode(&value).unwrap().len(), 16); // 13 bytes, padded to one block
        let (key, name) = eui.split_once('.').unwrap();
        assert_eq!(b64.decode(key).unwrap().len(), 128);
        assert_eq!(b64.decode(name).unwrap(), b"phone");
    }
}
