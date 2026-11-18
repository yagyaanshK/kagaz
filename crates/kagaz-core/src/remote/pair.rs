//! Pairing with a computer that runs Kagaz, from this side: its address,
//! the code it shows, then three words for the person to compare, and on
//! their yes the computer saved with what it takes to sign in (see
//! link.rs for how).

use super::link;
use super::store::{keep_link_secret, Device, Devices, Identity, PathTo, StoreError};
use super::{Speaks, Way};
use serde_json::json;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("nothing answered at that address and port")]
    NoAnswer,
    #[error("it shows no pairing code now: on it, choose Pair a device first")]
    NotPairing,
    #[error("that was not its code; it shows a new one when asked again")]
    WrongCode,
    #[error("it is not paired: someone said no, or took too long")]
    Declined,
    #[error("it pairs another way (a newer or older Kagaz)")]
    Version,
    #[error("the pairing broke off: {0}")]
    Broke(String),
    #[error("{0}")]
    Store(#[from] StoreError),
}

impl From<std::io::Error> for PairError {
    fn from(e: std::io::Error) -> PairError {
        PairError::Broke(e.to_string())
    }
}

/// A pairing under way: the words to compare are known.
pub struct Started {
    stream: TcpStream,
    key: Vec<u8>,
    said: Vec<Vec<u8>>,
    pub words: String,
    /// What the computer calls itself, its certificate and port.
    pub name: String,
    pub fingerprint: String,
    pub port: u16,
    host: String,
    system: String,
}

/// Reaches the computer at `host` (and `port`), gives it the code, and
/// comes back with the words to compare.
pub fn begin(host: &str, port: u16, code: &str, my_name: &str) -> Result<Started, PairError> {
    let addr: SocketAddr = (host.trim(), port).to_socket_addrs().map_err(|_| PairError::NoAnswer)?.next().ok_or(PairError::NoAnswer)?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(8)).map_err(|_| PairError::NoAnswer)?;
    s.set_read_timeout(Some(Duration::from_secs(20)))?;
    let half = link::start(code);
    let mine = half.message.clone();
    use std::io::Write;
    s.write_all(link::MAGIC)?;
    link::write_frame(&mut s, &json!({ "v": link::VERSION, "name": my_name, "spake": link::hex(&mine) }))?;
    let answer = link::read_frame(&mut s)?;
    match answer["error"].as_str() {
        Some("not pairing") => return Err(PairError::NotPairing),
        Some("version") => return Err(PairError::Version),
        Some(other) => return Err(PairError::Broke(other.to_string())),
        None => {}
    }
    let theirs = answer["spake"].as_str().and_then(link::unhex).ok_or_else(|| PairError::Broke("no key exchange".into()))?;
    let name: String = answer["name"].as_str().unwrap_or("Computer").chars().filter(|c| !c.is_control()).take(64).collect();
    let fingerprint = answer["fingerprint"].as_str().unwrap_or("").to_string();
    let port = answer["port"].as_u64().map(|p| p as u16).unwrap_or(port);
    let system = match answer["system"].as_str() {
        Some(s @ ("windows" | "linux" | "macos")) => s.to_string(),
        _ => String::new(),
    };
    let key = link::finish(half, &theirs).ok_or_else(|| PairError::Broke("the key exchange failed".into()))?;
    // The order the computer uses: its side's message second.
    let said: Vec<Vec<u8>> = vec![mine.clone(), theirs.clone(), fingerprint.clone().into_bytes(), my_name.as_bytes().to_vec(), name.clone().into_bytes()];
    let refs: Vec<&[u8]> = said.iter().map(|v| v.as_slice()).collect();
    link::write_frame(&mut s, &json!({ "proof": link::proof(&key, "viewer", &refs) }))?;
    let back = link::read_frame(&mut s)?;
    if back["error"].as_str() == Some("code") {
        return Err(PairError::WrongCode);
    }
    if !link::same(back["proof"].as_str().unwrap_or(""), &link::proof(&key, "host", &refs)) {
        return Err(PairError::WrongCode);
    }
    Ok(Started { words: link::words(&key), stream: s, key, said, name, fingerprint, port, host: host.trim().to_string(), system })
}

impl Started {
    /// The person here said the words match (`yes`) or not. On yes, and
    /// once the person there says so too, the computer is saved in
    /// `list`; its id here is returned.
    pub fn finish(mut self, yes: bool, list: &Path) -> Result<String, PairError> {
        let refs: Vec<&[u8]> = self.said.iter().map(|v| v.as_slice()).collect();
        link::write_frame(&mut self.stream, &json!({ "ok": yes, "proof": link::proof(&self.key, "viewer ok", &refs) }))?;
        if !yes {
            return Err(PairError::Declined);
        }
        // The person there has two minutes to say so.
        self.stream.set_read_timeout(Some(Duration::from_secs(130)))?;
        let last = link::read_frame(&mut self.stream)?;
        let id = last["id"].as_str().unwrap_or("").to_string();
        if last["ok"].as_bool() != Some(true)
            || id.is_empty()
            || !link::same(last["proof"].as_str().unwrap_or(""), &link::proof(&self.key, "host ok", &[id.as_bytes()]))
        {
            return Err(PairError::Declined);
        }
        let device = Device {
            name: self.name.clone(),
            system: self.system.clone(),
            ways: vec![Way { speaks: Speaks::Rdp, port: self.port }],
            paths: vec![PathTo::new(&self.host)],
            identity: Identity { name: None, certificate: Some(self.fingerprint.clone()) },
            user: Some(id),
            paired: true,
            ..Device::default()
        };
        let mut new_id = String::new();
        Devices::update(list, |d| {
            new_id = d.add(device.clone())?;
            Ok(())
        })?;
        keep_link_secret(list, &new_id, &link::device_secret(&self.key))?;
        Ok(new_id)
    }
}
