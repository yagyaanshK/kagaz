//! Kagaz's window steering a share it did not start: the one the helper
//! service runs (`--system`), which goes on while nobody is signed in and
//! Kagaz is closed. The same lines as on a driven program's input and
//! output go over a connection on this computer only, which needs the
//! token kept in `control.json` (in a folder only the person who
//! installed the helper, the system and administrators can read).
//!
//! One window steers it at a time; a new one takes over. On connecting it
//! is told what is going on (listening, the devices, who is connected), as
//! a window that started the program would have heard it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// The window steering it now.
static OUT: Mutex<Option<TcpStream>> = Mutex::new(None);
/// The latest of each kind of news that tells what is going on.
static NOW: Mutex<Vec<(String, serde_json::Value)>> = Mutex::new(Vec::new());

/// A window is steering it (and can answer a question).
pub fn connected() -> bool {
    OUT.lock().unwrap().is_some()
}

/// A line to the window steering it, if any.
pub fn send(line: &str) {
    let mut out = OUT.lock().unwrap();
    let broke = match out.as_mut() {
        Some(s) => writeln!(s, "{line}").and_then(|_| s.flush()).is_err(),
        None => false,
    };
    if broke {
        *out = None;
    }
}

/// Keeps what a window connecting later must hear.
pub fn remember(value: &serde_json::Value) {
    let kind = value["kind"].as_str().unwrap_or("").to_string();
    let mut now = NOW.lock().unwrap();
    match kind.as_str() {
        "listening" | "off" | "devices" | "connected" | "mode" => {
            if kind == "listening" || kind == "off" {
                now.retain(|(k, _)| k != "listening" && k != "off");
            }
            now.retain(|(k, _)| k != &kind);
            now.push((kind, value.clone()));
        }
        "left" => now.retain(|(k, _)| k != "connected" && k != "mode"),
        _ => {}
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Listens on this computer only, says where in `dir/control.json`, and
/// hands each line a window sends to `line`.
pub fn serve(dir: &Path, line: impl Fn(&str) + Send + Sync + 'static) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    let token = hex(&secret);
    // Written whole under another name first, so that it is never read
    // half written.
    let told = serde_json::json!({ "port": port, "token": token, "pid": std::process::id() });
    let (at, new) = (dir.join("control.json"), dir.join("control.json.new"));
    std::fs::write(&new, told.to_string())?;
    std::fs::rename(&new, &at)?;
    let line = std::sync::Arc::new(line);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let (token, line) = (token.clone(), line.clone());
            std::thread::spawn(move || steer(stream, &token, &*line));
        }
    });
    Ok(())
}

fn steer(stream: TcpStream, token: &str, line: &dyn Fn(&str)) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(reading) = stream.try_clone() else { return };
    let mut reader = BufReader::new(reading);
    // The token first (a line of at most a few hundred bytes), compared
    // in full whatever it says.
    let mut given = String::new();
    if (&mut reader).take(512).read_line(&mut given).unwrap_or(0) == 0 {
        return;
    }
    let (a, b) = (given.trim().as_bytes(), token.as_bytes());
    let same = a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0;
    if !same {
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }
    let _ = stream.set_read_timeout(None);
    let Ok(mut writing) = stream.try_clone() else { return };
    // What is going on, then the news as it comes.
    {
        let mut out = OUT.lock().unwrap();
        if let Some(before) = out.take() {
            let _ = before.shutdown(Shutdown::Both);
        }
        let hello = serde_json::json!({ "kind": "helper", "version": env!("CARGO_PKG_VERSION") });
        let _ = writeln!(writing, "@{hello}");
        for (_, value) in NOW.lock().unwrap().iter() {
            let _ = writeln!(writing, "@{value}");
        }
        let _ = writing.flush();
        *out = Some(writing);
    }
    crate::say("Kagaz's window is steering");
    let me = stream.peer_addr().ok();
    for l in reader.lines() {
        let Ok(l) = l else { break };
        line(&l);
    }
    let mut out = OUT.lock().unwrap();
    if out.as_ref().and_then(|s| s.peer_addr().ok()) == me {
        *out = None;
        crate::say("Kagaz's window let go");
    }
}

/// Kagaz's side: the token and port the helper wrote.
#[cfg(test)]
pub fn told(dir: &Path) -> Option<(u16, String)> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("control.json")).ok()?).ok()?;
    Some((v["port"].as_u64()? as u16, v["token"].as_str()?.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    #[test]
    fn a_window_with_the_token_steers_and_hears_what_is_going_on() {
        let dir = std::env::temp_dir().join(format!("kagaz-control-test-{}", std::process::id()));
        let heard = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let h = heard.clone();
        remember(&serde_json::json!({ "kind": "devices", "devices": [] }));
        serve(&dir, move |l| h.lock().unwrap().push(l.to_string())).unwrap();
        let (port, token) = told(&dir).unwrap();

        // Without the token: nothing is taken, nothing is told.
        let mut wrong = TcpStream::connect(("127.0.0.1", port)).unwrap();
        writeln!(wrong, "{}", "0".repeat(64)).unwrap();
        writeln!(wrong, "pair").unwrap();
        let mut buf = String::new();
        let _ = wrong.set_read_timeout(Some(Duration::from_secs(2)));
        let got = BufReader::new(&wrong).read_line(&mut buf).unwrap_or(0);
        assert_eq!(got, 0, "told {buf:?}");

        let mut right = TcpStream::connect(("127.0.0.1", port)).unwrap();
        writeln!(right, "{token}").unwrap();
        let mut reader = BufReader::new(right.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        assert!(first.contains("\"helper\""), "{first}");
        let mut second = String::new();
        reader.read_line(&mut second).unwrap();
        assert!(second.contains("\"devices\""), "{second}");
        writeln!(right, "devices").unwrap();
        for _ in 0..50 {
            if !heard.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(heard.lock().unwrap().as_slice(), ["devices"]);
        assert!(connected());
        send("@{\"kind\":\"test\"}");
        let mut third = String::new();
        reader.read_line(&mut third).unwrap();
        assert!(third.contains("test"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
