//! The video doorbell (intercom): talk to an Akuvox-platform SIP door
//! station the way the official app does, but on our terms. The door
//! station and an indoor unit register to a SIP server; pressing
//! the bell places a SIP call that every registered endpoint receives.
//! Kagaz registers as one more endpoint of the same account, so it rings
//! too — on the desktop now, and (through a push bridge) on the phone.
//!
//! `sip` is the signalling (REGISTER, and later the incoming call); media
//! (voice over RTP, the door camera's video, opening the door) builds on
//! top of it.

pub mod incoming;
pub mod media;
pub mod sip;
pub mod stations;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A SIP account: who we register as, and where. These are the same values
/// the door station's own web page shows under Account → Basic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SipAccount {
    /// The SIP user (e.g. "20011001"); also the register and auth name here.
    pub username: String,
    pub password: String,
    /// Shown to the other side; the device uses its room name.
    #[serde(default)]
    pub display_name: String,
    /// The registrar / SIP server host (an IP or name).
    pub server: String,
    /// Its port (the device uses 5070, not the usual 5060).
    #[serde(default = "default_sip_port")]
    pub port: u16,
    /// "tcp" (what the device uses here) or "udp".
    #[serde(default = "default_transport")]
    pub transport: String,
    /// Seconds a registration lasts before it must be renewed.
    #[serde(default = "default_expires")]
    pub expires: u32,
}

fn default_sip_port() -> u16 {
    5060
}
fn default_transport() -> String {
    "udp".to_string()
}
fn default_expires() -> u32 {
    1800
}

impl SipAccount {
    /// The account's address of record, `sip:user@server`.
    pub fn aor(&self) -> String {
        format!("sip:{}@{}", self.username, self.server)
    }

    /// Whether to speak SIP over TCP (vs UDP).
    pub fn over_tcp(&self) -> bool {
        self.transport.eq_ignore_ascii_case("tcp")
    }

    /// The saved account, from `<config>/intercom.json`.
    pub fn load() -> Result<SipAccount, Error> {
        Self::load_from(&Self::default_path())
    }

    pub fn load_from(path: &Path) -> Result<SipAccount, Error> {
        let text = std::fs::read_to_string(path).map_err(|_| Error::NotConfigured)?;
        serde_json::from_str(&text).map_err(|e| Error::Config(e.to_string()))
    }

    pub fn save(&self) -> Result<(), Error> {
        let path = Self::default_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::Config(e.to_string()))?;
        }
        let text = serde_json::to_string_pretty(self).expect("json");
        std::fs::write(&path, text).map_err(|e| Error::Config(e.to_string()))
    }

    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("intercom.json")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no intercom account saved yet")]
    NotConfigured,
    #[error("intercom config is invalid: {0}")]
    Config(String),
    #[error("cannot reach the SIP server: {0}")]
    Transport(String),
    #[error("the SIP server rejected the request: {0}")]
    Rejected(String),
    #[error("the SIP server's answer was malformed: {0}")]
    Malformed(String),
}
