//! Calls through the OneTouch cloud: the account's own phone-app line and
//! the devices it can call. `/userconf` carries both: `app_conf.sip`,
//! `sip_passwd`, `sip_server` and `trans_type` are the line the OneTouch
//! app registers when its push arrives; each `dev_list` entry has the `sip`
//! number to dial and the `dtmf` that opens its door. With them Kagaz is
//! one more endpoint of the account, so the cloud rings it directly.

use super::{api_data, protocol, Error, Provider, Session};
use crate::intercom::SipAccount;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One device the account can call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallDevice {
    pub name: String,
    /// The number to dial through the cloud.
    pub sip: String,
    /// What it is: "door", "indoor", "guard", or "other".
    pub kind: String,
    /// DTMF that opens its door during a call; empty when it has no lock.
    pub door_dtmf: String,
    pub online: bool,
}

/// The account's cloud line and its devices. Not serializable on purpose:
/// the line's password must never reach the page or a log.
#[derive(Clone)]
pub struct CallSetup {
    pub line: SipAccount,
    /// Whether the cloud scrambles RTP ("RTP confuse"); Kagaz speaks only
    /// plain RTP.
    pub rtp_scrambled: bool,
    pub devices: Vec<CallDevice>,
}

impl Session {
    /// The account's cloud call line and the devices it can call. This
    /// never renews the session: the window keeps its own copy of it, and
    /// a renewal here would leave that copy stale. An expired session is
    /// reported as [`Error::Expired`]; reconnecting OneTouch fixes it.
    pub fn call_setup(&self) -> Result<CallSetup, Error> {
        if self.provider != Provider::SmartCloud {
            return Err(protocol(
                "Calls are available for OneTouch Smart Cloud accounts.",
            ));
        }
        let data = self
            .get(
                &format!("{}/userconf", self.rest),
                "7.13",
                &[("token", self.token.clone())],
            )
            .and_then(api_data)?;
        parse_call_setup(&data)
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Akuvox device types: 0 unit/stair door phone, 1 villa door phone,
/// 2 indoor monitor, 3 guard/management phone.
fn kind(dev_type: &str) -> &'static str {
    match dev_type {
        "0" | "1" => "door",
        "2" => "indoor",
        "3" => "guard",
        _ => "other",
    }
}

pub(crate) fn parse_call_setup(data: &Value) -> Result<CallSetup, Error> {
    let app = &data["app_conf"];
    let sip = text(&app["sip"]);
    let password = text(&app["sip_passwd"]);
    let server = text(&app["sip_server"]);
    if sip.is_empty() || password.is_empty() || server.is_empty() {
        return Err(protocol(
            "This OneTouch account has no app call line, so Kagaz cannot take its calls.",
        ));
    }
    let (host, port) = match server.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()),
        _ => (server.clone(), 5060),
    };
    // trans_type: 0 UDP, 1 TCP, 2 TLS. TLS is not spoken here; TCP is what
    // the accounts seen so far use.
    let transport = match text(&app["trans_type"]).as_str() {
        "0" => "udp",
        "2" => {
            return Err(protocol(
                "This OneTouch account calls over TLS, which Kagaz does not speak yet.",
            ))
        }
        _ => "tcp",
    };
    let display = text(&app["display_name"]);
    let devices = data["dev_list"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|d| text(&d["is_need_call"]) != "0")
                .filter_map(|d| {
                    let sip = text(&d["sip"]);
                    (!sip.is_empty()).then(|| CallDevice {
                        name: {
                            let n = text(&d["location"]);
                            if n.is_empty() {
                                sip.clone()
                            } else {
                                n
                            }
                        },
                        kind: kind(&text(&d["dev_type"])).to_string(),
                        door_dtmf: text(&d["dtmf"]),
                        online: text(&d["status"]) == "1",
                        sip,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(CallSetup {
        line: SipAccount {
            username: sip,
            password,
            display_name: if display.is_empty() {
                "Kagaz".into()
            } else {
                display
            },
            server: host,
            port,
            transport: transport.into(),
            expires: 300,
        },
        rtp_scrambled: text(&app["rtp_confuse"]) == "1",
        devices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_line_and_the_devices() {
        let s = parse_call_setup(&json!({
            "app_conf": {"sip": "20011001", "sip_passwd": "secret", "sip_server": "203.0.113.10:5070",
                          "trans_type": "1", "display_name": "Flat", "rtp_confuse": 0},
            "dev_list": [
                {"location": "Lobby", "sip": "20011002", "dev_type": "0", "dtmf": "#", "status": "1", "is_need_call": 1},
                {"location": "Upstairs", "sip": "20011003", "dev_type": "2", "dtmf": "#", "status": "0", "is_need_call": 1},
                {"location": "Hidden", "sip": "20011004", "dev_type": "1", "dtmf": "#", "status": "1", "is_need_call": 0}
            ]
        }))
        .unwrap();
        assert_eq!(s.line.username, "20011001");
        assert_eq!(
            (s.line.server.as_str(), s.line.port),
            ("203.0.113.10", 5070)
        );
        assert!(s.line.over_tcp());
        assert!(!s.rtp_scrambled);
        assert_eq!(s.devices.len(), 2);
        assert_eq!(s.devices[0].kind, "door");
        assert!(s.devices[0].online);
        assert_eq!(s.devices[1].kind, "indoor");
        assert!(!s.devices[1].online);
    }

    #[test]
    fn an_account_without_a_line_says_so() {
        assert!(parse_call_setup(&json!({"app_conf": {"sip": ""}, "dev_list": []})).is_err());
        assert!(
            parse_call_setup(&json!({"app_conf": {"sip": "1", "sip_passwd": "x",
            "sip_server": "203.0.113.10:5071", "trans_type": "2"}}))
            .is_err()
        );
    }
}
