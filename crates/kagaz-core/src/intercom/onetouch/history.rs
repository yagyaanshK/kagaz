//! The account's history and family, as the OneTouch Smart Cloud's own
//! Activities and family pages show them.
//!
//! - Door log: every ring and door opening at a door station, most with a
//!   picture of who was there (and a video when the station records one).
//! - Call log: calls to and from the account's apps: answered, missed,
//!   outgoing.
//! - Captures: pictures taken by hand during a live view (often empty).
//!
//! The current web pages ask `v4/smartplus/<project>/log/<role>/get…Log`
//! (camelCase fields, the list under `data.list`); older ones asked
//! `v3/app/<project>/log/get…Log` (PascalCase fields, the list as `data`).
//! Both are tried, newest first, and either shape is read. A page asks for
//! 20 rows; the server sends one more when there are more.
//!
//! Family members (each with their own login, up to ten) come from
//! `v3/app/<project>/user/getSubList`.

use super::{field, media_url, protocol, Error, Provider, Session};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ROWS: usize = 20;

/// Which history to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Log {
    Door,
    Calls,
    Captures,
}

impl Log {
    fn name(self) -> &'static str {
        match self {
            Log::Door => "Door",
            Log::Calls => "Call",
            Log::Captures => "Capture",
        }
    }
}

/// One thing that happened.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Event {
    pub id: String,
    /// "door", "call" or "capture".
    pub kind: String,
    /// When, as the cloud says it (the project's local time).
    pub at: String,
    /// The device, or for a call the other side.
    pub device: String,
    /// What happened, in words ("Rang", "Door opened", …).
    pub detail: String,
    /// Calls: "missed", "answered" or "outgoing".
    pub direction: String,
    pub picture: String,
    pub thumbnail: String,
    pub video: String,
    pub unread: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Page {
    pub entries: Vec<Event>,
    pub more: bool,
}

/// A family member: their own login on the same home.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Person {
    pub id: String,
    pub name: String,
    /// Their call number, when the cloud gives it.
    pub sip: String,
    /// The address their invitation went to, when the cloud gives it.
    pub email: String,
}

/// A new family member, as OneTouch's own "New Family Member" form asks for one.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NewMember {
    pub first_name: String,
    pub last_name: String,
    /// Where the login is emailed; optional when a mobile number is given.
    pub email: String,
    /// The country calling code for the mobile number, digits only ("91").
    pub phone_code: String,
    pub mobile: String,
    /// A note for the home's records (up to 255 characters).
    pub remark: String,
}

/// What came of inviting a family member.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Invited {
    /// The cloud emailed them an invitation (it may instead have texted a
    /// phone-only member, which Kagaz does not create).
    pub emailed: bool,
    /// The cloud's own words, when it gave any.
    pub message: String,
}

/// The project and role words of the account's role code.
fn place(role: &str) -> Result<(&'static str, &'static str), Error> {
    match role {
        "10" => Ok(("single", "master")),
        "11" => Ok(("single", "member")),
        "20" => Ok(("community", "master")),
        "21" => Ok(("community", "member")),
        _ => Err(protocol(
            "History for this kind of OneTouch account is not supported yet.",
        )),
    }
}

/// The first of several field names that has a value.
fn pick(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .map(|k| field(v, k))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

fn truthy(v: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|k| match &v[*k] {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        Value::String(s) => matches!(s.as_str(), "1" | "true"),
        _ => false,
    })
}

/// A media address, or nothing when it is not a plain https/http link.
fn media(v: &Value, keys: &[&str]) -> String {
    media_url(pick(v, keys)).unwrap_or_default()
}

fn check(v: &Value) -> Result<(), Error> {
    let code = field(v, "code");
    if matches!(
        code.as_str(),
        "2" | "1000000001" | "1000100001" | "1000100002" | "1000100003"
    ) {
        return Err(Error::Expired);
    }
    if code != "0" {
        return Err(protocol(&format!(
            "OneTouch could not read its history (code {code})."
        )));
    }
    Ok(())
}

/// A write's answer: ok when the cloud says code 0, else its own words.
fn accepted(v: &Value, what: &str) -> Result<(), Error> {
    match check(v) {
        Err(Error::Expired) => Err(Error::Expired),
        Err(_) => {
            let msg = field(v, "msg");
            Err(protocol(&if msg.is_empty() {
                format!("OneTouch did not {what} (code {}).", field(v, "code"))
            } else {
                format!("OneTouch did not {what}: {msg}")
            }))
        }
        Ok(()) => Ok(()),
    }
}

/// The answer to an invitation: `TipCode` 0 means an email went out, 1 a
/// text message (phone-only members, which the vendor's page offers only
/// without an email).
pub(crate) fn parse_invited(v: &Value) -> Result<Invited, Error> {
    accepted(v, "add the member")?;
    Ok(Invited {
        emailed: field(v, "TipCode") != "1",
        message: field(v, "msg"),
    })
}

/// The rows of either shape: `data.list` (v4) or `data` (v3).
fn rows(v: &Value) -> Result<Vec<Value>, Error> {
    check(v)?;
    v["data"]["list"]
        .as_array()
        .or_else(|| v["data"].as_array())
        .cloned()
        .ok_or_else(|| protocol("OneTouch's history list was missing."))
}

fn door_detail(item: &Value) -> String {
    let what = pick(item, &["captureType", "CaptureType", "CaptureAction"]);
    let failed = {
        let r = pick(item, &["response", "Response"]);
        !r.is_empty() && r != "0" && r != "success"
    };
    let words = match what.to_ascii_lowercase().as_str() {
        "call" => "Rang".to_string(),
        "" => String::new(),
        other if other.contains("unlock") || other.contains("open") => "Door opened".into(),
        _ => what.clone(),
    };
    match (failed, words.is_empty()) {
        (true, _) => "Could not open the door".into(),
        (false, true) => "Activity".into(),
        (false, false) => words,
    }
}

pub(crate) fn parse_door(item: &Value) -> Event {
    Event {
        id: pick(item, &["id", "ID", "UUID", "uuid"]),
        kind: "door".into(),
        at: pick(item, &["captureTime", "CaptureTime", "Time", "time"]),
        device: pick(item, &["deviceName", "Location", "sipAccount", "SipAccount"]),
        detail: door_detail(item),
        direction: String::new(),
        picture: media(item, &["picUrl", "PicUrl", "thirdCameraPicUrl", "ThirdCameraPicUrl"]),
        thumbnail: media(
            item,
            &["smallPicUrl", "SPicUrl", "thirdCameraSPicUrl", "ThirdCameraSPicUrl", "picUrl", "PicUrl"],
        ),
        video: media(item, &["videoPlayUrl", "VideoRecordUrl", "videoRecordUrl"]),
        unread: false,
    }
}

pub(crate) fn parse_call(item: &Value) -> Event {
    // v4: isCallee/isAnswer. v3: Type 1 missed, 2 answered, 3 outgoing.
    let v4 = !item["isCallee"].is_null();
    let (direction, device) = if v4 {
        let callee = truthy(item, &["isCallee"]);
        let direction = match (callee, truthy(item, &["isAnswer"])) {
            (true, true) => "answered",
            (true, false) => "missed",
            (false, _) => "outgoing",
        };
        let device = if callee {
            pick(item, &["callerName", "callerSipAccount"])
        } else {
            pick(item, &["calleeName", "calleeSipAccount"])
        };
        (direction, device)
    } else {
        let direction = match field(item, "Type").as_str() {
            "1" => "missed",
            "2" => "answered",
            "3" => "outgoing",
            _ => "",
        };
        (direction, pick(item, &["DevLocation", "DevSipNum"]))
    };
    let read = if v4 {
        truthy(item, &["isRead"])
    } else {
        field(item, "Status") == "1"
    };
    let group = truthy(item, &["isGroupCall", "IsGroupCall"]);
    Event {
        id: pick(item, &["id", "ID", "UUID"]),
        kind: "call".into(),
        at: pick(item, &["startTime", "Time", "time"]),
        device,
        detail: match direction {
            "missed" => "Missed call",
            "answered" => "Answered",
            "outgoing" => "You called",
            _ => "Call",
        }
        .to_string()
            + if group { " (rang everyone)" } else { "" },
        direction: direction.into(),
        picture: String::new(),
        thumbnail: String::new(),
        video: String::new(),
        unread: direction == "missed" && !read,
    }
}

pub(crate) fn parse_capture(item: &Value) -> Event {
    Event {
        id: pick(item, &["id", "ID"]),
        kind: "capture".into(),
        at: pick(item, &["captureTime", "CaptureTime", "Time"]),
        device: pick(item, &["deviceName", "Location"]),
        detail: "Picture taken".into(),
        direction: String::new(),
        picture: media(item, &["picUrl", "PicUrl"]),
        thumbnail: media(item, &["smallPicUrl", "SPicUrl", "picUrl", "PicUrl"]),
        video: media(item, &["videoPlayUrl", "VideoRecordUrl"]),
        unread: false,
    }
}

pub(crate) fn parse_page(v: &Value, log: Log) -> Result<Page, Error> {
    let list = rows(v)?;
    let more = list.len() > ROWS;
    let entries = list
        .iter()
        .take(ROWS)
        .map(|item| match log {
            Log::Door => parse_door(item),
            Log::Calls => parse_call(item),
            Log::Captures => parse_capture(item),
        })
        .collect();
    Ok(Page { entries, more })
}

pub(crate) fn parse_people(v: &Value) -> Result<Vec<Person>, Error> {
    Ok(rows(v)?
        .iter()
        .map(|item| {
            let name = {
                let n = pick(item, &["Name", "name", "UserName", "userName"]);
                if !n.is_empty() {
                    n
                } else {
                    let full = format!(
                        "{} {}",
                        pick(item, &["FirstName", "firstName"]),
                        pick(item, &["LastName", "lastName"])
                    );
                    let full = full.trim().to_string();
                    if full.is_empty() {
                        pick(item, &["Email", "email"])
                    } else {
                        full
                    }
                }
            };
            Person {
                id: pick(item, &["ID", "id", "UUID", "uuid", "Account", "account"]),
                name: if name.is_empty() { "Family member".into() } else { name },
                sip: pick(item, &["SipAccount", "sipAccount", "Sip", "sip"]),
                email: pick(item, &["Email", "email"]),
            }
        })
        .collect())
}

impl Session {
    fn web_get(&self, path: &str, page: u32) -> Result<Value, Error> {
        self.get(
            &format!("{}/web-server/{path}", self.web),
            "7.13",
            &[("page", page.max(1).to_string()), ("row", ROWS.to_string())],
        )
    }
    /// A form post to the web backend, as the vendor's pages send their
    /// `v3/` writes (the `v4/` ones take JSON; none is used here).
    fn web_post(&self, path: &str, form: &[(&str, &str)]) -> Result<Value, Error> {
        self.post_form(&format!("{}/web-server/{path}", self.web), form)
    }

    /// One page of a history, newest first: the newer list, else the
    /// older one (an empty newer list may just be the wrong place for this
    /// account).
    pub fn history(&mut self, role: &str, log: Log, page: u32) -> Result<Page, Error> {
        if self.provider == Provider::Otipvdp {
            return Err(protocol(
                "History is available for OneTouch Smart Cloud accounts.",
            ));
        }
        let (project, member) = place(role)?;
        let newer = format!("v4/smartplus/{project}/log/{member}/get{}Log", log.name());
        let older = format!("v3/app/{project}/log/get{}Log", log.name());
        for attempt in 0..2 {
            let first = self.web_get(&newer, page).and_then(|v| parse_page(&v, log));
            let result = match first {
                Ok(p) if !p.entries.is_empty() => Ok(p),
                Err(Error::Expired) => Err(Error::Expired),
                first => match self.web_get(&older, page).and_then(|v| parse_page(&v, log)) {
                    Ok(p) => Ok(p),
                    Err(Error::Expired) => Err(Error::Expired),
                    Err(e) => first.or(Err(e)),
                },
            };
            match result {
                Err(Error::Expired) if attempt == 0 => self.refresh()?,
                result => return result,
            }
        }
        Err(Error::Expired)
    }

    /// The family members of this home (their own logins).
    pub fn people(&mut self, role: &str) -> Result<Vec<Person>, Error> {
        if self.provider == Provider::Otipvdp {
            return Ok(Vec::new());
        }
        let (project, _) = place(role)?;
        let path = format!("v3/app/{project}/user/getSubList");
        for attempt in 0..2 {
            match self.web_get(&path, 1).and_then(|v| parse_people(&v)) {
                Err(Error::Expired) if attempt == 0 => self.refresh()?,
                result => return result,
            }
        }
        Err(Error::Expired)
    }

    /// Invite a family member: the fields of OneTouch's "New Family Member"
    /// form (first and last name, email, remark, country and mobile number),
    /// checked the way that form checks them. OneTouch makes their login and
    /// sends it by email (or by text message when only a mobile is given). A
    /// single home's form takes one Name, so the two names are joined there,
    /// and it has no Remark.
    pub fn invite(&mut self, role: &str, m: &NewMember) -> Result<Invited, Error> {
        if self.provider == Provider::Otipvdp {
            return Err(protocol(
                "Family members are managed on OneTouch Smart Cloud accounts.",
            ));
        }
        let form = member_form(role, m)?;
        let (project, _) = place(role)?;
        let path = format!("v3/app/{project}/subUser/add");
        let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
        for attempt in 0..2 {
            match self.web_post(&path, &form).and_then(|v| parse_invited(&v)) {
                Err(Error::Expired) if attempt == 0 => self.refresh()?,
                result => return result,
            }
        }
        Err(Error::Expired)
    }

    /// Remove a family member (their login loses this home; it is not
    /// deleted).
    pub fn remove_member(&mut self, role: &str, id: &str) -> Result<(), Error> {
        if self.provider == Provider::Otipvdp {
            return Err(protocol(
                "Family members are managed on OneTouch Smart Cloud accounts.",
            ));
        }
        if id.trim().is_empty() {
            return Err(protocol("OneTouch gave no id for this member."));
        }
        let (project, _) = place(role)?;
        let path = format!("v3/app/{project}/subUser/delete");
        for attempt in 0..2 {
            match self
                .web_post(&path, &[("ID", id.trim())])
                .and_then(|v| accepted(&v, "remove the member"))
            {
                Err(Error::Expired) if attempt == 0 => self.refresh()?,
                result => return result,
            }
        }
        Err(Error::Expired)
    }
}

/// The form a new member is posted with, after the checks OneTouch's own
/// page makes (names, a valid email, a country code with a mobile number).
pub(crate) fn member_form(role: &str, m: &NewMember) -> Result<Vec<(&'static str, String)>, Error> {
    let (project, _) = place(role)?;
    let clean = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let first = clean(&m.first_name);
    let last = clean(&m.last_name);
    let email = m.email.trim().to_string();
    let mobile: String = m.mobile.chars().filter(|c| c.is_ascii_digit()).collect();
    let code: String = m.phone_code.chars().filter(|c| c.is_ascii_digit()).collect();
    let remark = m.remark.trim().to_string();
    if first.is_empty() {
        return Err(protocol("Please enter the first name."));
    }
    if project == "community" && last.is_empty() {
        return Err(protocol("Please enter the last name."));
    }
    if !email.is_empty() && !looks_like_email(&email) {
        return Err(protocol("That does not look like an email address."));
    }
    if email.is_empty() && mobile.is_empty() {
        return Err(protocol("Give an email address or a mobile number, so OneTouch can send them their login."));
    }
    if !m.mobile.trim().is_empty() && (mobile.len() < 6 || mobile.len() > 15) {
        return Err(protocol("That does not look like a mobile number."));
    }
    if !mobile.is_empty() && code.is_empty() {
        return Err(protocol("Choose the country or region of the mobile number."));
    }
    if remark.chars().count() > 255 {
        return Err(protocol("The remark can be at most 255 characters."));
    }
    let phone_code = if mobile.is_empty() { String::new() } else { code };
    Ok(if project == "single" {
        vec![
            ("Name", format!("{first} {last}").trim().to_string()),
            ("Email", email),
            ("PhoneCode", phone_code),
            ("Phone", String::new()),
            ("MobileNumber", mobile),
        ]
    } else {
        vec![
            ("FirstName", first),
            ("LastName", last),
            ("Email", email),
            ("PhoneCode", phone_code),
            ("Phone", String::new()),
            ("MobileNumber", mobile),
            ("Remark", remark),
        ]
    })
}

/// The vendor page's own test, loosely: something@domain.tld.
fn looks_like_email(s: &str) -> bool {
    let Some((user, host)) = s.split_once('@') else { return false };
    !user.is_empty()
        && host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && s.chars().all(|c| !c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_both_door_log_shapes() {
        let v4 = json!({"code": 0, "data": {"list": [
            {"id": "a1", "captureTime": "2026-10-01 09:00:00", "deviceName": "Front gate",
             "captureType": "Call", "response": "success",
             "picUrl": "https://media.example.com/p/1.jpg", "smallPicUrl": "https://media.example.com/p/1s.jpg"}
        ]}});
        let p = parse_page(&v4, Log::Door).unwrap();
        assert_eq!(p.entries.len(), 1);
        assert!(!p.more);
        let e = &p.entries[0];
        assert_eq!((e.device.as_str(), e.detail.as_str()), ("Front gate", "Rang"));
        assert_eq!(e.thumbnail, "https://media.example.com/p/1s.jpg");
        let v3 = json!({"code": 0, "data": [
            {"ID": "7", "CaptureTime": "2026-10-01 09:00:00", "Location": "Lobby", "CaptureType": "Call",
             "Response": "0", "PicUrl": "https://media.example.com/p/2.jpg", "SPicUrl": "",
             "VideoRecordUrl": "javascript:alert(1)"}
        ]});
        let e = &parse_page(&v3, Log::Door).unwrap().entries[0];
        assert_eq!(e.device, "Lobby");
        assert_eq!(e.thumbnail, "https://media.example.com/p/2.jpg");
        assert!(e.video.is_empty(), "only http(s) media is kept");
    }

    #[test]
    fn reads_both_call_log_shapes_and_pages() {
        let v4 = json!({"code": 0, "data": {"list": [
            {"id": 1, "startTime": "t1", "isCallee": 1, "isAnswer": 0, "callerName": "Front gate", "isRead": 0},
            {"id": 2, "startTime": "t2", "isCallee": 0, "calleeName": "Lobby", "isRead": 1, "isGroupCall": 1}
        ]}});
        let p = parse_page(&v4, Log::Calls).unwrap();
        assert_eq!(p.entries[0].direction, "missed");
        assert!(p.entries[0].unread);
        assert_eq!(p.entries[1].direction, "outgoing");
        assert_eq!(p.entries[1].device, "Lobby");
        assert!(p.entries[1].detail.contains("everyone"));
        let rows: Vec<Value> = (0..21)
            .map(|i| json!({"ID": i, "Time": "t", "Type": 2, "Status": "1", "DevLocation": "Lobby"}))
            .collect();
        let p = parse_page(&json!({"code": 0, "data": rows}), Log::Calls).unwrap();
        assert_eq!(p.entries.len(), 20);
        assert!(p.more);
        assert_eq!(p.entries[0].direction, "answered");
        assert!(!p.entries[0].unread);
    }

    #[test]
    fn expiry_and_people() {
        assert!(matches!(
            parse_page(&json!({"code": 1000000001}), Log::Door),
            Err(Error::Expired)
        ));
        let people = parse_people(&json!({"code": 0, "data": [
            {"ID": "u2", "FirstName": "Asha", "LastName": "Example", "SipAccount": "1100"},
            {"ID": "u3", "Email": "member@example.test"}
        ]}))
        .unwrap();
        assert_eq!(people[0].name, "Asha Example");
        assert_eq!(people[0].sip, "1100");
        assert_eq!(people[1].name, "member@example.test");
        assert_eq!(people[1].email, "member@example.test");
        assert!(people[1].sip.is_empty());
    }

    #[test]
    fn invitation_answers() {
        let ok = parse_invited(&json!({"code": 0, "msg": "Success", "TipCode": 0})).unwrap();
        assert!(ok.emailed);
        assert_eq!(ok.message, "Success");
        assert!(!parse_invited(&json!({"code": 0, "TipCode": 1})).unwrap().emailed);
        let refused = parse_invited(&json!({"code": 400, "msg": "Email exists"})).unwrap_err();
        assert!(refused.to_string().contains("Email exists"));
        assert!(matches!(parse_invited(&json!({"code": 2})), Err(Error::Expired)));
        assert!(accepted(&json!({"code": 0}), "remove the member").is_ok());
        assert!(looks_like_email("asha@example.test"));
        let m = |f: &str, l: &str, e: &str, c: &str, n: &str, r: &str| NewMember { first_name: f.into(), last_name: l.into(), email: e.into(), phone_code: c.into(), mobile: n.into(), remark: r.into() };
        let form = member_form("20", &m(" Asha ", "Example", "asha@example.test", "+91", "00000 00000", "Sister")).unwrap();
        assert_eq!(form, vec![("FirstName", "Asha".into()), ("LastName", "Example".into()), ("Email", "asha@example.test".into()), ("PhoneCode", "91".into()), ("Phone", String::new()), ("MobileNumber", "0000000000".into()), ("Remark", "Sister".into())]);
        let single = member_form("10", &m("Asha", "Example", "asha@example.test", "", "", "")).unwrap();
        assert_eq!(single[0], ("Name", "Asha Example".to_string()));
        assert!(member_form("20", &m("Asha", "", "asha@example.test", "", "", "")).is_err()); // community needs a last name
        assert!(member_form("10", &m("Asha", "", "", "", "", "")).is_err()); // email or mobile
        assert!(member_form("10", &m("Asha", "", "", "", "0000000000", "")).is_err()); // mobile needs a country
        assert!(member_form("10", &m("Asha", "", "", "91", "0000000000", "")).is_ok()); // mobile only is fine
        assert!(member_form("20", &m("Asha", "Example", "a@example.test", "", "", &"x".repeat(256))).is_err());
        assert!(!looks_like_email("asha@example"));
        assert!(!looks_like_email("asha example@example.test"));
    }
}
