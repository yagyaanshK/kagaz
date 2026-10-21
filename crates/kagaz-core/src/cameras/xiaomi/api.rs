//! Requests to Xiaomi's device cloud with a signed-in account: each request
//! is signed and RC4-encrypted with a key made from this visit's
//! `ssecurity`, as the Mi Home app does.

use super::account::XiaomiError;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::time::Duration;

/// Xiaomi's server regions, in the order they are tried.
pub const REGIONS: &[&str] = &["cn", "de", "i2", "ru", "sg", "us"];

/// A camera (or camera doorbell) on the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Camera {
    pub did: String,
    pub name: String,
    pub model: String,
    /// Lower case, colon-separated, as Xiaomi gives it.
    pub mac: String,
    /// Its address on its own network, as it last told Xiaomi.
    pub ip: String,
    pub region: String,
    pub online: bool,
}

impl Camera {
    /// go2rtc's address for it: go2rtc fetches the keys with the account's
    /// token for `user_id` and connects to `ip` on the home network.
    pub fn stream_url(&self, user_id: &str) -> String {
        format!(
            "xiaomi://{user_id}:{}@{}?did={}&model={}",
            self.region, self.ip, self.did, self.model
        )
    }
}

pub struct Cloud {
    user_id: String,
    ssecurity: Vec<u8>,
    service_token: String,
    agent_name: String,
    agent: ureq::Agent,
}

impl Cloud {
    pub(super) fn new(user_id: &str, ssecurity: Vec<u8>, service_token: &str, agent_name: &str) -> Cloud {
        Cloud {
            user_id: user_id.to_string(),
            ssecurity,
            service_token: service_token.to_string(),
            agent_name: agent_name.to_string(),
            agent: ureq::AgentBuilder::new()
                .tls_config(crate::cameras::tapo::tls::client_config())
                .timeout_connect(Duration::from_secs(15))
                .timeout_read(Duration::from_secs(30))
                .build(),
        }
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// One signed request; returns the answer's `result`.
    pub fn request(&self, region: &str, path: &str, params: &str) -> Result<Value, XiaomiError> {
        let nonce = nonce();
        let key = signed_nonce(&self.ssecurity, &nonce);
        let b64 = base64::engine::general_purpose::STANDARD;
        let hash = signature("POST", path, params, None, &key);
        let data = b64.encode(rc4(&key, params.as_bytes()));
        let hash = b64.encode(rc4(&key, hash.as_bytes()));
        let sig = signature("POST", path, &data, Some(&hash), &key);
        let url = format!("{}{path}", base_url(region));
        let resp = self
            .agent
            .post(&url)
            .set("User-Agent", &self.agent_name)
            .set(
                "Cookie",
                &format!(
                    "userId={0}; cUserId={0}; serviceToken={1}; yetAnotherServiceToken={1}; locale=en_US",
                    self.user_id, self.service_token
                ),
            )
            .send_form(&[
                ("data", data.as_str()),
                ("rc4_hash__", hash.as_str()),
                ("signature", sig.as_str()),
                ("_nonce", b64.encode(nonce).as_str()),
            ]);
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(401 | 403, _)) => return Err(XiaomiError::SignedOut),
            Err(ureq::Error::Status(code, _)) => {
                return Err(XiaomiError::Unexpected {
                    step: path.into(),
                    detail: format!("HTTP {code}"),
                })
            }
            Err(e) => return Err(XiaomiError::Transport(e.to_string())),
        };
        let mut body = String::new();
        let _ = resp.into_reader().take(8 << 20).read_to_string(&mut body);
        let plain = b64
            .decode(body.trim())
            .map(|c| rc4(&key, &c))
            .map_err(|_| XiaomiError::Unexpected {
                step: path.into(),
                detail: "an answer that is not encrypted".into(),
            })?;
        let v: Value = serde_json::from_slice(&plain).map_err(|_| XiaomiError::Unexpected {
            step: path.into(),
            detail: "an answer that does not decrypt".into(),
        })?;
        match v.get("code").and_then(Value::as_i64) {
            Some(0) => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            _ => Err(XiaomiError::Refused(
                v.get("message").and_then(Value::as_str).unwrap_or("refused").to_string(),
            )),
        }
    }

    /// The cameras in one region.
    pub fn cameras_in(&self, region: &str) -> Result<Vec<Camera>, XiaomiError> {
        let result = self.request(region, "/v2/home/device_list_page", "{}")?;
        Ok(parse_cameras(&result, region))
    }

    /// The cameras in every region; `known` regions are asked first and,
    /// when they have cameras, alone. Returns the cameras and the regions
    /// they were found in.
    pub fn cameras(&self, known: &[String]) -> Result<(Vec<Camera>, Vec<String>), XiaomiError> {
        let mut regions: Vec<String> = known.to_vec();
        let mut found = Vec::new();
        let mut last_err = None;
        for pass in 0..2 {
            for r in &regions {
                match self.cameras_in(r) {
                    Ok(c) => found.extend(c),
                    Err(XiaomiError::SignedOut) => return Err(XiaomiError::SignedOut),
                    Err(e) => last_err = Some(e),
                }
            }
            if !found.is_empty() || pass == 1 {
                break;
            }
            regions = REGIONS
                .iter()
                .map(|r| r.to_string())
                .filter(|r| !known.contains(r))
                .collect();
        }
        if found.is_empty() {
            if let Some(e) = last_err {
                return Err(e);
            }
        }
        let mut where_: Vec<String> = found.iter().map(|c| c.region.clone()).collect();
        where_.sort();
        where_.dedup();
        Ok((found, where_))
    }
}

pub fn parse_cameras(result: &Value, region: &str) -> Vec<Camera> {
    let s = |d: &Value, k: &str| match d.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    result
        .get("list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|d| {
            let m = s(d, "model");
            m.contains(".camera.") || m.contains(".cateye.")
        })
        .map(|d| Camera {
            did: s(d, "did"),
            name: s(d, "name"),
            model: s(d, "model"),
            mac: s(d, "mac").to_ascii_lowercase(),
            ip: s(d, "localip"),
            region: region.to_string(),
            online: d.get("isOnline").and_then(Value::as_bool).unwrap_or(true),
        })
        .collect()
}

pub fn base_url(region: &str) -> String {
    match region {
        "de" | "i2" | "ru" | "sg" | "us" => format!("https://{region}.api.io.mi.com/app"),
        _ => "https://api.io.mi.com/app".into(),
    }
}

fn nonce() -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..8]);
    let minutes = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0) as u32;
    n[8..].copy_from_slice(&minutes.to_be_bytes());
    n
}

fn signed_nonce(ssecurity: &[u8], nonce: &[u8]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(ssecurity);
    h.update(nonce);
    h.finalize().to_vec()
}

fn signature(method: &str, path: &str, data: &str, hash: Option<&str>, key: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut s = format!("{method}&{path}&data={data}");
    if let Some(h) = hash {
        s += &format!("&rc4_hash__={h}");
    }
    s += &format!("&{}", b64.encode(key));
    b64.encode(Sha1::digest(s.as_bytes()))
}

/// RC4 with the first 1024 bytes of keystream dropped.
fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    let mut next = || {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        s[(s[i as usize].wrapping_add(s[j as usize])) as usize]
    };
    for _ in 0..1024 {
        next();
    }
    data.iter().map(|b| b ^ next()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RC4 against the classic "Key"/"Plaintext" vector, with the 1024
    /// dropped bytes accounted for by comparing to a plain RC4 run.
    #[test]
    fn rc4_drops_the_first_kilobyte() {
        fn plain(key: &[u8], n: usize) -> Vec<u8> {
            let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
            let mut j: u8 = 0;
            for i in 0..256 {
                j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
                s.swap(i, j as usize);
            }
            let (mut i, mut j) = (0u8, 0u8);
            (0..n)
                .map(|_| {
                    i = i.wrapping_add(1);
                    j = j.wrapping_add(s[i as usize]);
                    s.swap(i as usize, j as usize);
                    s[(s[i as usize].wrapping_add(s[j as usize])) as usize]
                })
                .collect()
        }
        // The classic vector first: RC4("Key") on "Plaintext".
        let ks = plain(b"Key", 9);
        let ct: Vec<u8> = b"Plaintext".iter().zip(&ks).map(|(a, b)| a ^ b).collect();
        assert_eq!(ct, [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]);
        let ks = plain(b"Key", 1024 + 4);
        assert_eq!(rc4(b"Key", &[0; 4]), ks[1024..]);
        assert_eq!(rc4(b"Key", &rc4(b"Key", b"round trip")), b"round trip");
    }

    #[test]
    fn cameras_come_from_the_device_list() {
        let v: Value = serde_json::from_str(
            r#"{"list":[
                {"did":"123456789","name":"Hall","model":"mijia.camera.v1","mac":"AA:BB:CC:00:00:01","localip":"192.0.2.20","isOnline":true},
                {"did":"987","name":"Lamp","model":"yeelink.light.color1","localip":"192.0.2.21"}
            ]}"#,
        )
        .unwrap();
        let c = parse_cameras(&v, "i2");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].mac, "aa:bb:cc:00:00:01");
        assert_eq!(
            c[0].stream_url("1000"),
            "xiaomi://1000:i2@192.0.2.20?did=123456789&model=mijia.camera.v1"
        );
    }

    #[test]
    fn regions_have_their_own_hosts() {
        assert_eq!(base_url("cn"), "https://api.io.mi.com/app");
        assert_eq!(base_url("i2"), "https://i2.api.io.mi.com/app");
    }
}
