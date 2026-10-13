//! OneTouch Smart Cloud, successor to OTIPVDP. See docs/onetouch.md.
//! Account tokens never appear in webview models or errors.
mod legacy;
pub mod rtsp;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Could not reach the OneTouch service. Check your connection and try again.")]
    Network,
    #[error("OneTouch returned HTTP {0}.")]
    Http(u16),
    #[error("Your OneTouch session has expired. Please sign in again.")]
    Expired,
    #[error("The OneTouch username or password was not accepted.")]
    Credentials,
    #[error("{operation}: {source}")]
    During {
        operation: &'static str,
        source: Box<Error>,
    },
    #[error("{0}")]
    Protocol(String),
    #[error("Could not save the OneTouch session: {0}")]
    Storage(std::io::Error),
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    SmartCloud,
    Otipvdp,
}
impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::SmartCloud => "OneTouch Smart Cloud",
            Self::Otipvdp => "OTIPVDP (legacy)",
        }
    }
}
// No Debug: includes access tokens. Passwords are never saved.
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub username: String,
    #[serde(default)]
    pub provider: Provider,
    #[serde(default)]
    role_hint: String,
    gate: String,
    rest: String,
    web: String,
    relay: String,
    token: String,
    refresh_token: String,
}
#[derive(Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub can_monitor: bool,
    #[serde(skip)]
    pub(crate) password: String,
}
#[derive(Clone)]
pub struct Account {
    pub devices: Vec<Device>,
    pub uid: String,
    pub role: String,
}
#[derive(Clone, Serialize)]
pub struct Recording {
    pub id: String,
    pub name: String,
    pub at: String,
    pub has_video: bool,
    pub has_picture: bool,
    #[serde(skip)]
    pub video: String,
    #[serde(skip)]
    pub picture: String,
}
#[derive(Serialize)]
pub struct Recordings {
    pub entries: Vec<Recording>,
    pub more: bool,
    pub message: String,
}
const SMART_CLOUD_USER_AGENT: &str = "Onetouchsmartcloud";

fn agent_builder() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .timeout_write(Duration::from_secs(15))
        .redirects(0)
}
fn agent() -> ureq::Agent {
    agent_builder().build()
}
fn smart_cloud_agent() -> ureq::Agent {
    // IntercomRemoteRepository.getOemUserAgent / OemDefined.DISCREET.
    // The gateway rejects valid credentials without this OEM routing header.
    agent_builder().user_agent(SMART_CLOUD_USER_AGENT).build()
}
fn transport_error(e: ureq::Error) -> Error {
    match e {
        ureq::Error::Status(401 | 403, _) => Error::Expired,
        ureq::Error::Status(c, _) => Error::Http(c),
        _ => Error::Network,
    }
}
fn read_response(response: ureq::Response) -> Result<Value, Error> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Network)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(protocol("OneTouch's response was too large."));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| protocol("OneTouch returned an unreadable response."))
}
fn protocol(s: &str) -> Error {
    Error::Protocol(s.into())
}
pub(crate) fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn shift(s: &str, amount: u8) -> String {
    s.chars()
        .map(|c| {
            let (base, modulus) = match c {
                '0'..='9' => (b'0', 10),
                'a'..='z' => (b'a', 26),
                'A'..='Z' => (b'A', 26),
                _ => return c,
            };
            (base + (c as u8 - base + amount) % modulus) as char
        })
        .collect()
}
// Region 06 is the app's Discreet/OneTouch region, not the old OTIPVDP default.
fn discovery_user() -> String {
    let seed = uuid::Uuid::new_v4().simple().to_string();
    let key = format!("3{}", &seed[..11]);
    let region = shift("000000000006", 3);
    format!(
        "{}{}{}{}{}{}",
        &key[..4],
        &region[..4],
        &key[4..8],
        &region[4..8],
        &key[8..],
        &region[8..]
    )
}
fn field(v: &Value, key: &str) -> String {
    match &v[key] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
fn required(v: &Value, key: &str) -> Result<String, Error> {
    let s = field(v, key);
    if s.is_empty() {
        Err(protocol(&format!("OneTouch's response is missing {key}.")))
    } else {
        Ok(s)
    }
}
fn api_data(v: Value) -> Result<Value, Error> {
    let code = field(&v, "err_code");
    if matches!(code.as_str(), "1000000001" | "1000100001" | "1000100002" | "1000100003") {
        return Err(Error::Expired);
    }
    if code == "1000100004" {
        return Err(Error::Credentials);
    }
    // Current APIs return err_code without result; RPS discovery returns result
    // without err_code. Require explicit success and reject conflicting fields.
    let result = field(&v, "result");
    let success = if code.is_empty() {
        result == "0"
    } else {
        code == "0" && (result.is_empty() || result == "0")
    };
    if !success {
        return Err(protocol(&format!(
            "OneTouch rejected the request (code {}).",
            if code.is_empty() {
                field(&v, "result")
            } else {
                code
            }
        )));
    }
    v.get("datas")
        .cloned()
        .ok_or_else(|| protocol("OneTouch's response is missing account data."))
}
fn during(operation: &'static str, source: Error) -> Error {
    Error::During {
        operation,
        source: Box::new(source),
    }
}
// Android interpolates the shifted email into the URL, leaving @ literal.
// Encode query delimiters before restoring @ so a username cannot add parameters.
fn login_request(
    a: &ureq::Agent,
    gate: &str,
    username: &str,
    password: &str,
) -> Result<ureq::Request, Error> {
    let mut url = Url::parse(&format!("{gate}/login"))
        .map_err(|_| protocol("OneTouch returned an invalid sign-in address."))?;
    url.query_pairs_mut()
        .append_pair("user", &shift(username, 3))
        .append_pair("passwd", &md5_hex(&md5_hex(password)));
    let query = url.query().unwrap_or_default().replace("%40", "@");
    url.set_query(Some(&query));
    Ok(a.get(url.as_str())
        .set("api-version", "7.11")
        .set("x-auth-token", ""))
}
fn service(value: &str) -> Result<String, Error> {
    let text = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let u =
        Url::parse(&text).map_err(|_| protocol("OneTouch returned an invalid service address."))?;
    let h = u.host_str().unwrap_or_default();
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
        || !(h == "onetouchsmartcloud.com"
            || h.ends_with(".onetouchsmartcloud.com")
            || h == "akuvox.com"
            || h.ends_with(".akuvox.com"))
    {
        return Err(protocol(
            "OneTouch returned an unrecognized HTTPS service address.",
        ));
    }
    Ok(u.as_str().trim_end_matches('/').to_string())
}
impl Session {
    pub fn login(username: &str, password: &str) -> Result<(Self, Account), Error> {
        let username = username.trim();
        if username.is_empty() || password.is_empty() {
            return Err(protocol("Enter your OneTouch username and password."));
        }
        let a = smart_cloud_agent();
        let gate = (|| {
            let response = a
                .get("https://rps.akuvox.com/v3.2/redirect")
                // Constant.API_VERSION_6_5_4 in the APK is the wire value 6.54.
                .set("api-version", "6.54")
                .query("user", &discovery_user())
                .call()
                .map_err(transport_error)?;
            service(&required(
                &api_data(read_response(response)?)?,
                "gate_serverV4",
            )?)
        })()
        .map_err(|e| during("Finding the OneTouch server", e))?;
        let data = (|| {
            let response = login_request(&a, &gate, username, password)?
                .call()
                .map_err(transport_error)?;
            api_data(read_response(response)?)
        })()
        .map_err(|e| during("Signing in to OneTouch Smart Cloud", e))?;
        if field(&data, "is_need_redirect") == "1" {
            return Err(protocol(
                "Complete the cloud migration in OneTouch Smart Cloud, then sign in here.",
            ));
        }
        if field(&data, "is_need_two_factor_auth") == "1" {
            return Err(protocol("This account requires OneTouch two-factor authentication, which Kagaz does not support yet."));
        }
        let mut s = Self {
            username: username.into(),
            provider: Provider::SmartCloud,
            role_hint: String::new(),
            gate,
            rest: service(&required(&data, "rest_server_https")?)?,
            web: if field(&data, "web_backend_server").is_empty() {
                "https://backend.onetouchsmartcloud.com".into()
            } else {
                service(&field(&data, "web_backend_server"))?
            },
            relay: required(&data, "vrtsp_server")?,
            token: required(&data, "token")?,
            refresh_token: field(&data, "refresh_token"),
        };
        let account = s
            .account()
            .map_err(|e| during("Loading OneTouch devices after sign-in", e))?;
        Ok((s, account))
    }
    fn get(&self, url: &str, version: &str, params: &[(&str, String)]) -> Result<Value, Error> {
        let a = match self.provider {
            Provider::SmartCloud => smart_cloud_agent(),
            Provider::Otipvdp => agent(),
        };
        let mut req = a
            .get(url)
            .set("api-version", version)
            .set("x-auth-token", &self.token)
            .set("x-cloud-lang", "en")
            .set("x-cloud-version", "6.4");
        for (key, value) in params {
            req = req.query(key, value);
        }
        read_response(req.call().map_err(transport_error)?)
    }
    fn refresh(&mut self) -> Result<(), Error> {
        if self.refresh_token.is_empty() {
            return Err(Error::Expired);
        }
        let response = smart_cloud_agent()
            .post(&format!("{}/refresh_token", self.gate))
            .set("api-version", "6.8")
            .set("Content-Type", "application/json")
            .set("x-auth-token", &self.token)
            .send_string(&json!({"refresh_token": self.refresh_token}).to_string())
            .map_err(transport_error)?;
        let data = api_data(read_response(response)?)?;
        self.token = required(&data, "token")?;
        let refresh = field(&data, "refresh_token");
        if !refresh.is_empty() {
            self.refresh_token = refresh;
        }
        self.save()
    }
    pub fn login_with_provider(
        provider: Provider,
        username: &str,
        password: &str,
        legacy_device: &str,
    ) -> Result<(Self, Account), Error> {
        match provider {
            Provider::SmartCloud => Self::login(username, password),
            Provider::Otipvdp => legacy::login(username, password, legacy_device),
        }
    }
    pub fn account(&mut self) -> Result<Account, Error> {
        if self.provider == Provider::Otipvdp {
            return legacy::account(self);
        }
        for attempt in 0..2 {
            let data = self
                .get(
                    &format!("{}/userconf", self.rest),
                    "7.13",
                    &[("token", self.token.clone())],
                )
                .and_then(api_data);
            match data {
                Err(Error::Expired) if attempt == 0 => self.refresh()?,
                result => return parse_account(&result?),
            }
        }
        Err(Error::Expired)
    }
    pub fn recordings(&mut self, role: &str, page: u32) -> Result<Recordings, Error> {
        if self.provider == Provider::Otipvdp {
            return legacy::recordings(self, page);
        }
        let (project, member) = match role {
            "10" => ("single", "master"),
            "11" => ("single", "member"),
            "20" => ("community", "master"),
            "21" => ("community", "member"),
            _ => {
                return Err(protocol(
                    "Recordings for this OneTouch account role are not supported yet.",
                ))
            }
        };
        let url = format!(
            "{}/web-server/v4/smartplus/{project}/log/{member}/getCaptureLog",
            self.web
        );
        for attempt in 0..2 {
            let result = self
                .get(
                    &url,
                    "7.13",
                    &[("page", page.max(1).to_string()), ("row", "20".into())],
                )
                .and_then(parse_recordings);
            match result {
                Err(Error::Expired) if attempt == 0 => self.refresh()?,
                result => return result,
            }
        }
        Err(Error::Expired)
    }
    pub fn live_target(&self, account: &Account, device: &Device) -> Result<rtsp::Target, Error> {
        if !device.can_monitor {
            return Err(protocol(
                "OneTouch does not allow live monitoring for this device.",
            ));
        }
        rtsp::Target::new(&self.relay, &device.id, &device.password, &account.uid)
            .map_err(|_| protocol("OneTouch returned an invalid video relay address."))
    }
    pub fn path() -> std::path::PathBuf {
        crate::paths::config_dir().join("onetouch-session.json")
    }
    pub fn load() -> Result<Option<Self>, Error> {
        let text = match std::fs::read_to_string(Self::path()) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::Storage(e)),
        };
        let s: Self = serde_json::from_str(&text)
            .map_err(|_| protocol("The saved OneTouch session is unreadable. Sign in again."))?;
        if s.provider == Provider::SmartCloud {
            service(&s.gate)?;
            service(&s.rest)?;
            service(&s.web)?;
        } else {
            legacy::service(&s.gate, "http")?;
            legacy::service(&s.rest, "https")?;
            legacy::service(&s.web, "http")?;
        }
        Ok(Some(s))
    }
    pub fn save(&self) -> Result<(), Error> {
        save_private(
            &Self::path(),
            &serde_json::to_vec(self).expect("session JSON"),
        )
    }
    pub fn logout() -> Result<(), Error> {
        match std::fs::remove_file(Self::path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Storage(e)),
        }
    }
}
fn save_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path
        .parent()
        .ok_or_else(|| protocol("Invalid session path."))?;
    std::fs::create_dir_all(parent).map_err(Error::Storage)?;
    let temp = parent.join(format!(".onetouch-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut f = options.open(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result.map_err(Error::Storage)
}
fn parse_account(data: &Value) -> Result<Account, Error> {
    let list = data["dev_list"]
        .as_array()
        .ok_or_else(|| protocol("OneTouch's device list was missing."))?;
    let mut devices = Vec::new();
    for item in list {
        let id = required(item, "mac")?;
        if id.len() != 12 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(protocol("OneTouch returned an invalid device identifier."));
        }
        devices.push(Device {
            id,
            name: required(item, "location")?,
            kind: field(item, "dev_type"),
            status: field(item, "status"),
            can_monitor: field(item, "is_need_monitor") == "1",
            password: field(item, "rtsp_pwd"),
        });
    }
    Ok(Account {
        devices,
        uid: required(&data["app_conf"], "uid")?,
        role: required(&data["app_conf"], "role")?,
    })
}
fn media_url(s: String) -> Result<String, Error> {
    if s.is_empty() {
        return Ok(s);
    }
    let url =
        Url::parse(&s).map_err(|_| protocol("OneTouch returned an invalid recording address."))?;
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(protocol(
            "OneTouch returned an unsupported recording address.",
        ));
    }
    Ok(s)
}
fn parse_recordings(v: Value) -> Result<Recordings, Error> {
    let code = field(&v, "code");
    if matches!(
        code.as_str(),
        "2" | "1000000001" | "1000100001" | "1000100002" | "1000100003"
    ) {
        return Err(Error::Expired);
    }
    if code != "0" {
        return Err(protocol(&format!(
            "OneTouch could not list recordings (code {code})."
        )));
    }
    let list = v["data"]["list"]
        .as_array()
        .ok_or_else(|| protocol("OneTouch's recordings list was missing."))?;
    let mut entries = Vec::new();
    for item in list.iter().take(20) {
        let video = media_url(field(item, "videoPlayUrl"))?;
        let picture = media_url(field(item, "picUrl"))?;
        entries.push(Recording {
            id: required(item, "id")?,
            name: field(item, "deviceName"),
            at: field(item, "captureTime"),
            has_video: !video.is_empty(),
            has_picture: !picture.is_empty(),
            video,
            picture,
        });
    }
    Ok(Recordings {
        entries,
        more: list.len() > 20,
        message: String::new(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_encoding_matches_app() {
        assert_eq!(shift("AbcXyz789+@.é", 3), "DefAbc012+@.é");
        assert_eq!(
            md5_hex(&md5_hex("password")),
            "696d29e0940a4957748fe3fc9efd22a3"
        );
        let u = discovery_user();
        assert_eq!(u.len(), 24);
        assert_eq!(
            format!("{}{}{}", &u[4..8], &u[12..16], &u[20..24]),
            "333333333339"
        );
    }
    #[test]
    fn login_request_preserves_email_and_separates_query_values() {
        let request = login_request(
            &agent(),
            "https://gate.onetouchsmartcloud.com:8600",
            "home@example.test",
            "password",
        )
        .unwrap();
        assert_eq!(request.url(), "https://gate.onetouchsmartcloud.com:8600/login?user=krph@hadpsoh.whvw&passwd=696d29e0940a4957748fe3fc9efd22a3");
        assert_eq!(request.header("api-version"), Some("7.11"));
        let request = login_request(
            &agent(),
            "https://gate.onetouchsmartcloud.com:8600",
            "a&passwd=oops+#@example.test",
            "password",
        )
        .unwrap();
        let url = Url::parse(request.url()).unwrap();
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].1, shift("a&passwd=oops+#@example.test", 3));
        assert!(url.fragment().is_none());
    }
    #[test]
    fn smart_cloud_sends_oem_header_on_login_and_authenticated_requests() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                requests.push(String::from_utf8(bytes).unwrap());
                let body = r#"{"err_code":"0","datas":{}}"#;
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
            requests
        });
        login_request(
            &smart_cloud_agent(),
            &origin,
            "home@example.test",
            "test-only",
        )
        .unwrap()
        .call()
        .unwrap();
        let session = Session {
            username: "home@example.test".into(),
            provider: Provider::SmartCloud,
            role_hint: String::new(),
            gate: origin.clone(),
            rest: origin.clone(),
            web: origin.clone(),
            relay: String::new(),
            token: "test-token".into(),
            refresh_token: String::new(),
        };
        session
            .get(&format!("{origin}/userconf"), "7.13", &[])
            .unwrap();
        let requests = server.join().unwrap();
        for request in &requests {
            assert!(request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("user-agent: Onetouchsmartcloud")));
        }
        assert!(requests[0].starts_with("GET /login?"));
        assert!(requests[1].contains("test-token"));
    }
    #[test]
    fn accepts_current_cloud_and_discovery_success_envelopes() {
        for response in [
            json!({"err_code":"0","datas":{"ok":true}}),
            json!({"err_code":0,"datas":{"ok":true}}),
            json!({"result":0,"datas":{"ok":true}}),
            json!({"result":0,"err_code":"0","datas":{"ok":true}}),
        ] {
            assert_eq!(api_data(response).unwrap(), json!({"ok":true}));
        }
        for response in [
            json!({"datas":{}}),
            json!({"result":1,"datas":{}}),
            json!({"result":1,"err_code":"0","datas":{}}),
            json!({"result":0,"err_code":"1000100004","datas":{}}),
            json!({"err_code":"0"}),
        ] {
            assert!(api_data(response).is_err());
        }
    }
    #[test]
    fn login_errors_identify_step_without_exposing_response_data() {
        let error = api_data(
            json!({"result":1,"message":"private-account-data","datas":{"token":"SECRET"}}),
        )
        .unwrap_err();
        let message = during("Signing in to OneTouch Smart Cloud", error).to_string();
        assert!(message.starts_with("Signing in to OneTouch Smart Cloud:"));
        assert!(message.contains("code 1"));
        assert!(!message.contains("private-account-data"));
        assert!(!message.contains("SECRET"));
    }
    #[test]
    fn secrets_stay_out_of_views_and_snapshots_are_not_videos() {
        let a = parse_account(&json!({"app_conf":{"uid":"u1","role":10},"dev_list":[{"mac":"001122334455","location":"Door","is_need_monitor":1,"rtsp_pwd":"SECRET"}]})).unwrap();
        assert!(a.devices[0].can_monitor);
        assert!(!serde_json::to_string(&a.devices)
            .unwrap()
            .contains("SECRET"));
        let r = parse_recordings(json!({"code":0,"data":{"list":[{"id":12,"picUrl":"https://storage.example/pic?secret=1"},{"id":13,"videoPlayUrl":"https://storage.example/movie?secret=2"}]}})).unwrap();
        assert!(!r.entries[0].has_video);
        assert!(r.entries[1].has_video);
        assert!(!serde_json::to_string(&r).unwrap().contains("secret"));
    }
    #[test]
    fn reject_wrong_services_and_bad_responses() {
        for s in [
            "http://gate.onetouchsmartcloud.com",
            "https://onetouchsmartcloud.com.evil.test",
            "https://user:pass@gate.onetouchsmartcloud.com",
            "https://gate.onetouchsmartcloud.com/login",
        ] {
            assert!(service(s).is_err());
        }
        assert!(service("gate.onetouchsmartcloud.com:8600").is_ok());
        assert!(matches!(
            api_data(json!({"result":-1,"err_code":"1000000001","message":"token invalid"})),
            Err(Error::Expired)
        ));
        assert!(matches!(
            api_data(json!({"result":1,"err_code":"1000100002"})),
            Err(Error::Expired)
        ));
        assert!(matches!(
            api_data(json!({"result":1,"err_code":"1000100004"})),
            Err(Error::Credentials)
        ));
        assert!(parse_recordings(json!({"code":0,"data":{}})).is_err());
        assert!(media_url("file:///etc/passwd".into()).is_err());
    }
    #[test]
    fn private_atomic_storage() {
        let dir = std::env::temp_dir().join(format!("kagaz-onetouch-{}", uuid::Uuid::new_v4()));
        let p = dir.join("session.json");
        save_private(&p, b"first").unwrap();
        save_private(&p, b"second").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
