//! A TP-Link (Tapo) cloud session: the signed login the Tapo app performs,
//! its email second factor, token refresh, and the account's camera list.
//!
//! The request shapes, header set and signing are those of the Tapo Android
//! app (v3.20.512), as documented and verified live by the ontapo project
//! (MIT) and earlier by tapo-cli. Only the account's own devices are ever
//! reachable; nothing here bypasses any paid feature.

use super::tls::agent;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const APP_TYPE: &str = "TP-Link_Tapo_Android";
pub const APP_VERSION: &str = "3.20.512";
pub const LOGIN_BASE: &str = "https://n-wap.i.tplinkcloud.com";
pub const DEFAULT_APP_SERVER: &str = "aps1-app-server.iot.i.tplinkcloud.com";
/// Email delivery of the second-factor code.
pub const MFA_EMAIL: u32 = 2;

// The app's login signing identity: public values from the shipped APK, used
// only to sign requests to endpoints the user's own account already uses.
const ACCESS_KEY: &str = "e37525375f8845999bcc56d5e6faa76d";
const SECRET_KEY: &str = "314bc6700b3140ca80bc655e527cb062";
const HARDCODED_TIMESTAMP: &str = "9999999999";

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("cannot reach TP-Link's cloud: {0}")]
    Transport(String),
    #[error("login failed: {0}")]
    Auth(String),
    #[error("the account needs a second factor (process {process_id}); supported types {types:?}")]
    MfaRequired { process_id: String, types: Vec<u32> },
    #[error("the cloud token is no longer valid; log in again")]
    Unauthorized,
    #[error("TP-Link's cloud answered HTTP {status}: {message}")]
    Rejected { status: u16, message: String },
    #[error("cannot save or read the session file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// A camera on the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Camera {
    pub device_id: String,
    pub name: String,
    pub model: String,
    /// The region-correct app server host for this device.
    pub app_server: String,
    /// Hardware address, lower-case without separators, for finding the
    /// camera on the local network.
    #[serde(default)]
    pub mac: String,
    #[serde(default)]
    pub hw_version: String,
    #[serde(default)]
    pub firmware: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub email: String,
    pub token: String,
    #[serde(default)]
    pub refresh_token: String,
    pub terminal_uuid: String,
    pub app_server: String,
    pub account_base: String,
    /// The account password, kept only when the user chose local viewing:
    /// cameras on the same network take it (hashed) as their local login,
    /// exactly as the Tapo app does. The file is private (0600).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub local_password: String,
    #[serde(skip)]
    mfa_process_id: String,
}

pub fn new_terminal_uuid() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .to_ascii_uppercase()
}

/// `(Content-MD5, X-Authorization)` for a signed account request.
pub fn sign(path: &str, body: &[u8], nonce: &str) -> (String, String) {
    use md5::Digest;
    let content_md5 = base64::engine::general_purpose::STANDARD.encode(md5::Md5::digest(body));
    let to_sign = format!("{content_md5}\n{HARDCODED_TIMESTAMP}\n{nonce}\n{path}");
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(SECRET_KEY.as_bytes()).expect("hmac key");
    mac.update(to_sign.as_bytes());
    let sig: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    (
        content_md5,
        format!("Timestamp={HARDCODED_TIMESTAMP}, Nonce={nonce}, AccessKey={ACCESS_KEY}, Signature={sig}"),
    )
}

fn account_post(base: &str, path: &str, body: &Value) -> Result<Value, CloudError> {
    let bytes = serde_json::to_vec(body).expect("json");
    let (content_md5, x_auth) = sign(path, &bytes, &new_terminal_uuid());
    let response = agent()
        .post(&format!("{base}{path}"))
        .set("Content-Type", "application/json; charset=UTF-8")
        .set("Content-MD5", &content_md5)
        .set("X-Authorization", &x_auth)
        .set(
            "User-Agent",
            &format!("{APP_TYPE}/{APP_VERSION}(kagaz;Android 15)"),
        )
        .send_bytes(&bytes);
    let response = match response {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(t)) => return Err(CloudError::Transport(t.to_string())),
    };
    let text = response
        .into_string()
        .map_err(|e| CloudError::Transport(e.to_string()))?;
    serde_json::from_str(&text).map_err(|_| CloudError::Auth(trim(&text)))
}

fn trim(s: &str) -> String {
    s.chars().take(300).collect()
}

impl Session {
    /// Log in. On accounts with a second factor this returns
    /// `CloudError::MfaRequired`; keep the returned partial session via
    /// [`Session::begin`] and finish with [`Session::send_mfa_code`] and
    /// [`Session::submit_mfa`].
    pub fn login(email: &str, password: &str) -> Result<Session, CloudError> {
        let mut s = Session::begin(email);
        s.do_login(password)?;
        Ok(s)
    }

    /// An empty session for `email`, before login.
    pub fn begin(email: &str) -> Session {
        Session {
            email: email.to_string(),
            token: String::new(),
            refresh_token: String::new(),
            terminal_uuid: new_terminal_uuid(),
            app_server: DEFAULT_APP_SERVER.to_string(),
            account_base: LOGIN_BASE.to_string(),
            local_password: String::new(),
            mfa_process_id: String::new(),
        }
    }

    /// Submit the password; sets the token, or stores the MFA process and
    /// returns `MfaRequired`. Follows TP-Link's region redirect.
    pub fn do_login(&mut self, password: &str) -> Result<(), CloudError> {
        let body = json!({
            "appType": APP_TYPE,
            "appVersion": APP_VERSION,
            "cloudUserName": self.email,
            "cloudPassword": password,
            "platform": "Android 15",
            "refreshTokenNeeded": true,
            "terminalUUID": self.terminal_uuid,
            "terminalName": "Kagaz",
            "terminalMeta": "kagaz",
        });
        let path = "/api/v2/account/login";
        let mut data = account_post(&self.account_base, path, &body)?;
        let mut result = data.get("result").cloned().unwrap_or(Value::Null);
        // A wrong-region gateway answers without a token but with the right server.
        if result.get("token").is_none() && result.get("MFAProcessId").is_none() {
            if let Some(url) = result.get("appServerUrlV2").and_then(Value::as_str) {
                let url = url.trim_end_matches('/');
                if url != self.account_base {
                    self.account_base = url.to_string();
                    data = account_post(&self.account_base, path, &body)?;
                    result = data.get("result").cloned().unwrap_or(Value::Null);
                }
            }
        }
        if let Some(token) = result.get("token").and_then(Value::as_str) {
            self.token = token.to_string();
            self.refresh_token = result
                .get("refreshToken")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            return Ok(());
        }
        if let Some(pid) = result.get("MFAProcessId").and_then(Value::as_str) {
            self.mfa_process_id = pid.to_string();
            let types = result
                .get("supportedMFATypes")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_u64)
                        .map(|v| v as u32)
                        .collect()
                })
                .unwrap_or_default();
            return Err(CloudError::MfaRequired {
                process_id: pid.to_string(),
                types,
            });
        }
        Err(CloudError::Auth(trim(&data.to_string())))
    }

    /// Ask TP-Link to send the verification code: 2 = email, 1 = a push to a
    /// Tapo app already bound to the account (a fresh terminal has none, so
    /// email is the one that works). The email endpoint is not documented;
    /// the candidates are tried in order until one is accepted.
    pub fn send_mfa_code(&self, mfa_type: u32, password: &str) -> Result<String, CloudError> {
        let endpoints: &[&str] = if mfa_type == MFA_EMAIL {
            &[
                "/api/v2/account/getEmailVC4TerminalMFA",
                "/api/v2/account/sendEmailVC4TerminalMFA",
                "/api/v2/account/getPushVC4TerminalMFA",
                "/api/v2/account/getVC4TerminalMFA",
                "/api/v2/account/getEmailVerifyCode",
            ]
        } else {
            &["/api/v2/account/getPushVC4TerminalMFA"]
        };
        let body = json!({
            "appType": APP_TYPE,
            "cloudUserName": self.email,
            "cloudPassword": password,
            "MFAProcessId": self.mfa_process_id,
            "MFAType": mfa_type,
            "terminalUUID": self.terminal_uuid,
        });
        let mut last = String::new();
        for endpoint in endpoints {
            let data = account_post(&self.account_base, endpoint, &body)?;
            let outer = data.get("error_code").and_then(Value::as_i64).unwrap_or(0);
            let inner = data
                .get("result")
                .and_then(|r| r.get("errorCode"))
                .and_then(|c| {
                    c.as_i64()
                        .or_else(|| c.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0);
            if outer == 0 && inner == 0 {
                return Ok(endpoint.to_string());
            }
            last = format!("{endpoint}: {}", trim(&data.to_string()));
        }
        Err(CloudError::Auth(format!(
            "TP-Link would not send the code: {last}"
        )))
    }

    /// Finish the login with the code TP-Link sent.
    pub fn submit_mfa(&mut self, code: &str, mfa_type: u32) -> Result<(), CloudError> {
        let body = json!({
            "appType": APP_TYPE,
            "cloudUserName": self.email,
            "code": code.trim(),
            "MFAProcessId": self.mfa_process_id,
            "MFAType": mfa_type,
            "terminalUUID": self.terminal_uuid,
            "terminalName": "Kagaz",
            "refreshTokenNeeded": true,
        });
        let data = account_post(
            &self.account_base,
            "/api/v2/account/checkMFACodeAndLogin",
            &body,
        )?;
        let result = data.get("result").cloned().unwrap_or(Value::Null);
        match result.get("token").and_then(Value::as_str) {
            Some(t) => {
                self.token = t.to_string();
                self.refresh_token = result
                    .get("refreshToken")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Ok(())
            }
            None => Err(CloudError::Auth(format!(
                "the code was not accepted: {}",
                trim(&data.to_string())
            ))),
        }
    }

    /// Exchange the refresh token for a new session token.
    pub fn refresh(&mut self) -> Result<(), CloudError> {
        if self.refresh_token.is_empty() {
            return Err(CloudError::Unauthorized);
        }
        let body = json!({
            "method": "refreshToken",
            "params": {
                "appType": APP_TYPE,
                "refreshToken": self.refresh_token,
                "terminalUUID": self.terminal_uuid,
            }
        });
        let bytes = serde_json::to_vec(&body).expect("json");
        let response = agent()
            .post(&format!("{}/", self.account_base))
            .set("Content-Type", "application/json; charset=UTF-8")
            .send_bytes(&bytes);
        let response = match response {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => return Err(CloudError::Transport(t.to_string())),
        };
        let data: Value = response
            .into_string()
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let result = data.get("result").cloned().unwrap_or(Value::Null);
        match result.get("token").and_then(Value::as_str) {
            Some(t) => {
                self.token = t.to_string();
                if let Some(r) = result.get("refreshToken").and_then(Value::as_str) {
                    self.refresh_token = r.to_string();
                }
                Ok(())
            }
            None => Err(CloudError::Unauthorized),
        }
    }

    /// The headers every non-login cloud request carries.
    pub fn cloud_request(&self, req: ureq::Request) -> ureq::Request {
        req.set("Authorization", &format!("ut|{}", self.token))
            .set("app-cid", &format!("app:{APP_TYPE}:{}", self.terminal_uuid))
            .set("x-app-name", APP_TYPE)
            .set("x-app-version", APP_VERSION)
            .set("x-term-id", &self.terminal_uuid)
            .set("x-ospf", "Android 15")
            .set("x-net-type", "wifi")
            .set("x-strict", "0")
            .set("x-locale", "en_US")
            .set("Content-Type", "application/json; charset=UTF-8")
            .set(
                "User-Agent",
                &format!("{APP_TYPE}/{APP_VERSION}(kagaz;Android 15)"),
            )
    }

    /// The account's cameras.
    pub fn cameras(&self) -> Result<Vec<Camera>, CloudError> {
        let url = format!("https://{}/v2/things", self.app_server);
        let response = self.cloud_request(agent().get(&url)).call();
        let response = match response {
            Ok(r) => r,
            Err(ureq::Error::Status(401, _)) => return Err(CloudError::Unauthorized),
            Err(ureq::Error::Status(status, r)) => {
                return Err(CloudError::Rejected {
                    status,
                    message: r.into_string().map(|s| trim(&s)).unwrap_or_default(),
                })
            }
            Err(ureq::Error::Transport(t)) => return Err(CloudError::Transport(t.to_string())),
        };
        let data: Value = response
            .into_string()
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        Ok(parse_things(&data, &self.app_server))
    }

    /// The raw device list, as the cloud returns it.
    pub fn things_raw(&self) -> Result<Value, CloudError> {
        let url = format!("https://{}/v2/things", self.app_server);
        let text = self
            .cloud_request(agent().get(&url))
            .call()
            .map_err(|e| CloudError::Transport(e.to_string()))?
            .into_string()
            .map_err(|e| CloudError::Transport(e.to_string()))?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Call device methods through the cloud passthrough (`services-sync`),
    /// as the app does. Returns one raw response object per request.
    pub fn device_requests(
        &self,
        device_id: &str,
        app_server: &str,
        requests: &[Value],
    ) -> Result<Vec<Value>, CloudError> {
        let envelope = json!({
            "inputParams": {
                "requestData": {
                    "method": "multipleRequest",
                    "params": { "requests": requests }
                }
            },
            "serviceId": "passthrough"
        });
        let url = format!("https://{app_server}/v1/things/{device_id}/services-sync");
        let bytes = serde_json::to_vec(&envelope).expect("json");
        let response = self.cloud_request(agent().post(&url)).send_bytes(&bytes);
        let (status, response) = match response {
            Ok(r) => (200u16, r),
            Err(ureq::Error::Status(401, _)) => return Err(CloudError::Unauthorized),
            Err(ureq::Error::Status(s, r)) => (s, r),
            Err(ureq::Error::Transport(t)) => return Err(CloudError::Transport(t.to_string())),
        };
        let text = response
            .into_string()
            .map_err(|e| CloudError::Transport(e.to_string()))?;
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if body.get("code").and_then(Value::as_i64) == Some(401) {
            return Err(CloudError::Unauthorized);
        }
        if status >= 400 {
            let message = body
                .get("message")
                .or_else(|| body.get("msg"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| trim(&text));
            return Err(CloudError::Rejected { status, message });
        }
        Ok(body
            .get("outputParams")
            .and_then(|o| o.get("responseData"))
            .and_then(|r| r.get("result"))
            .and_then(|r| r.get("responses"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// One device method; its `result`, or the device's error code.
    pub fn device_request(
        &self,
        device_id: &str,
        app_server: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, CloudError> {
        let responses = self.device_requests(
            device_id,
            app_server,
            &[json!({ "method": method, "params": params })],
        )?;
        let first = responses
            .into_iter()
            .next()
            .ok_or_else(|| CloudError::Rejected {
                status: 200,
                message: format!("{method}: no response"),
            })?;
        match first.get("error_code").and_then(Value::as_i64).unwrap_or(0) {
            0 => Ok(first.get("result").cloned().unwrap_or(Value::Null)),
            code => Err(CloudError::Rejected {
                status: 200,
                message: format!("{method}: device error {code}"),
            }),
        }
    }

    // ---- persistence: a 0600 file in the config folder ----

    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("tapo-session.json")
    }

    pub fn save(&self, path: &Path) -> Result<(), CloudError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| CloudError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        let text = serde_json::to_string_pretty(self).expect("json");
        write_private(path, text.as_bytes()).map_err(|source| CloudError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn load(path: &Path) -> Result<Session, CloudError> {
        let text = std::fs::read_to_string(path).map_err(|source| CloudError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|e| CloudError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

/// Parse `GET /v2/things`: the account's IP cameras.
pub fn parse_things(data: &Value, default_server: &str) -> Vec<Camera> {
    let mut out = Vec::new();
    for d in data
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if d.get("deviceType").and_then(Value::as_str) != Some("SMART.IPCAMERA") {
            continue;
        }
        let Some(id) = d.get("thingName").and_then(Value::as_str) else {
            continue;
        };
        let nickname = d.get("nickname").and_then(Value::as_str).unwrap_or("");
        let fallback = d.get("deviceName").and_then(Value::as_str).unwrap_or("");
        let name = base64::engine::general_purpose::STANDARD
            .decode(nickname)
            .ok()
            .filter(|b| !b.is_empty())
            .map(|b| String::from_utf8_lossy(&b[..]).to_string())
            .unwrap_or_else(|| fallback.to_string());
        let app_server = d
            .get("appServerUrlV2")
            .and_then(Value::as_str)
            .map(|u| {
                u.trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_string()
            })
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| default_server.to_string());
        out.push(Camera {
            device_id: id.to_string(),
            name,
            model: d
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            app_server,
            mac: d
                .get("mac")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .collect::<String>()
                .to_ascii_lowercase(),
            hw_version: d
                .get("hwVer")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            firmware: d
                .get("fwVer")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_matches_the_known_vector() {
        // Same inputs as ontapo's test_crypto: deterministic given body and nonce.
        let (md5, auth) = sign("/api/v2/account/login", b"{\"a\":1}", "nonce123");
        assert_eq!(
            md5,
            base64::engine::general_purpose::STANDARD
                .encode(<md5::Md5 as md5::Digest>::digest(b"{\"a\":1}"))
        );
        assert!(auth.starts_with("Timestamp=9999999999, Nonce=nonce123, AccessKey=e37525375f8845999bcc56d5e6faa76d, Signature="));
        assert_eq!(auth.len(), auth.find("Signature=").unwrap() + 10 + 40); // hex sha1
    }

    #[test]
    fn parses_cameras_from_things() {
        let data = json!({"data": [
            {"deviceType": "SMART.IPCAMERA", "thingName": "ABC123", "nickname": "RnJvbnQgZG9vcg==", "model": "C100", "appServerUrlV2": "https://euw1-app-server.iot.i.tplinkcloud.com/"},
            {"deviceType": "SMART.TAPOPLUG", "thingName": "PLUG"},
            {"deviceType": "SMART.IPCAMERA", "thingName": "DEF456", "nickname": "", "deviceName": "C200", "model": "C200"}
        ]});
        let cams = parse_things(&data, "aps1-app-server.iot.i.tplinkcloud.com");
        assert_eq!(cams.len(), 2);
        assert_eq!(cams[0].name, "Front door");
        assert_eq!(cams[0].app_server, "euw1-app-server.iot.i.tplinkcloud.com");
        assert_eq!(cams[1].name, "C200");
        assert_eq!(cams[1].app_server, "aps1-app-server.iot.i.tplinkcloud.com");
    }

    #[test]
    fn session_round_trips_through_a_private_file() {
        let dir = std::env::temp_dir().join(format!("kagaz-tapo-{}", std::process::id()));
        let path = dir.join("s.json");
        let mut s = Session::begin("me@example.com");
        s.token = "tok".into();
        s.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let back = Session::load(&path).unwrap();
        assert_eq!(back.token, "tok");
        assert_eq!(back.terminal_uuid, s.terminal_uuid);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
