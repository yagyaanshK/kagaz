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
use serde::Serialize;
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
        assert!(people[1].sip.is_empty());
    }
}
