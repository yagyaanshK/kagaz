//! The separately selected OTIPVDP protocol. Never an automatic fallback
//! for Smart Cloud credentials. Legacy discovery/gateway use HTTP as in the APK.
use super::*;
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use base64::Engine as _;

pub(super) fn service(value: &str, scheme: &str) -> Result<String, Error> {
    let value = if value.contains("://") {
        value.to_owned()
    } else {
        format!("{scheme}://{value}")
    };
    let u =
        Url::parse(&value).map_err(|_| protocol("OTIPVDP returned an invalid server address."))?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err(protocol("OTIPVDP returned an invalid server address."));
    }
    Ok(u.as_str().trim_end_matches('/').into())
}
fn login_data(bytes: &[u8], username: &str) -> Result<Value, Error> {
    if let Ok(value) = serde_json::from_slice(bytes) {
        return legacy_data(value, true);
    }
    // API 4.6+ uses AES-256-CBC with a key derived from the username MD5.
    let encoded: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| protocol("OTIPVDP returned an unreadable login response."))?;
    let key = format!("{}Akuvox55069013!@", &md5_hex(username)[..16]);
    let plaintext =
        cbc::Decryptor::<aes::Aes256>::new_from_slices(key.as_bytes(), b"1234567887654321")
            .map_err(|_| protocol("Could not read OTIPVDP's encrypted login response."))?
            .decrypt_padded_vec_mut::<Pkcs7>(&ciphertext)
            .map_err(|_| protocol("Could not read OTIPVDP's encrypted login response."))?;
    let value = serde_json::from_slice(&plaintext)
        .map_err(|_| protocol("OTIPVDP returned an unreadable login response."))?;
    legacy_data(value, true)
}
fn legacy_data(v: Value, login: bool) -> Result<Value, Error> {
    match field(&v, "result").as_str() {
        "0" => v
            .get("datas")
            .cloned()
            .ok_or_else(|| protocol("OTIPVDP's response is missing account data.")),
        "1" | "2" if login => Err(Error::Credentials),
        "3" if login => Err(protocol("Your OTIPVDP subscription has expired.")),
        "4" | "5" if login => Err(protocol(
            "Your OTIPVDP account is disabled. Check it in the OTIPVDP app.",
        )),
        "1" | "2" => Err(Error::Expired),
        _ => Err(protocol(
            "OTIPVDP rejected the request. Check your account and server in the OTIPVDP app.",
        )),
    }
}
pub(super) fn login(
    username: &str,
    password: &str,
    device: &str,
) -> Result<(Session, Account), Error> {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return Err(protocol("Enter your OTIPVDP username and password."));
    }
    let mac = device.trim().replace([':', '-'], "").to_ascii_uppercase();
    let mac = if mac.is_empty() { "0C1105056C12" } else { &mac };
    if mac.len() != 12 || !mac.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(protocol(
            "Enter the 12-digit device MAC from OTIPVDP's advanced settings.",
        ));
    }
    let a = agent();
    let response = a
        .get("http://rps.akuvox.com:8080/redirectv6")
        .query("mac", mac)
        .set("api-version", "3.0")
        .call()
        .map_err(transport_error)?;
    let gate = service(
        &required(
            &legacy_data(read_response(response)?, false)?,
            "gate_serverV4",
        )?,
        "http",
    )?;
    let response = a
        .get(&format!("{gate}/login"))
        .query("user", username)
        .query("passwd", &md5_hex(&md5_hex(password)))
        .set("api-version", "4.6")
        .call()
        .map_err(transport_error)?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Network)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(protocol("OTIPVDP's login response was too large."));
    }
    let data = login_data(&bytes, username)?;
    let relay = field(&data, "vrtsp_server");
    let mut session = Session {
        username: username.into(),
        provider: Provider::Otipvdp,
        role_hint: field(&data, "role"),
        gate,
        rest: service(&required(&data, "rest_server_https")?, "https")?,
        web: service(&required(&data, "web_server")?, "http")?,
        relay: if relay.is_empty() {
            required(&data, "access_server")?
        } else {
            relay
        },
        token: required(&data, "token")?,
        refresh_token: String::new(),
    };
    let account = session.account()?;
    Ok((session, account))
}
pub(super) fn account(s: &Session) -> Result<Account, Error> {
    let data = legacy_data(
        s.get(
            &format!("{}/userconf", s.rest),
            "3.0",
            &[("token", s.token.clone())],
        )?,
        false,
    )?;
    let list = data["dev_list"]
        .as_array()
        .ok_or_else(|| protocol("OTIPVDP's device list was missing."))?;
    let mut devices = Vec::new();
    for item in list {
        let id = required(item, "mac")?;
        if id.len() != 12 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(protocol("OTIPVDP returned an invalid device identifier."));
        }
        let password = field(item, "rtsp_pwd");
        devices.push(Device {
            id,
            name: required(item, "location")?,
            kind: field(item, "dev_type"),
            status: field(item, "status"),
            can_monitor: true,
            password: if password == "null" {
                String::new()
            } else {
                password
            },
        });
    }
    Ok(Account {
        devices,
        uid: field(&data["app_conf"], "uid"),
        role: s.role_hint.clone(),
    })
}
fn web_data(v: Value) -> Result<Value, Error> {
    match field(&v, "code").as_str() {
        "0" => Ok(v["data"].clone()),
        "2" => Err(Error::Expired),
        _ => Err(protocol("OTIPVDP could not load this history list.")),
    }
}
fn time(v: &Value, key: &str) -> String {
    match &v[key] {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => field(v, key),
    }
}
fn snapshots(v: Value) -> Result<Vec<Recording>, Error> {
    let data = web_data(v)?;
    let rows = data["row"]
        .as_array()
        .ok_or_else(|| protocol("OTIPVDP's snapshot list was missing."))?;
    rows.iter()
        .map(|r| {
            let picture = media_url(field(r, "PicUrl"))?;
            Ok(Recording {
                id: format!("snapshot:{}", required(r, "ID")?),
                name: field(r, "Location"),
                at: time(r, "Time"),
                has_video: false,
                has_picture: !picture.is_empty(),
                video: String::new(),
                picture,
            })
        })
        .collect()
}
fn videos(v: Value) -> Result<Vec<Recording>, Error> {
    let data = web_data(v)?;
    let rows = data
        .as_array()
        .ok_or_else(|| protocol("OTIPVDP's video list was missing."))?;
    rows.iter()
        .map(|r| {
            let video = media_url(field(r, "VideoUri"))?;
            Ok(Recording {
                id: format!("video:{}", required(r, "ID")?),
                name: field(r, "Location"),
                at: time(r, "VideoTime2"),
                has_video: !video.is_empty(),
                has_picture: false,
                video,
                picture: String::new(),
            })
        })
        .collect()
}
pub(super) fn recordings(s: &Session, page: u32) -> Result<Recordings, Error> {
    let mut entries = snapshots(s.get(
        &format!("{}/apache-v3.0/getnewpersonalcaptrue", s.web),
        "3.0",
        &[],
    )?)?;
    let video_result = s
        .get(
            &format!("{}/apache-v3.0/getpersonalvideo", s.web),
            "3.0",
            &[],
        )
        .and_then(videos);
    let mut message = String::new();
    match video_result {
        Ok(videos) => entries.extend(videos),
        Err(Error::Expired) => return Err(Error::Expired),
        Err(_) => {
            message = "Snapshots loaded. OTIPVDP's video list is currently unavailable.".into()
        }
    }
    // The legacy web app fetches complete lists; paginate locally, without guessing API parameters.
    let start = (page.max(1) as usize - 1).saturating_mul(20);
    let more = entries.len() > start.saturating_add(20);
    Ok(Recordings {
        entries: entries.into_iter().skip(start).take(20).collect(),
        more,
        message,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncryptMut;
    #[test]
    fn decrypts_legacy_login_and_keeps_errors_distinct() {
        let plaintext = br#"{"result":0,"datas":{"token":"private-token"}}"#;
        let key = format!("{}Akuvox55069013!@", &md5_hex("alice")[..16]);
        let encrypted =
            cbc::Encryptor::<aes::Aes256>::new_from_slices(key.as_bytes(), b"1234567887654321")
                .unwrap()
                .encrypt_padded_vec_mut::<Pkcs7>(plaintext);
        let encoded = base64::engine::general_purpose::STANDARD.encode(encrypted);
        assert_eq!(
            login_data(encoded.as_bytes(), "alice").unwrap()["token"],
            "private-token"
        );
        assert!(login_data(encoded.as_bytes(), "bob").is_err());
        assert!(matches!(
            login_data(br#"{"result":2}"#, "alice"),
            Err(Error::Credentials)
        ));
        assert!(matches!(web_data(json!({"code":2})), Err(Error::Expired)));
    }
    #[test]
    fn old_history_shapes_and_plain_login_are_supported() {
        assert_eq!(
            login_data(br#"{"result":"0","datas":{"token":"t"}}"#, "alice").unwrap()["token"],
            "t"
        );
        let pics = snapshots(json!({"code":0,"data":{"row":[{"ID":1,"Time":["2026-10-13","12:00"],"PicUrl":"https://example.test/p.jpg"}]}})).unwrap();
        assert_eq!(pics[0].at, "2026-10-13 12:00");
        assert!(!pics[0].has_video);
        assert!(pics[0].has_picture);
        let clips = videos(json!({"code":0,"data":[{"ID":1,"VideoUri":"https://example.test/c.mp4","VideoTime2":"12:00"}]})).unwrap();
        assert!(clips[0].has_video);
        assert_ne!(clips[0].id, pics[0].id);
    }
}
