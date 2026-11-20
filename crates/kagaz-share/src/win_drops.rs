//! Files dropped from Kagaz over Remote Desktop (`kagaz-share --drops`):
//! where the person turned it on (Kagaz's Share page there), this starts
//! at sign-in and waits. In a Remote Desktop session from Kagaz, once
//! Kagaz's drive appears (`\\tsclient\Kagaz`, shown when files are first
//! sent or dragged), it says so on the drive and starts Kagaz's helper
//! from its own copy, so that Kagaz need not type it into Windows' Run
//! box. Outside such a session it does nothing but look, once a second,
//! whether one began. It ends when the setting is turned off.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Kagaz's helper (helper.ps1 in the core, which says what it does).
const HELPER: &str = include_str!("../../kagaz-core/src/remote/rdp/helper.ps1");
/// Where Kagaz asks it things, in a session from Kagaz.
const DRIVE: &str = r"\\tsclient\Kagaz\.kagaz";

/// Waits for sessions from Kagaz until `settings` (Kagaz's share.json)
/// no longer has it on. One at a time: a second one ends at once.
pub fn run(settings: Option<PathBuf>) -> anyhow::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let lock = std::env::temp_dir().join("kagaz-drops.lock");
    let Ok(_held) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).share_mode(0).open(&lock) else {
        return Ok(());
    };
    let home = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Kagaz");
    std::fs::create_dir_all(&home)?;
    let script = home.join("helper.ps1");
    std::fs::write(&script, HELPER)?;
    let mut started: Option<Instant> = None;
    let mut asked = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if let Some(path) = &settings {
            if asked.elapsed() >= Duration::from_secs(5) {
                asked = Instant::now();
                if !still_on(path) {
                    return Ok(());
                }
            }
        }
        if !remote_session() {
            continue;
        }
        let dir = Path::new(DRIVE);
        if !dir.is_dir() {
            continue;
        }
        let here = dir.join("share-here.txt");
        if !here.exists() {
            let _ = std::fs::write(&here, b"1");
        }
        if alive(dir) || started.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
            continue;
        }
        use std::os::windows::process::CommandExt;
        let spawned = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"])
            .arg(&script)
            .creation_flags(0x0800_0000)
            .spawn();
        if spawned.is_ok() {
            started = Some(Instant::now());
        }
    }
}

/// Kagaz's setting is still on (a missing or unreadable file: off).
fn still_on(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("drops").and_then(|d| d.as_bool()))
        .unwrap_or(false)
}

/// This is a Remote Desktop session.
fn remote_session() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

/// The helper says it is running, every two seconds.
fn alive(dir: &Path) -> bool {
    std::fs::metadata(dir.join("helper-alive.txt"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(6))
}
