//! Who may connect: the devices paired with this computer, what each may
//! do, this computer's PIN, and asking the person here.
//!
//! A paired device signs in with its id here and its secret (made at
//! pairing, kept here only as a hash), and the PIN when it was typed. With
//! the right PIN, a device allowed to connect without asking comes
//! straight in; otherwise the person here is asked, and may let it use
//! the mouse and keys or only watch. No answer in 30 seconds is no. Five
//! wrong PINs in a row stop PINs for five minutes.

use crate::link;
use crate::{event, say};
use serde_json::json;
use ironrdp_server::{CredentialDecision, CredentialValidationError, CredentialValidator, Credentials};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a paired device may do here.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct May {
    /// Use the mouse and keys (otherwise it watches).
    pub control: bool,
    pub clipboard: bool,
    pub files: bool,
    pub sound: bool,
    /// Connect with the PIN, without asking the person here.
    pub unattended: bool,
}

impl Default for May {
    fn default() -> May {
        May { control: true, clipboard: true, files: true, sound: true, unattended: false }
    }
}

impl May {
    /// From words such as `control files sound`; none means the usual.
    pub fn from_words(words: &[&str]) -> May {
        if words.is_empty() {
            return May::default();
        }
        let has = |w: &str| words.contains(&w);
        May {
            control: has("control"),
            clipboard: has("clipboard"),
            files: has("files"),
            sound: has("sound"),
            unattended: has("unattended"),
        }
    }

    pub fn words(&self) -> String {
        let mut w = vec![if self.control { "control" } else { "watch" }];
        for (on, name) in [(self.clipboard, "clipboard"), (self.files, "files"), (self.sound, "sound"), (self.unattended, "unattended")] {
            if on {
                w.push(name);
            }
        }
        w.join(" ")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// The hash of its secret.
    pub secret: String,
    pub may: May,
    pub paired: u64,
    #[serde(default)]
    pub seen: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    /// This computer's PIN: a salt and the PIN's slow hash.
    #[serde(default)]
    pub pin: Option<(String, String)>,
    #[serde(default)]
    pub devices: Vec<Device>,
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("the system gives random bytes");
    link::hex(&b)
}

/// A PIN's hash, slow on purpose: a stolen list does not give the PIN
/// away by trying a million of them in a moment.
fn pin_hash(salt: &str, pin: &str) -> String {
    let mut h = link::hash(&format!("{salt}:{}", link::digits(pin)));
    for _ in 0..200_000 {
        h = link::hash(&h);
    }
    h
}

/// The list, kept in a file next to the certificate.
pub struct Paired {
    path: PathBuf,
    pub store: Mutex<Store>,
    wrong: Mutex<(u32, Option<Instant>)>,
}

impl Paired {
    pub fn load(path: PathBuf) -> Paired {
        let store = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Paired { path, store: Mutex::new(store), wrong: Mutex::new((0, None)) }
    }

    pub fn save(&self) {
        let store = self.store.lock().unwrap();
        if let Ok(bytes) = serde_json::to_vec_pretty(&*store) {
            let part = self.path.with_extension("new");
            if std::fs::write(&part, bytes).is_ok() {
                let _ = std::fs::rename(&part, &self.path);
            }
        }
    }

    pub fn set_pin(&self, pin: &str) -> bool {
        let pin = link::digits(pin);
        if pin.len() < 4 {
            return false;
        }
        let salt = random_hex(16);
        self.store.lock().unwrap().pin = Some((salt.clone(), pin_hash(&salt, &pin)));
        self.save();
        true
    }

    fn pin_right(&self, pin: &str) -> Option<bool> {
        let mut wrong = self.wrong.lock().unwrap();
        if let Some(until) = wrong.1 {
            if Instant::now() < until {
                return None;
            }
            *wrong = (0, None);
        }
        let right = match &self.store.lock().unwrap().pin {
            Some((salt, hash)) => link::same(&pin_hash(salt, pin), hash),
            None => false,
        };
        if right {
            wrong.0 = 0;
        } else {
            wrong.0 += 1;
            if wrong.0 >= 5 {
                wrong.1 = Some(Instant::now() + Duration::from_secs(5 * 60));
                say("five wrong PINs: none taken for five minutes");
            }
        }
        Some(right)
    }

    pub fn add(&self, name: &str, secret: &str, may: May) -> String {
        let id = random_hex(8);
        self.store.lock().unwrap().devices.push(Device {
            id: id.clone(),
            name: name.to_string(),
            secret: link::hash(secret),
            may,
            paired: now(),
            seen: None,
        });
        self.save();
        id
    }

    pub fn forget(&self, id: &str) -> bool {
        let gone = {
            let mut store = self.store.lock().unwrap();
            let before = store.devices.len();
            store.devices.retain(|d| d.id != id);
            store.devices.len() != before
        };
        if gone {
            self.save();
        }
        gone
    }

    pub fn change(&self, id: &str, may: May) -> bool {
        let found = {
            let mut store = self.store.lock().unwrap();
            store.devices.iter_mut().find(|d| d.id == id).map(|d| d.may = may).is_some()
        };
        if found {
            self.save();
        }
        found
    }
}

/// The person here's answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    Control,
    Watch,
    Deny,
}

/// How the person here is asked.
pub trait Asker: Send + Sync {
    /// Asks, and waits at most `wait` for the answer.
    fn ask(&self, id: &str, name: &str, wait: Duration) -> Answer;
}

/// What the session going on now may do; one session at a time.
#[derive(Clone, Default)]
pub struct Allowed {
    pub control: Arc<AtomicBool>,
    pub clipboard: Arc<AtomicBool>,
    pub files: Arc<AtomicBool>,
    pub sound: Arc<AtomicBool>,
}

impl Allowed {
    pub fn set(&self, may: May) {
        self.control.store(may.control, Ordering::Release);
        self.clipboard.store(may.clipboard, Ordering::Release);
        self.files.store(may.files, Ordering::Release);
        self.sound.store(may.sound, Ordering::Release);
    }
}

/// The device connected now, for the sign here, a change from watching
/// to control, and the record of sessions.
#[derive(Clone, Debug)]
pub struct Current {
    pub id: String,
    pub name: String,
    /// What it may do at most (its permissions); `allowed` says what it
    /// does now.
    pub may: May,
    pub began: Instant,
    /// How much the person here had used the mouse and keys when it
    /// began, to tell afterwards whether anyone was here.
    pub local_input: u64,
}

/// Decides each sign-in.
pub struct Gate {
    pub paired: Arc<Paired>,
    pub asker: Arc<dyn Asker>,
    pub allowed: Allowed,
    /// Watching only, whatever the device may do (`--watch`).
    pub watch_only: bool,
    pub current: Arc<Mutex<Option<Current>>>,
}

#[async_trait::async_trait]
impl CredentialValidator for Gate {
    async fn validate(&self, credentials: &Credentials) -> Result<CredentialDecision, CredentialValidationError> {
        let device = {
            let store = self.paired.store.lock().unwrap();
            store.devices.iter().find(|d| d.id == credentials.username).cloned()
        };
        let Some(device) = device else {
            say("a device that is not paired was turned away");
            return Ok(CredentialDecision::Reject);
        };
        let (secret, pin) = link::split_password(&credentials.password);
        if !link::same(&link::hash(secret), &device.secret) {
            say(&format!("{} was turned away: not its secret", device.name));
            return Ok(CredentialDecision::Reject);
        }
        let paired = self.paired.clone();
        let pin = pin.to_string();
        let pin_ok = if pin.is_empty() {
            false
        } else {
            match tokio::task::spawn_blocking(move || paired.pin_right(&pin)).await.ok().flatten() {
                Some(true) => true,
                Some(false) => {
                    say(&format!("{} was turned away: a wrong PIN", device.name));
                    return Ok(CredentialDecision::Reject);
                }
                None => {
                    say(&format!("{} was turned away: PINs are stopped for a while", device.name));
                    return Ok(CredentialDecision::Reject);
                }
            }
        };
        let mut may = device.may;
        if !(pin_ok && device.may.unattended) {
            let (asker, id, name) = (self.asker.clone(), device.id.clone(), device.name.clone());
            let answer = tokio::task::spawn_blocking(move || asker.ask(&id, &name, Duration::from_secs(30)))
                .await
                .unwrap_or(Answer::Deny);
            match answer {
                Answer::Deny => {
                    say(&format!("{} was not allowed", device.name));
                    event(json!({ "kind": "asked", "id": device.id }));
                    return Ok(CredentialDecision::Reject);
                }
                Answer::Watch => may.control = false,
                Answer::Control => {}
            }
            event(json!({ "kind": "asked", "id": device.id }));
        }
        if self.watch_only {
            may.control = false;
        }
        // Watching takes nothing from the viewer.
        if !may.control {
            may.files = false;
        }
        self.allowed.set(may);
        {
            let mut store = self.paired.store.lock().unwrap();
            if let Some(d) = store.devices.iter_mut().find(|d| d.id == device.id) {
                d.seen = Some(now());
            }
        }
        self.paired.save();
        say(&format!("{} is connected: {}", device.name, may.words()));
        event(json!({ "kind": "connected", "id": device.id, "name": device.name, "control": may.control, "may": device.may.words() }));
        *self.current.lock().unwrap() = Some(Current {
            id: device.id.clone(),
            name: device.name.clone(),
            may: device.may,
            began: Instant::now(),
            local_input: crate::local_input(),
        });
        Ok(CredentialDecision::Accept)
    }
}

/// Answers given on the input (`answer <id> control|watch|deny`), for
/// the window that runs this program, or a person at a terminal.
#[derive(Default)]
pub struct Lines {
    waiting: Mutex<Vec<(String, std::sync::mpsc::Sender<Answer>)>>,
}

impl Lines {
    pub fn answer(&self, id: &str, answer: Answer) {
        let mut waiting = self.waiting.lock().unwrap();
        if let Some(at) = waiting.iter().position(|(w, _)| w == id) {
            let (_, tx) = waiting.remove(at);
            let _ = tx.send(answer);
        }
    }
}

impl Asker for Lines {
    fn ask(&self, id: &str, name: &str, wait: Duration) -> Answer {
        let (tx, rx) = std::sync::mpsc::channel();
        self.waiting.lock().unwrap().push((id.to_string(), tx));
        say(&format!("ask {id} {name:?}: type answer {id} control, watch or deny"));
        event(json!({ "kind": "ask", "id": id, "name": name, "seconds": wait.as_secs() }));
        let answer = rx.recv_timeout(wait).unwrap_or(Answer::Deny);
        self.waiting.lock().unwrap().retain(|(w, _)| w != id);
        answer
    }
}

/// The same answer always: for the test card's runs.
pub struct Always(pub Answer);

impl Asker for Always {
    fn ask(&self, _id: &str, name: &str, _wait: Duration) -> Answer {
        say(&format!("asked about {name}: {:?}", self.0));
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_is_kept_as_a_hash_and_checked() {
        let dir = std::env::temp_dir().join(format!("kagaz-share-test-{}", random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = Paired::load(dir.join("paired.json"));
        assert!(!p.set_pin("12"));
        assert!(p.set_pin("4829"));
        assert_eq!(p.pin_right("4829"), Some(true));
        assert_eq!(p.pin_right("1111"), Some(false));
        let kept = std::fs::read_to_string(dir.join("paired.json")).unwrap();
        assert!(!kept.contains("4829"));
        for _ in 0..4 {
            p.pin_right("0000");
        }
        assert_eq!(p.pin_right("4829"), None, "stopped after five wrong");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn permissions_read_from_words() {
        assert_eq!(May::from_words(&[]), May::default());
        let m = May::from_words(&["sound", "unattended"]);
        assert!(!m.control && m.sound && m.unattended && !m.files);
        assert_eq!(m.words(), "watch sound unattended");
    }
}
