//! Pairing, on this computer: a code shown here (good once, for five
//! minutes), the device that types it, three words both show, and the
//! person here confirming and choosing what the device may do. See
//! remote/link.rs for why the code cannot be learnt by listening.

use crate::link;
use crate::paired::{May, Paired};
use crate::{event, say};
use serde_json::json;
use std::net::TcpStream;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Pairing {
    pub paired: Arc<Paired>,
    pub name: String,
    pub fingerprint: String,
    pub port: u16,
    code: Mutex<Option<(String, Instant)>>,
    decision: Mutex<Option<Sender<Option<May>>>>,
}

impl Pairing {
    pub fn new(paired: Arc<Paired>, name: String, fingerprint: String, port: u16) -> Pairing {
        Pairing { paired, name, fingerprint, port, code: Mutex::new(None), decision: Mutex::new(None) }
    }

    /// A new code, replacing any before it.
    pub fn start(&self) -> String {
        let mut b = [0u8; 4];
        getrandom::fill(&mut b).expect("the system gives random bytes");
        let n = u32::from_le_bytes(b) % 1_000_000;
        let code = format!("{:03} {:03}", n / 1000, n % 1000);
        *self.code.lock().unwrap() = Some((code.clone(), Instant::now() + Duration::from_secs(link::CODE_LASTS)));
        event(json!({ "kind": "code", "code": code, "seconds": link::CODE_LASTS }));
        say(&format!(
            "pairing code {code}: on the other device, Add a device › Computers & phones › Pair with a code (this computer's address, port {}); good once, for 5 minutes",
            self.port
        ));
        code
    }

    pub fn stop(&self) {
        *self.code.lock().unwrap() = None;
    }

    /// The person here confirmed the words (with what the device may do),
    /// or did not.
    pub fn decide(&self, may: Option<May>) {
        match self.decision.lock().unwrap().take() {
            Some(tx) => {
                let _ = tx.send(may);
            }
            None => say("no pairing is waiting for that"),
        }
    }

    /// A device that started a pairing (its first four bytes, the mark,
    /// already read).
    pub fn handle(&self, mut s: TcpStream) {
        if let Err(e) = self.run(&mut s) {
            say(&format!("the pairing stopped: {e}"));
        }
        *self.decision.lock().unwrap() = None;
    }

    fn run(&self, s: &mut TcpStream) -> anyhow::Result<()> {
        s.set_read_timeout(Some(Duration::from_secs(150)))?;
        let hello = link::read_frame(s)?;
        if hello["v"].as_u64() != Some(link::VERSION) {
            link::write_frame(s, &json!({ "error": "version" }))?;
            anyhow::bail!("another kind of pairing");
        }
        let viewer: String = hello["name"].as_str().unwrap_or("A device").chars().filter(|c| !c.is_control()).take(64).collect();
        let theirs = hello["spake"].as_str().and_then(link::unhex).ok_or_else(|| anyhow::anyhow!("no key exchange"))?;
        // One try for each code: a wrong guess ends it.
        let code = match self.code.lock().unwrap().take() {
            Some((code, until)) if Instant::now() < until => code,
            _ => {
                link::write_frame(s, &json!({ "error": "not pairing" }))?;
                anyhow::bail!("{viewer} tried to pair, but no code was showing");
            }
        };
        let half = link::start(&code);
        let mine = half.message.clone();
        link::write_frame(s, &json!({ "v": link::VERSION, "name": self.name, "spake": link::hex(&mine), "fingerprint": self.fingerprint, "port": self.port, "system": std::env::consts::OS }))?;
        let key = link::finish(half, &theirs).ok_or_else(|| anyhow::anyhow!("the key exchange failed"))?;
        let said: [&[u8]; 5] = [&theirs, &mine, self.fingerprint.as_bytes(), viewer.as_bytes(), self.name.as_bytes()];
        let proof = link::read_frame(s)?;
        if !link::same(proof["proof"].as_str().unwrap_or(""), &link::proof(&key, "viewer", &said)) {
            link::write_frame(s, &json!({ "error": "code" }))?;
            event(json!({ "kind": "not-paired", "name": viewer, "why": "code" }));
            anyhow::bail!("{viewer} typed another code; type pair for a new one");
        }
        link::write_frame(s, &json!({ "proof": link::proof(&key, "host", &said) }))?;
        event(json!({ "kind": "words", "words": link::words(&key), "name": viewer }));
        say(&format!(
            "words {} from {viewer:?}: if the other device shows the same, type confirm (or confirm with what it may do: control clipboard files sound unattended), else cancel",
            link::words(&key)
        ));

        // Both people decide, in either order.
        let (tx, mine_rx) = channel();
        *self.decision.lock().unwrap() = Some(tx);
        let (vtx, theirs_rx) = channel();
        let mut reader = s.try_clone()?;
        std::thread::spawn(move || {
            let _ = vtx.send(link::read_frame(&mut reader).ok());
        });
        let until = Instant::now() + Duration::from_secs(120);
        let (mut here, mut there): (Option<Option<May>>, Option<bool>) = (None, None);
        while (here.is_none() || there.is_none()) && Instant::now() < until {
            if here.is_none() {
                if let Ok(d) = mine_rx.recv_timeout(Duration::from_millis(50)) {
                    here = Some(d);
                }
            }
            if there.is_none() {
                if let Ok(frame) = theirs_rx.recv_timeout(Duration::from_millis(50)) {
                    let ok = frame.is_some_and(|f| {
                        f["ok"].as_bool() == Some(true) && link::same(f["proof"].as_str().unwrap_or(""), &link::proof(&key, "viewer ok", &said))
                    });
                    there = Some(ok);
                }
            }
            if here == Some(None) || there == Some(false) {
                break;
            }
        }
        match (here, there) {
            (Some(Some(may)), Some(true)) => {
                let id = self.paired.add(&viewer, &link::device_secret(&key), may);
                link::write_frame(s, &json!({ "ok": true, "id": id, "proof": link::proof(&key, "host ok", &[id.as_bytes()]) }))?;
                say(&format!("paired {viewer:?} as {id}: {}", may.words()));
                event(json!({ "kind": "paired", "id": id, "name": viewer }));
            }
            _ => {
                let _ = link::write_frame(s, &json!({ "ok": false }));
                say(&format!("not paired with {viewer:?}"));
                event(json!({ "kind": "not-paired", "name": viewer }));
            }
        }
        Ok(())
    }
}
