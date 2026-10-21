//! Signing in to a Xiaomi account, the way the Mi Home app's account library
//! does: the password (MD5, as Xiaomi expects), Xiaomi's picture captcha, and
//! the email or phone code Xiaomi asks for from a device it does not know;
//! or a QR code scanned in the Mi Home app, which needs none of those.
//!
//! Xiaomi answers the "identity" step (the code for a device it does not
//! know) with a web page rather than data; Kagaz goes on from the page's
//! own requests, and any page it cannot read comes back as
//! [`XiaomiError::Page`] so the window can show it to the person instead.
//!
//! Every exchange is noted in `<cache>/xiaomi-login.log` (private): the
//! address without its query values, the status, cookie names and the
//! answer's shape. Passwords, codes, tokens and cookie values never go in.

use super::api::Cloud;
use base64::Engine;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ACCOUNT: &str = "https://account.xiaomi.com";
const SID: &str = "xiaomiio";
const CALLBACK: &str = "https://sts.api.io.mi.com/sts";
/// `?sid=xiaomiio&_json=true`, already encoded, as the account library sends it.
const QS: &str = "%3Fsid%3Dxiaomiio%26_json%3Dtrue";
const SDK_VERSION: &str = "accountsdk-18.8.15";

#[derive(Debug, thiserror::Error)]
pub enum XiaomiError {
    #[error("cannot reach Xiaomi: {0}")]
    Transport(String),
    #[error("wrong account name or password")]
    WrongPassword,
    #[error("Xiaomi did not accept that code; try again")]
    WrongCode,
    #[error("Xiaomi did not accept the sign-in: {0}")]
    Refused(String),
    #[error("Xiaomi wants this finished on its own page ({title}): {url}")]
    Page { url: String, title: String },
    #[error("the saved Xiaomi sign-in is no longer valid; sign in again")]
    SignedOut,
    #[error("the QR code expired before it was scanned")]
    QrExpired,
    #[error("Xiaomi answered something unexpected at {step}: {detail}")]
    Unexpected { step: String, detail: String },
    #[error("cannot save or read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub(super) fn unexpected(step: &str, detail: impl Into<String>) -> XiaomiError {
    XiaomiError::Unexpected {
        step: step.into(),
        detail: detail.into(),
    }
}

/// A signed-in account: the user ID and pass token the cloud takes, plus the device ID Xiaomi saw at sign-in, kept so that a
/// device it was told to trust stays trusted.
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub user_id: String,
    pub pass_token: String,
    pub device_id: String,
    /// Server regions where the account's devices were found (`cn`, `de`,
    /// `i2`, `ru`, `sg`, `us`); empty until the first listing.
    #[serde(default)]
    pub regions: Vec<String>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("user_id", &self.user_id)
            .field("regions", &self.regions)
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("xiaomi-session.json")
    }

    pub fn save(&self, path: &Path) -> Result<(), XiaomiError> {
        write_private(path, &serde_json::to_vec_pretty(self).expect("json"))
    }

    pub fn load(path: &Path) -> Result<Session, XiaomiError> {
        let text = std::fs::read(path).map_err(|source| XiaomiError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_slice(&text).map_err(|e| XiaomiError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })
    }

    /// Open the cloud with the kept pass token: Xiaomi hands back this
    /// visit's `ssecurity` and service token. The pass token may be renewed
    /// on the way; save the session afterwards when it changed.
    pub fn connect(&mut self) -> Result<Cloud, XiaomiError> {
        let mut c = Client::new(&self.device_id);
        c.jar.set("userId", &self.user_id);
        c.jar.set("passToken", &self.pass_token);
        let r = c.get("token", &format!("{ACCOUNT}/pass/serviceLogin?_json=true&sid={SID}"))?;
        let v = r.json().ok_or(XiaomiError::SignedOut)?;
        let ssecurity = b64_field(&v, "ssecurity").ok_or(XiaomiError::SignedOut)?;
        if let Some(t) = str_field(&v, "passToken").filter(|t| !t.is_empty()) {
            self.pass_token = t;
        }
        let location = str_field(&v, "location").ok_or(XiaomiError::SignedOut)?;
        c.follow("token-sts", &location)?;
        let service_token = c
            .jar
            .get("serviceToken")
            .ok_or_else(|| unexpected("token-sts", "no service token"))?;
        Ok(Cloud::new(&self.user_id, ssecurity, &service_token, &c.agent_name))
    }
}

/// What a sign-in needs next.
#[derive(Debug)]
pub enum Step {
    Done(Session),
    /// A picture of characters to type in (PNG or JPEG bytes).
    Captcha(Vec<u8>),
    /// Xiaomi sent a code, by email or text message, to `to` (masked).
    Code { by_email: bool, to: String },
}

/// A password sign-in in progress. It can be kept between commands (a CLI
/// run per step) with [`Login::save`]: it then holds the password hash, so
/// the file is private and short-lived.
#[derive(Serialize, Deserialize)]
pub struct Login {
    user: String,
    hash: String,
    device_id: String,
    jar: Jar,
    sign: String,
    /// The return values step one hands out, sent back with the password.
    #[serde(default)]
    qs: String,
    #[serde(default)]
    callback: String,
    /// Set once Xiaomi asked for a code: the identity context and which
    /// way the code goes (4 = phone, 8 = email).
    #[serde(default)]
    context: String,
    #[serde(default)]
    flag: u32,
    #[serde(default)]
    started: u64,
}

impl Login {
    /// A new sign-in. `device_id` should be the one from an earlier session
    /// of this account, if any, so a trusted device is recognised.
    pub fn new(user: &str, password: &str, device_id: Option<&str>) -> Login {
        let hash = Md5::digest(password.as_bytes());
        Login {
            user: user.trim().to_string(),
            hash: hash.iter().map(|b| format!("{b:02X}")).collect(),
            device_id: device_id.map(str::to_string).unwrap_or_else(new_device_id),
            jar: Jar::default(),
            sign: String::new(),
            qs: String::new(),
            callback: String::new(),
            context: String::new(),
            flag: 0,
            started: now_secs(),
        }
    }

    pub fn pending_path() -> PathBuf {
        crate::paths::config_dir().join("xiaomi-login-pending.json")
    }

    /// The sign-in's cookies on Xiaomi's account site, for carrying it on
    /// in a web view when Xiaomi wants one of its own pages.
    pub fn cookies(&self) -> Vec<(String, String)> {
        self.jar.pairs()
    }

    /// The device ID Xiaomi knows this sign-in by.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn save(&self, path: &Path) -> Result<(), XiaomiError> {
        write_private(path, &serde_json::to_vec(self).expect("json"))
    }

    /// A kept sign-in, if it is younger than 15 minutes.
    pub fn load(path: &Path) -> Option<Login> {
        let l: Login = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        (now_secs().saturating_sub(l.started) < 15 * 60).then_some(l)
    }

    fn client(&self) -> Client {
        let mut c = Client::new(&self.device_id);
        c.jar = self.jar.clone();
        c
    }

    /// Send the account name and password.
    pub fn begin(&mut self) -> Result<Step, XiaomiError> {
        let mut c = self.client();
        Trace::start(&self.user);
        let r = c.get("start", &format!("{ACCOUNT}/pass/serviceLogin?sid={SID}&_json=true"))?;
        let v = r.json().ok_or_else(|| r.page_error("start"))?;
        self.sign = str_field(&v, "_sign").unwrap_or_default();
        self.qs = str_field(&v, "qs").unwrap_or_else(|| "?sid=xiaomiio&_json=true".into());
        self.callback = str_field(&v, "callback").unwrap_or_else(|| CALLBACK.into());
        self.jar = c.jar;
        self.password(None)
    }

    /// Answer a captcha (for the password, or for sending the code).
    pub fn captcha(&mut self, text: &str) -> Result<Step, XiaomiError> {
        if self.context.is_empty() {
            self.password(Some(text.trim()))
        } else {
            self.send_code(text.trim())
        }
    }

    /// Answer the emailed or texted code.
    pub fn code(&mut self, code: &str) -> Result<Step, XiaomiError> {
        if self.context.is_empty() {
            return Err(unexpected("code", "no code was asked for"));
        }
        let mut c = self.client();
        let name = self.way();
        let ick = c.jar.get("ick").unwrap_or_default();
        let url = format!(
            "{ACCOUNT}/identity/auth/verify{name}?_flag={}&_json=true&sid={SID}&context={}&mask=0&_locale=en_US",
            self.flag,
            enc(&self.context)
        );
        let flag = self.flag.to_string();
        let r = c.post(
            "verify",
            &url,
            &[
                ("_flag", flag.as_str()),
                ("ticket", code.trim()),
                ("trust", "true"),
                ("_json", "true"),
                ("ick", ick.as_str()),
            ],
        )?;
        let mut next = None;
        if let Some(v) = r.json() {
            if let Some(loc) = str_field(&v, "location").filter(|l| !l.is_empty()) {
                next = Some(loc);
            } else if v.get("code").and_then(Value::as_i64).unwrap_or(0) != 0 {
                self.jar = c.jar;
                return Err(XiaomiError::WrongCode);
            }
        } else if let Some(i) = r.text().find("https://account.xiaomi.com/identity/result/check?") {
            let rest = &r.text()[i..];
            let end = rest.find(['"', '\'']).unwrap_or(rest.len());
            next = Some(rest[..end].replace("&amp;", "&"));
        }
        let next = next.unwrap_or_else(|| {
            format!(
                "{ACCOUNT}/identity/result/check?sid={SID}&context={}&_locale=en_US",
                enc(&self.context)
            )
        });
        let end = c.follow("finish", &next)?;
        self.jar = c.jar.clone();
        self.done_or_page(&c, end)
    }

    /// Done when the chain left the pass token; otherwise Xiaomi stopped on
    /// one of its own pages (such as adding an email to the account), which
    /// only that page can finish.
    fn done_or_page(&self, c: &Client, end: Followed) -> Result<Step, XiaomiError> {
        if c.jar.get("passToken").is_some_and(|t| !t.is_empty()) {
            return self.done(c);
        }
        let title = if end.url.contains("/userinfo/") || end.url.contains("/security/info") {
            "add security information to the account".to_string()
        } else {
            end.title
        };
        Trace::note(&format!("stopped on a page: {}", bare(&end.url)));
        Err(XiaomiError::Page { url: end.url, title })
    }

    fn way(&self) -> &'static str {
        if self.flag == 4 { "Phone" } else { "Email" }
    }

    fn password(&mut self, captcha: Option<&str>) -> Result<Step, XiaomiError> {
        let mut c = self.client();
        let mut form = vec![
            ("_json", "true"),
            ("hash", self.hash.as_str()),
            ("sid", SID),
            ("callback", self.callback.as_str()),
            ("_sign", self.sign.as_str()),
            ("qs", self.qs.as_str()),
            ("user", self.user.as_str()),
        ];
        if let Some(t) = captcha {
            form.push(("captCode", t));
        }
        let r = c.post("password", &format!("{ACCOUNT}/pass/serviceLoginAuth2"), &form)?;
        self.jar = c.jar.clone();
        let v = r.json().ok_or_else(|| r.page_error("password"))?;
        if let Some(url) = str_field(&v, "captchaUrl").or_else(|| str_field(&v, "captchaURL")) {
            if !url.is_empty() && url != "null" {
                return self.fetch_captcha(&url);
            }
        }
        if b64_field(&v, "ssecurity").is_some_and(|s| s.len() > 4) {
            // Known device, right password: finish through the location.
            if let Some(loc) = str_field(&v, "location").filter(|l| !l.is_empty()) {
                c.follow("finish", &loc)?;
            }
            for k in ["passToken", "userId"] {
                if let Some(x) = str_field(&v, k) {
                    c.jar.set(k, &x);
                }
            }
            self.jar = c.jar.clone();
            return self.done(&c);
        }
        if let Some(url) = str_field(&v, "notificationUrl").filter(|u| !u.is_empty()) {
            return self.identity(&url);
        }
        match v.get("code").and_then(Value::as_i64) {
            Some(70016) => Err(XiaomiError::WrongPassword),
            Some(87001) => Err(XiaomiError::WrongCode),
            _ => Err(XiaomiError::Refused(describe(&v))),
        }
    }

    fn fetch_captcha(&mut self, url: &str) -> Result<Step, XiaomiError> {
        let mut c = self.client();
        let full = if url.starts_with('/') { format!("{ACCOUNT}{url}") } else { url.to_string() };
        let r = c.get("captcha", &full)?;
        self.jar = c.jar;
        if r.status != 200 || r.body.is_empty() {
            return Err(unexpected("captcha", format!("HTTP {}", r.status)));
        }
        Ok(Step::Captcha(r.body))
    }

    /// Xiaomi wants the person to prove it is them: open the identity
    /// session, see which ways it offers, and send a code.
    fn identity(&mut self, notification_url: &str) -> Result<Step, XiaomiError> {
        let mut c = self.client();
        self.context = query_value(notification_url, "context").unwrap_or_default();
        // The page itself only sets the identity session cookie.
        c.follow("identity", notification_url)?;
        let list = c.get(
            "identity-list",
            &format!(
                "{ACCOUNT}/identity/list?sid={SID}&context={}&_locale=en_US",
                enc(&self.context)
            ),
        )?;
        self.flag = list
            .json()
            .and_then(|v| v.get("flag").and_then(Value::as_u64))
            .map(|f| f as u32)
            .filter(|f| *f == 4 || *f == 8)
            .unwrap_or(8);
        self.jar = c.jar;
        if self.context.is_empty() {
            return Err(XiaomiError::Page {
                url: notification_url.to_string(),
                title: "identity check".into(),
            });
        }
        match self.send_code("") {
            // The other way, if the first one is not set up on the account.
            Err(XiaomiError::Refused(_)) => {
                self.flag = if self.flag == 8 { 4 } else { 8 };
                self.send_code("").map_err(|_| XiaomiError::Page {
                    url: notification_url.to_string(),
                    title: "identity check".into(),
                })
            }
            other => other,
        }
    }

    fn send_code(&mut self, icode: &str) -> Result<Step, XiaomiError> {
        let mut c = self.client();
        let name = self.way();
        // Where the code goes, masked (best effort).
        let mut to = String::new();
        if let Ok(r) = c.get(
            "identity-way",
            &format!(
                "{ACCOUNT}/identity/auth/verify{name}?_flag={}&_json=true&sid={SID}&context={}&_locale=en_US",
                self.flag,
                enc(&self.context)
            ),
        ) {
            if let Some(v) = r.json() {
                to = str_field(&v, "maskedPhone")
                    .or_else(|| str_field(&v, "maskedEmail"))
                    .unwrap_or_default();
            }
        }
        let ick = c.jar.get("ick").unwrap_or_default();
        let url = format!(
            "{ACCOUNT}/identity/auth/send{name}Ticket?_dc={}&sid={SID}&context={}&mask=0&_locale=en_US",
            now_millis(),
            enc(&self.context)
        );
        let r = c.post(
            "send-code",
            &url,
            &[("retry", "0"), ("icode", icode), ("_json", "true"), ("ick", ick.as_str())],
        )?;
        self.jar = c.jar;
        let v = r.json().ok_or_else(|| r.page_error("send-code"))?;
        if let Some(url) = str_field(&v, "captchaUrl").or_else(|| str_field(&v, "captchaURL")) {
            if !url.is_empty() && url != "null" {
                return self.fetch_captcha(&url);
            }
        }
        match v.get("code").and_then(Value::as_i64) {
            Some(0) | None => Ok(Step::Code {
                by_email: self.flag != 4,
                to,
            }),
            Some(87001) => Err(XiaomiError::WrongCode),
            _ => Err(XiaomiError::Refused(describe(&v))),
        }
    }

    fn done(&self, c: &Client) -> Result<Step, XiaomiError> {
        let user_id = c.jar.get("userId");
        let pass_token = c.jar.get("passToken");
        match (user_id, pass_token) {
            (Some(user_id), Some(pass_token)) if !pass_token.is_empty() => {
                Trace::note("signed in");
                Ok(Step::Done(Session {
                    user_id,
                    pass_token,
                    device_id: self.device_id.clone(),
                    regions: Vec::new(),
                }))
            }
            _ => Err(unexpected("finish", "no pass token after the sign-in")),
        }
    }
}

/// A sign-in by QR code: the person scans it with the Mi Home app (or opens
/// `login_url` on a phone signed in to Xiaomi) and confirms.
pub struct QrLogin {
    pub login_url: String,
    pub image_url: String,
    /// Xiaomi's own words on where to scan it.
    pub tips: String,
    poll_url: String,
    deadline: Instant,
    client: Client,
}

impl QrLogin {
    pub fn start(device_id: Option<&str>) -> Result<QrLogin, XiaomiError> {
        Trace::start("qr");
        let device_id = device_id.map(str::to_string).unwrap_or_else(new_device_id);
        let mut c = Client::new(&device_id);
        let url = format!(
            "{ACCOUNT}/longPolling/loginUrl?_qrsize=480&qs={QS}&callback={}&_hasLogo=false&sid={SID}&serviceParam=&_locale=en_GB&_dc={}",
            enc(CALLBACK),
            now_millis()
        );
        let r = c.get("qr", &url)?;
        let v = r.json().ok_or_else(|| r.page_error("qr"))?;
        let field = |k: &str| str_field(&v, k).ok_or_else(|| unexpected("qr", format!("no {k}")));
        let timeout = v.get("timeout").and_then(Value::as_u64).unwrap_or(300);
        Ok(QrLogin {
            login_url: field("loginUrl")?,
            image_url: field("qr")?,
            tips: str_field(&v, "qrTips").unwrap_or_default(),
            poll_url: field("lp")?,
            deadline: Instant::now() + Duration::from_secs(timeout.clamp(30, 600)),
            client: c,
        })
    }

    /// Xiaomi's own picture of the code.
    pub fn image(&mut self) -> Result<Vec<u8>, XiaomiError> {
        let url = self.image_url.clone();
        Ok(self.client.get("qr-image", &url)?.body)
    }

    /// Wait until the code is scanned and confirmed, or it expires.
    pub fn wait(self) -> Result<Session, XiaomiError> {
        self.wait_while(|| true)
    }

    /// The same, giving up when `keep_going` says so (checked between
    /// Xiaomi's long polls, so within about 40 seconds).
    pub fn wait_while(mut self, keep_going: impl Fn() -> bool) -> Result<Session, XiaomiError> {
        let url = self.poll_url.clone();
        while Instant::now() < self.deadline && keep_going() {
            let r = match self.client.get("qr-wait", &url) {
                Ok(r) => r,
                // A long poll that ran out: ask again.
                Err(XiaomiError::Transport(_)) => continue,
                Err(e) => return Err(e),
            };
            if r.status != 200 {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            let Some(v) = r.json() else { continue };
            let (Some(user_id), Some(pass_token)) =
                (str_field(&v, "userId"), str_field(&v, "passToken"))
            else {
                continue;
            };
            Trace::note("signed in by QR code");
            return Ok(Session {
                user_id,
                pass_token,
                device_id: self.client.device_id.clone(),
                regions: Vec::new(),
            });
        }
        Err(XiaomiError::QrExpired)
    }
}

// ---- HTTP with a small cookie jar ----

#[derive(Default, Clone, Serialize, Deserialize)]
pub(super) struct Jar(BTreeMap<String, String>);

impl Jar {
    pub(super) fn set(&mut self, k: &str, v: &str) {
        self.0.insert(k.to_string(), v.to_string());
    }
    pub(super) fn get(&self, k: &str) -> Option<String> {
        self.0.get(k).cloned()
    }
    pub(super) fn pairs(&self) -> Vec<(String, String)> {
        self.0.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
    fn header(&self) -> String {
        self.0
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
    /// Take the response's cookies; returns their names for the log.
    fn take(&mut self, resp: &ureq::Response) -> Vec<String> {
        let mut names = Vec::new();
        for line in resp.all("set-cookie") {
            let pair = line.split(';').next().unwrap_or("");
            let Some((k, v)) = pair.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            if k.is_empty() {
                continue;
            }
            if v.is_empty() || v == "EXPIRED" {
                self.0.remove(k);
            } else {
                self.0.insert(k.to_string(), v.to_string());
            }
            names.push(k.to_string());
        }
        names
    }
}

pub(super) struct Client {
    agent: ureq::Agent,
    pub(super) jar: Jar,
    device_id: String,
    pub(super) agent_name: String,
    /// Headers sent with every request (a web page's Origin, Referer...).
    pub(super) headers: Vec<(String, String)>,
}

/// Where a redirect chain ended.
pub(super) struct Followed {
    #[allow(dead_code)]
    pub(super) ssecurity: Option<Vec<u8>>,
    pub(super) url: String,
    pub(super) title: String,
}

pub(super) struct Reply {
    pub url: String,
    pub status: u16,
    pub location: Option<String>,
    pub pragma: Option<String>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }
    /// The answer as JSON, with Xiaomi's `&&&START&&&` prefix removed.
    pub fn json(&self) -> Option<Value> {
        let t = self.text().trim_start();
        let t = t.strip_prefix("&&&START&&&").unwrap_or(t);
        serde_json::from_str(t).ok()
    }
    pub(super) fn page_error(&self, step: &str) -> XiaomiError {
        if looks_html(self.text()) {
            XiaomiError::Page {
                url: self.url.clone(),
                title: html_title(self.text()),
            }
        } else {
            unexpected(step, format!("HTTP {}", self.status))
        }
    }
}

impl Client {
    pub(super) fn new(device_id: &str) -> Client {
        let agent = ureq::AgentBuilder::new()
            .tls_config(crate::cameras::tapo::tls::client_config())
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(40))
            .redirects(0)
            .build();
        let mut jar = Jar::default();
        jar.set("deviceId", device_id);
        jar.set("sdkVersion", SDK_VERSION);
        Client {
            agent,
            jar,
            device_id: device_id.to_string(),
            agent_name: user_agent(device_id),
            headers: Vec::new(),
        }
    }

    pub(super) fn get(&mut self, step: &str, url: &str) -> Result<Reply, XiaomiError> {
        self.send(step, "GET", url, None, &[])
    }

    pub(super) fn post(&mut self, step: &str, url: &str, form: &[(&str, &str)]) -> Result<Reply, XiaomiError> {
        self.send(step, "POST", url, Some(form), &[])
    }

    pub(super) fn send(
        &mut self,
        step: &str,
        method: &str,
        url: &str,
        form: Option<&[(&str, &str)]>,
        extra: &[(&str, &str)],
    ) -> Result<Reply, XiaomiError> {
        let mut req = self
            .agent
            .request(method, url)
            .set("User-Agent", &self.agent_name)
            .set("Cookie", &self.jar.header());
        for (k, v) in &self.headers {
            req = req.set(k, v);
        }
        for (k, v) in extra {
            req = req.set(k, v);
        }
        let res = match form {
            Some(f) => req.send_form(f),
            None => req.call(),
        };
        let resp = match res {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => {
                let why = without_values(&e.to_string());
                Trace::note(&format!("{step}: {method} {} failed: {why}", bare(url)));
                return Err(XiaomiError::Transport(why));
            }
        };
        let status = resp.status();
        let location = resp.header("location").map(|l| absolute(url, l));
        let pragma = resp.header("extension-pragma").map(str::to_string);
        let ctype = resp.header("content-type").unwrap_or("").to_string();
        let cookies = self.jar.take(&resp);
        let mut body = Vec::new();
        let _ = resp.into_reader().take(4 << 20).read_to_end(&mut body);
        let reply = Reply {
            url: url.to_string(),
            status,
            location,
            pragma,
            body,
        };
        Trace::reply(step, method, url, &reply, &ctype, &cookies);
        Ok(reply)
    }

    /// Follow a redirect chain by hand, keeping every cookie, and note the
    /// `ssecurity` Xiaomi puts in a header on the way.
    pub(super) fn follow(&mut self, step: &str, start: &str) -> Result<Followed, XiaomiError> {
        let mut url = absolute(ACCOUNT, start);
        let mut ssecurity = None;
        let mut tips_retry = true;
        let mut title = String::new();
        for _ in 0..10 {
            let r = self.get(step, &url)?;
            title = if looks_html(r.text()) { html_title(r.text()) } else { String::new() };
            if let Some(p) = r.pragma.as_deref().and_then(|p| serde_json::from_str::<Value>(p).ok()) {
                if let Some(s) = b64_field(&p, "ssecurity") {
                    ssecurity = Some(s);
                }
            }
            // An interstitial "Tips" page answers once, then the redirect comes.
            if r.status == 200 && tips_retry && url.contains("serviceLoginAuth2/end") && r.text().contains("Tips") {
                tips_retry = false;
                continue;
            }
            match r.location {
                Some(next) if (300..400).contains(&r.status) => url = next,
                _ => break,
            }
        }
        Ok(Followed { ssecurity, url, title })
    }
}

// ---- the private log ----

pub(super) struct Trace;

impl Trace {
    fn path() -> PathBuf {
        crate::paths::cache_dir().join("xiaomi-login.log")
    }

    pub(super) fn start(what: &str) {
        if cfg!(test) {
            return;
        }
        let kind = if what == "qr" { "QR code" } else if what == "phone" { "phone number and text message" } else if what.contains('@') { "email" } else if what.starts_with('+') || what.chars().all(|c| c.is_ascii_digit()) { "number" } else { "name" };
        let _ = write_private(&Self::path(), format!("{} sign-in by {kind}\n", stamp()).as_bytes());
    }

    pub(super) fn note(line: &str) {
        if cfg!(test) {
            return;
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(Self::path()) {
            let _ = writeln!(f, "{} {line}", stamp());
        }
    }

    fn reply(step: &str, method: &str, url: &str, r: &Reply, ctype: &str, cookies: &[String]) {
        let mut line = format!("{step}: {method} {} -> {} {}", bare(url), r.status, ctype.split(';').next().unwrap_or(""));
        if let Some(l) = &r.location {
            line += &format!(" location={}", bare(l));
        }
        if r.pragma.is_some() {
            line += " (extension-pragma)";
        }
        if !cookies.is_empty() {
            line += &format!(" cookies=[{}]", cookies.join(","));
        }
        if let Some(v) = r.json() {
            line += &format!(" json {}", shape(&v));
        } else if looks_html(r.text()) {
            line += &format!(" html {}", page_summary(r.text()));
        } else {
            line += &format!(" {} bytes", r.body.len());
        }
        Self::note(&line);
    }
}

/// Keys of a JSON answer, with the values that say what happened (codes,
/// descriptions, flags) and the addresses' paths; never tokens.
fn shape(v: &Value) -> String {
    let Some(o) = v.as_object() else { return "(not an object)".into() };
    let safe = ["code", "result", "desc", "description", "flag", "options", "timeout", "securityStatus", "pwd", "notificationUrl", "captchaUrl", "location", "lp", "loginUrl"];
    let mut parts = Vec::new();
    for (k, val) in o {
        if safe.contains(&k.as_str()) {
            let shown = match val {
                Value::String(s) if s.starts_with("http") || s.starts_with('/') => bare(s),
                Value::String(s) => s.chars().take(80).collect(),
                other => other.to_string().chars().take(80).collect(),
            };
            parts.push(format!("{k}={shown}"));
        } else {
            parts.push(k.clone());
        }
    }
    format!("{{{}}}", parts.join(", "))
}

/// A web page's title, form targets, field names and opening words.
fn page_summary(html: &str) -> String {
    let mut out = format!("title={:?}", html_title(html));
    let lower = html.to_ascii_lowercase();
    for (tag, attr) in [("<form", "action"), ("<input", "name"), ("<script", "src"), ("<link", "href")] {
        let mut seen = Vec::new();
        let mut at = 0;
        while let Some(i) = lower[at..].find(tag) {
            let s = at + i;
            let e = lower[s..].find('>').map(|e| s + e).unwrap_or(lower.len());
            if let Some(v) = attr_value(&html[s..e], attr) {
                seen.push(bare(&v));
            }
            at = e.max(s + 1);
        }
        if !seen.is_empty() {
            out += &format!(" {attr}s=[{}]", seen.join(","));
        }
    }
    let text = visible_text(html);
    out += &format!(" text={:?}", text.chars().take(300).collect::<String>());
    out
}

fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let i = lower.find(&format!("{attr}="))? + attr.len() + 1;
    let rest = &tag[i..];
    let (q, rest) = match rest.chars().next()? {
        c @ ('"' | '\'') => (c, &rest[1..]),
        _ => (' ', rest),
    };
    Some(rest[..rest.find(q).unwrap_or(rest.len())].to_string())
}

fn visible_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let lower = html.to_ascii_lowercase();
    let mut skip_until: Option<&str> = None;
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if let Some(end) = skip_until {
            match lower[i..].find(end) {
                Some(j) => {
                    i += j + end.len();
                    skip_until = None;
                    continue;
                }
                None => break,
            }
        }
        if lower[i..].starts_with("<script") {
            skip_until = Some("</script>");
            continue;
        }
        if lower[i..].starts_with("<style") {
            skip_until = Some("</style>");
            continue;
        }
        let c = html[i..].chars().next().unwrap_or(' ');
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
        i += c.len_utf8();
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn html_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(s) = lower.find("<title>") else { return String::new() };
    let e = lower[s..].find("</title>").map(|e| s + e).unwrap_or(s + 7);
    html[s + 7..e].trim().to_string()
}

fn looks_html(t: &str) -> bool {
    let t = t.trim_start().to_ascii_lowercase();
    t.starts_with("<!doctype") || t.starts_with("<html")
}

/// An address without its query values (names kept).
pub(super) fn bare(url: &str) -> String {
    match url.split_once('?') {
        Some((base, q)) => {
            let names: Vec<&str> = q.split('&').map(|p| p.split('=').next().unwrap_or("")).collect();
            format!("{base}?{}", names.join("&"))
        }
        None => url.to_string(),
    }
}

/// Text with every address in it reduced to [`bare`].
fn without_values(text: &str) -> String {
    text.split(' ')
        .map(|w| if w.contains("://") { bare(w.trim_end_matches(':')) + if w.ends_with(':') { ":" } else { "" } } else { w.to_string() })
        .collect::<Vec<_>>()
        .join(" ")
}

fn absolute(base: &str, loc: &str) -> String {
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return loc.to_string();
    }
    match url::Url::parse(base).and_then(|b| b.join(loc)) {
        Ok(u) => u.to_string(),
        Err(_) => loc.to_string(),
    }
}

fn query_value(url: &str, key: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

pub(super) fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub(super) fn str_field(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn b64_field(v: &Value, k: &str) -> Option<Vec<u8>> {
    let s = v.get(k)?.as_str()?;
    base64::engine::general_purpose::STANDARD.decode(s).ok().filter(|b| !b.is_empty())
}

pub(super) fn describe(v: &Value) -> String {
    let code = v.get("code").map(|c| c.to_string()).unwrap_or_default();
    let desc = str_field(v, "desc")
        .or_else(|| str_field(v, "description"))
        .unwrap_or_default();
    format!("{desc} (code {code})").trim().to_string()
}

fn new_device_id() -> String {
    let id = uuid::Uuid::new_v4();
    id.as_bytes().iter().take(6).map(|x| (b'a' + x % 26) as char).collect()
}

/// The account library's user agent shape, stable per device ID.
fn user_agent(device_id: &str) -> String {
    let h = Md5::digest(device_id.as_bytes());
    let text: String = h.iter().take(13).map(|x| (b'A' + x % 5) as char).collect();
    let pre: String = h.iter().cycle().skip(3).take(18).map(|x| (b'a' + x % 26) as char).collect();
    format!("{pre}-{text} APP/com.xiaomi.mihome APPV/10.5.201")
}

pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn stamp() -> String {
    let s = now_secs();
    format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60)
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), XiaomiError> {
    let io = |source| XiaomiError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(io)?;
        f.write_all(bytes).map_err(io)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes).map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_password_goes_as_upper_case_md5() {
        let l = Login::new(" someone@example.com ", "secret", Some("abcdef"));
        assert_eq!(l.user, "someone@example.com");
        assert_eq!(l.hash, "5EBE2294ECD0E0F08EAB7690D2A6EE69");
        assert_eq!(l.device_id, "abcdef");
    }

    #[test]
    fn json_answers_lose_their_prefix() {
        let r = Reply {
            url: String::new(),
            status: 200,
            location: None,
            pragma: None,
            body: b"&&&START&&&{\"code\":0,\"_sign\":\"x\"}".to_vec(),
        };
        assert_eq!(r.json().unwrap()["code"], 0);
    }

    #[test]
    fn the_log_keeps_no_values() {
        assert_eq!(
            bare("https://account.xiaomi.com/identity/list?sid=xiaomiio&context=SECRET"),
            "https://account.xiaomi.com/identity/list?sid&context"
        );
        let v: Value = serde_json::from_str(r#"{"code":70016,"passToken":"SECRET","desc":"wrong"}"#).unwrap();
        let s = shape(&v);
        assert!(s.contains("code=70016") && s.contains("passToken") && !s.contains("SECRET"));
    }

    #[test]
    fn a_page_is_summed_up_without_its_values() {
        let html = r#"<!DOCTYPE html><html><head><title>Verify</title><script>var x=1;</script></head>
            <body><form action="/identity/auth/verifyEmail?context=SECRET"><input name="ticket" value="SECRET">
            <p>Enter the code we sent</p></form></body></html>"#;
        let s = page_summary(html);
        assert!(s.contains("title=\"Verify\""));
        assert!(s.contains("/identity/auth/verifyEmail?context"));
        assert!(s.contains("names=[ticket]"));
        assert!(s.contains("Enter the code we sent"));
        assert!(!s.contains("SECRET") && !s.contains("var x"));
    }

    #[test]
    fn errors_lose_their_addresses_values() {
        assert_eq!(
            without_values("https://lp.example.com/lp/s?k=SECRET: Network Error: timed out"),
            "https://lp.example.com/lp/s?k: Network Error: timed out"
        );
    }

    #[test]
    fn relative_addresses_are_made_whole() {
        assert_eq!(
            absolute(ACCOUNT, "/identity/set/security/info/strat"),
            "https://account.xiaomi.com/identity/set/security/info/strat"
        );
        assert_eq!(absolute(ACCOUNT, "https://sts.api.io.mi.com/sts?x=1"), "https://sts.api.io.mi.com/sts?x=1");
    }

    #[test]
    fn the_user_agent_stays_the_same_for_a_device() {
        assert_eq!(user_agent("abcdef"), user_agent("abcdef"));
        assert!(user_agent("abcdef").ends_with("APP/com.xiaomi.mihome APPV/10.5.201"));
    }
}
