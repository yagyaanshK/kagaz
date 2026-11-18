//! Windows: Kagaz's helper service, installed only when the person
//! chooses it (This computer › Sign-in screen and administrator prompts),
//! with an administrator's password, and removed when they choose
//! otherwise. Without it nothing of Kagaz runs with raised rights.
//!
//! The service ("KagazHelper", shown as "Kagaz helper") runs as the
//! system and keeps one program running in the session at the computer's
//! own screen: this one, with `--system`, which shares that screen
//! whichever desktop Windows shows (the person's, the sign-in screen, an
//! administrator prompt), also after a restart before anyone signs in.
//! Kagaz's window steers it over control.rs when someone is signed in.
//!
//! Its folder, `C:\ProgramData\Kagaz`, is the system's and
//! administrators' only, except: `bin` (the program, readable by all),
//! `control` (how to reach it, and the record of sessions: readable by the
//! person who installed it) and `received` (files sent from a viewer:
//! theirs to take).
//!
//!   kagaz-share helper install --from <Kagaz's share folder> [--settings <share.json>]
//!   kagaz-share helper remove --to <Kagaz's share folder>
//!
//! Each asks Windows' administrator question (and then runs again, raised,
//! as `install-raised` / `remove-raised`), and ends 0 when done, 1223 when
//! the question was not answered yes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenPrimary, TokenSessionId, TOKEN_ALL_ACCESS,
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW, SERVICE_ACCEPT_SHUTDOWN,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING,
    SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOPPED,
    SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, TerminateProcess,
    WaitForSingleObject, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

/// The names the owner chose (2026-11-19).
pub const NAME: &str = "KagazHelper";
const SHOWN: &str = "Kagaz helper";
const ABOUT: &str = "Lets a device paired with Kagaz see and answer the sign-in screen and administrator prompts. Installed and removed from Kagaz, This computer.";

const SYSTEM: &str = "*S-1-5-18";
const ADMINISTRATORS: &str = "*S-1-5-32-544";
const USERS: &str = "*S-1-5-32-545";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `C:\ProgramData\Kagaz`.
pub fn home() -> PathBuf {
    std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData")).join("Kagaz")
}

fn quiet(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW.0);
    c
}

fn run(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let out = quiet(program).args(args).output()?;
    anyhow::ensure!(out.status.success(), "{program} {} ended with {}", args.first().unwrap_or(&""), out.status);
    Ok(())
}

/// The person running this, as Windows names them (S-1-5-21-…).
fn my_sid() -> anyhow::Result<String> {
    let out = quiet("whoami").args(["/user", "/fo", "csv", "/nh"]).output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let sid = text.trim().rsplit(',').next().unwrap_or("").trim_matches('"').to_string();
    anyhow::ensure!(sid.starts_with("S-1-"), "who is running Kagaz could not be found");
    Ok(sid)
}

/// Runs this program again with `args`, raised, after Windows' question;
/// its ending code (1223 when the question was not answered yes).
fn raised(args: &[String]) -> anyhow::Result<u32> {
    let me = std::env::current_exe()?;
    // Each in quotes; a backslash before the closing quote would escape it.
    let line = args.iter().map(|a| format!("\"{}\"", a.replace('"', "").trim_end_matches('\\'))).collect::<Vec<_>>().join(" ");
    let (file, params, verb) = (wide(&me.to_string_lossy()), wide(&line), wide("runas"));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: 0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        // ERROR_CANCELLED: the question was answered no.
        if e.code().0 as u32 & 0xFFFF == 1223 {
            return Ok(1223);
        }
        return Err(e.into());
    }
    let mut code = 1u32;
    unsafe {
        WaitForSingleObject(info.hProcess, u32::MAX);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(code)
}

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// `kagaz-share helper …`.
pub fn main(args: &[String]) -> anyhow::Result<()> {
    let what = args.get(2).map(String::as_str).unwrap_or("");
    let code = match what {
        "install" => {
            let from = value(args, "--from").ok_or_else(|| anyhow::anyhow!("--from <Kagaz's share folder>"))?;
            let mut again = vec!["helper".into(), "install-raised".into(), "--from".into(), from, "--sid".into(), my_sid()?];
            if let Some(s) = value(args, "--settings") {
                again.extend(["--settings".into(), s]);
            }
            raised(&again)?
        }
        "remove" => {
            let to = value(args, "--to").ok_or_else(|| anyhow::anyhow!("--to <Kagaz's share folder>"))?;
            raised(&["helper".into(), "remove-raised".into(), "--to".into(), to])?
        }
        "install-raised" => {
            let from = value(args, "--from").ok_or_else(|| anyhow::anyhow!("--from"))?;
            let sid = value(args, "--sid").filter(|s| s.starts_with("S-1-") && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).ok_or_else(|| anyhow::anyhow!("--sid"))?;
            match install(Path::new(&from), &sid, value(args, "--settings").as_deref().map(Path::new)) {
                Ok(()) => 0,
                Err(e) => {
                    let _ = std::fs::write(std::env::temp_dir().join("kagaz-helper-install.txt"), format!("{e:#}"));
                    1
                }
            }
        }
        "remove-raised" => {
            let to = value(args, "--to").ok_or_else(|| anyhow::anyhow!("--to"))?;
            match remove(Path::new(&to)) {
                Ok(()) => 0,
                Err(e) => {
                    let _ = std::fs::write(std::env::temp_dir().join("kagaz-helper-remove.txt"), format!("{e:#}"));
                    1
                }
            }
        }
        "service" => {
            service()?;
            0
        }
        _ => anyhow::bail!("kagaz-share helper install --from DIR [--settings FILE] | remove --to DIR"),
    };
    std::process::exit(code as i32);
}

/// Is the helper installed?
pub fn installed() -> bool {
    quiet("sc.exe").args(["query", NAME]).output().is_ok_and(|o| o.status.success())
}

fn stop_and_delete() {
    let _ = run("sc.exe", &["stop", NAME]);
    let until = Instant::now() + Duration::from_secs(15);
    while Instant::now() < until {
        let out = quiet("sc.exe").args(["query", NAME]).output();
        match out {
            Ok(o) if o.status.success() && !String::from_utf8_lossy(&o.stdout).contains("STOPPED") => {
                std::thread::sleep(Duration::from_millis(300))
            }
            _ => break,
        }
    }
    let _ = run("sc.exe", &["delete", NAME]);
}

fn copy_if_there(from: &Path, to: &Path) -> anyhow::Result<()> {
    if from.is_file() {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// Raised: the folder with its rights, the program, what Kagaz kept for
/// sharing (the same certificate, so paired devices know it), and the
/// service, started.
fn install(from: &Path, sid: &str, settings: Option<&Path>) -> anyhow::Result<()> {
    if installed() {
        stop_and_delete();
    }
    let home = home();
    let (bin, share, control, received) = (home.join("bin"), home.join("share"), home.join("control"), home.join("received"));
    for d in [&home, &bin, &share, &control, &received] {
        std::fs::create_dir_all(d)?;
    }
    let h = home.to_string_lossy().to_string();
    let full = |who: &str| format!("{who}:(OI)(CI)F");
    run("icacls", &[&h, "/inheritance:r", "/grant:r", &full(SYSTEM), &full(ADMINISTRATORS)])?;
    run("icacls", &[&bin.to_string_lossy(), "/grant", &format!("{USERS}:(OI)(CI)RX")])?;
    run("icacls", &[&control.to_string_lossy(), "/grant", &format!("*{sid}:(OI)(CI)R")])?;
    run("icacls", &[&received.to_string_lossy(), "/grant", &format!("*{sid}:(OI)(CI)M")])?;
    std::fs::copy(std::env::current_exe()?, bin.join("kagaz-share.exe"))?;
    for name in ["paired.json", "certificate.der", "key.der"] {
        copy_if_there(&from.join(name), &share.join(name))?;
    }
    copy_if_there(&from.join("sessions.jsonl"), &control.join("sessions.jsonl"))?;
    if let Some(s) = settings {
        copy_if_there(s, &share.join("settings.json"))?;
    }
    let kept = serde_json::json!({ "sid": sid, "from": from });
    std::fs::write(share.join("helper.json"), kept.to_string())?;
    let program = format!("\"{}\" helper service", bin.join("kagaz-share.exe").to_string_lossy());
    run("sc.exe", &["create", NAME, "binPath=", &program, "start=", "auto", "DisplayName=", SHOWN])?;
    let _ = run("sc.exe", &["description", NAME, ABOUT]);
    let _ = run("sc.exe", &["failure", NAME, "reset=", "86400", "actions=", "restart/5000/restart/5000/restart/60000"]);
    run("sc.exe", &["start", NAME])?;
    Ok(())
}

/// Raised: the service stopped and gone, the paired devices and the record
/// handed back to Kagaz's folder, and the folder removed.
fn remove(to: &Path) -> anyhow::Result<()> {
    stop_and_delete();
    let home = home();
    std::fs::create_dir_all(to)?;
    copy_if_there(&home.join("share").join("paired.json"), &to.join("paired.json"))?;
    copy_if_there(&home.join("control").join("sessions.jsonl"), &to.join("sessions.jsonl"))?;
    // The program can take a moment to let go of its files.
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match std::fs::remove_dir_all(&home) {
            Ok(()) => return Ok(()),
            Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(500)),
            Err(e) => return Err(anyhow::anyhow!("{} could not be removed: {e}", home.display())),
        }
    }
}

// ---- the service ----

static STOP: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicUsize = AtomicUsize::new(0);

fn tell(state: SERVICE_STATUS_CURRENT_STATE) {
    let handle = SERVICE_STATUS_HANDLE(STATUS.load(Ordering::Acquire) as *mut core::ffi::c_void);
    if handle.0.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 },
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: if state == SERVICE_RUNNING || state == SERVICE_STOPPED { 0 } else { 10_000 },
    };
    unsafe {
        let _ = SetServiceStatus(handle, &status);
    }
}

unsafe extern "system" fn handler(control: u32, _: u32, _: *mut core::ffi::c_void, _: *mut core::ffi::c_void) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            STOP.store(true, Ordering::Release);
            tell(SERVICE_STOP_PENDING);
            0
        }
        SERVICE_CONTROL_INTERROGATE => 0,
        // ERROR_CALL_NOT_IMPLEMENTED
        _ => 120,
    }
}

unsafe extern "system" fn service_main(_: u32, _: *mut PWSTR) {
    let name = wide(NAME);
    let Ok(handle) = (unsafe { RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(handler), None) }) else { return };
    STATUS.store(handle.0 as usize, Ordering::Release);
    tell(SERVICE_START_PENDING);
    tell(SERVICE_RUNNING);
    supervise();
    tell(SERVICE_STOPPED);
}

fn service() -> anyhow::Result<()> {
    let mut name = wide(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR(name.as_mut_ptr()), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR::null(), lpServiceProc: None },
    ];
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }?;
    Ok(())
}

/// Keeps `kagaz-share --system` running in the session at the computer's
/// own screen, moving with it when someone else signs in there.
fn supervise() {
    let mut child: Option<(HANDLE, u32)> = None;
    let mut started = Instant::now() - Duration::from_secs(60);
    while !STOP.load(Ordering::Acquire) {
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if let Some((process, at)) = child {
            let ended = unsafe { WaitForSingleObject(process, 0) } == WAIT_OBJECT_0;
            if ended || at != session {
                unsafe {
                    if !ended {
                        let _ = TerminateProcess(process, 0);
                    }
                    let _ = CloseHandle(process);
                }
                child = None;
            }
        }
        // Not more often than every two seconds, should it keep ending.
        if child.is_none() && session != u32::MAX && started.elapsed() > Duration::from_secs(2) {
            started = Instant::now();
            if let Ok(process) = launch(session) {
                child = Some((process, session));
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if let Some((process, _)) = child {
        unsafe {
            let _ = TerminateProcess(process, 0);
            let _ = CloseHandle(process);
        }
    }
}

/// This program with `--system`, as the system, in `session`.
fn launch(session: u32) -> anyhow::Result<HANDLE> {
    let me = std::env::current_exe()?;
    let mut line = wide(&format!("\"{}\" --system", me.to_string_lossy()));
    let mut desktop = wide("winsta0\\default");
    unsafe {
        let mut own = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own)?;
        let mut token = HANDLE::default();
        let duplicated = DuplicateTokenEx(own, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut token);
        let _ = CloseHandle(own);
        duplicated?;
        let set = SetTokenInformation(token, TokenSessionId, (&session as *const u32).cast(), 4);
        if let Err(e) = set {
            let _ = CloseHandle(token);
            return Err(e.into());
        }
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let made = CreateProcessAsUserW(
            Some(token),
            PCWSTR::null(),
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        );
        let _ = CloseHandle(token);
        made?;
        let _ = CloseHandle(pi.hThread);
        Ok(pi.hProcess)
    }
}
