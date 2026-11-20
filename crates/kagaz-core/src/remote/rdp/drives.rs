//! Folders of this computer shown to a Windows computer as drives over
//! Remote Desktop (as Windows' own viewer shares "local resources"):
//!
//! - "Kagaz" (`\\tsclient\Kagaz` there): a folder of Kagaz's own for this
//!   session, holding only the files the person sends from here. It is
//!   shown there only once something is sent, and goes with the session.
//! - The folder the person chose to share with that device (its own
//!   name there), from the start of every session, both ways.
//!
//! Each is served by IronRDP's filesystem backend for this system, which
//! keeps every path the computer asks for inside its folder and follows
//! no links out of it; this one sends each request to the folder of the
//! drive it is for.

use ironrdp_core::impl_as_any;
use ironrdp_pdu::PduResult;
use ironrdp_rdpdr::pdu::efs::{
    AnyIoCtlCode, DecodedDeviceControlRequest, DeviceControlRequest, ServerDeviceAnnounceResponse, ServerDriveIoRequest,
};
use ironrdp_rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};
use ironrdp_rdpdr::{Rdpdr, RdpdrBackend};
use ironrdp_rdpdr_native::backend::NixRdpdrBackend;
use ironrdp_svc::SvcMessage;
use std::path::{Path, PathBuf};

/// The drive of files sent from here.
pub const SENT: u32 = 1;
/// The folder shared with the device.
pub const SHARED: u32 = 2;
/// The name the sent files' drive has there.
pub const SENT_NAME: &str = "Kagaz";

/// The session's own folder for files sent from here, made empty.
pub fn sent_folder() -> std::io::Result<PathBuf> {
    let base = crate::paths::cache_dir().join("sent");
    std::fs::create_dir_all(&base)?;
    // Sessions that ended without tidying (a crash) leave theirs: gone
    // after a day.
    if let Ok(entries) = std::fs::read_dir(&base) {
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        for e in entries.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > day);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    // A name of its own: the time to the nanosecond and this process.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = base.join(format!("{nanos:x}-{}", std::process::id()));
    std::fs::create_dir(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// The name a shared folder has there: its own, as Windows can show it.
pub fn shared_name(folder: &Path) -> String {
    let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let clean: String = name.chars().filter(|c| !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0')).collect();
    let clean = clean.trim().trim_end_matches('.').to_string();
    if clean.is_empty() || clean.eq_ignore_ascii_case(SENT_NAME) {
        "Shared".into()
    } else {
        clean
    }
}

#[derive(Debug)]
pub struct Drives {
    sent: NixRdpdrBackend,
    shared: Option<NixRdpdrBackend>,
}

impl_as_any!(Drives);

impl Drives {
    pub fn new(sent: &Path, shared: Option<&Path>) -> Drives {
        Drives {
            sent: NixRdpdrBackend::new(sent.to_string_lossy().to_string()),
            shared: shared.map(|p| NixRdpdrBackend::new(p.to_string_lossy().to_string())),
        }
    }

    fn of(&mut self, device: u32) -> &mut NixRdpdrBackend {
        match (device, self.shared.as_mut()) {
            (SHARED, Some(shared)) => shared,
            _ => &mut self.sent,
        }
    }
}

fn device_of(req: &ServerDriveIoRequest) -> u32 {
    use ServerDriveIoRequest as R;
    match req {
        R::ServerCreateDriveRequest(r) => r.device_io_request.device_id,
        R::ServerDriveQueryInformationRequest(r) => r.device_io_request.device_id,
        R::DeviceCloseRequest(r) => r.device_io_request.device_id,
        R::ServerDriveQueryDirectoryRequest(r) => r.device_io_request.device_id,
        R::ServerDriveNotifyChangeDirectoryRequest(r) => r.device_io_request.device_id,
        R::ServerDriveQueryVolumeInformationRequest(r) => r.device_io_request.device_id,
        R::DeviceControlRequest(r) => r.header.device_id,
        R::DeviceReadRequest(r) => r.device_io_request.device_id,
        R::DeviceWriteRequest(r) => r.device_io_request.device_id,
        R::DeviceFlushBuffersRequest(r) => r.device_io_request.device_id,
        R::ServerDriveSetInformationRequest(r) => r.device_io_request.device_id,
        R::ServerDriveLockControlRequest(r) => r.device_io_request.device_id,
        R::ServerDriveQuerySecurityRequest(r) => r.device_io_request.device_id,
        R::ServerDriveSetSecurityRequest(r) => r.device_io_request.device_id,
    }
}

impl RdpdrBackend for Drives {
    fn reset(&mut self) -> PduResult<()> {
        self.sent.reset()?;
        if let Some(s) = self.shared.as_mut() {
            s.reset()?;
        }
        Ok(())
    }

    fn restore_drive(&mut self, device_id: u32) -> PduResult<()> {
        self.of(device_id).restore_drive(device_id)
    }

    // Each drive is served from its own folder, ready from the start: the
    // backend for this system keeps nothing per drive, and refuses to be
    // asked (IronRDP's default), which kept the drive of sent files from
    // being shown during a session.
    fn add_drive(&mut self, _device_id: u32) -> PduResult<()> {
        Ok(())
    }

    fn remove_drive(&mut self, _device_id: u32) -> PduResult<Vec<SvcMessage>> {
        Ok(Vec::new())
    }

    fn handle_server_device_announce_response(&mut self, pdu: ServerDeviceAnnounceResponse) -> PduResult<()> {
        let device = pdu.device_id;
        self.of(device).handle_server_device_announce_response(pdu)
    }

    fn handle_scard_call(&mut self, req: DeviceControlRequest<ScardIoCtlCode>, call: ScardCall) -> PduResult<Vec<SvcMessage>> {
        self.sent.handle_scard_call(req, call)
    }

    fn handle_drive_io_request(&mut self, req: ServerDriveIoRequest) -> PduResult<Vec<SvcMessage>> {
        let device = device_of(&req);
        self.of(device).handle_drive_io_request(req)
    }

    fn handle_drive_device_control(&mut self, req: DecodedDeviceControlRequest<AnyIoCtlCode>) -> PduResult<Vec<SvcMessage>> {
        let device = req.request.header.device_id;
        self.of(device).handle_drive_device_control(req)
    }

    fn poll_deferred_messages(&mut self) -> PduResult<Vec<SvcMessage>> {
        let mut all = self.sent.poll_deferred_messages()?;
        if let Some(s) = self.shared.as_mut() {
            all.extend(s.poll_deferred_messages()?);
        }
        Ok(all)
    }

    fn handle_user_logged_on(&mut self, rdpdr: &mut Rdpdr) -> PduResult<Vec<SvcMessage>> {
        let mut all = self.sent.handle_user_logged_on(rdpdr)?;
        if let Some(s) = self.shared.as_mut() {
            all.extend(s.handle_user_logged_on(rdpdr)?);
        }
        Ok(all)
    }
}

/// Files (and folders) sent from here, put in the session's folder of sent
/// files as `Sent <n>` under names a Windows computer can keep: linked
/// where they are on the same disk, copied where not. `progress` hears
/// the bytes done and the bytes in all. The name of the batch's folder.
pub fn stage(sent: &Path, paths: &[PathBuf], mut progress: impl FnMut(u64, u64)) -> Result<String, String> {
    if paths.is_empty() {
        return Err("no file was chosen".into());
    }
    let mut n = 1;
    let batch = loop {
        let name = format!("Sent {n}");
        if !sent.join(&name).exists() {
            break name;
        }
        n += 1;
    };
    let into = sent.join(&batch);
    std::fs::create_dir(&into).map_err(|e| format!("the files could not be got ready ({e})"))?;
    // What there is, and how much.
    fn walk(from: &Path, to: PathBuf, list: &mut Vec<(PathBuf, PathBuf, u64)>) -> std::io::Result<()> {
        let meta = std::fs::metadata(from)?;
        if meta.is_dir() {
            list.push((from.to_path_buf(), to.clone(), u64::MAX));
            let mut taken = std::collections::HashSet::new();
            for e in std::fs::read_dir(from)? {
                let e = e?;
                let mut name = super::clip::name_there(&e.file_name().to_string_lossy());
                let mut k = 2;
                while !taken.insert(name.to_lowercase()) {
                    name = super::clip::numbered(&super::clip::name_there(&e.file_name().to_string_lossy()), k);
                    k += 1;
                }
                walk(&e.path(), to.join(name), list)?;
            }
        } else if meta.is_file() {
            list.push((from.to_path_buf(), to, meta.len()));
        }
        Ok(())
    }
    let mut list = Vec::new();
    let mut taken = std::collections::HashSet::new();
    for p in paths {
        let base = super::clip::name_there(&p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        let mut name = base.clone();
        let mut k = 2;
        while !taken.insert(name.to_lowercase()) {
            name = super::clip::numbered(&base, k);
            k += 1;
        }
        walk(p, into.join(name), &mut list).map_err(|e| format!("{} could not be read ({e})", p.display()))?;
    }
    let total: u64 = list.iter().filter(|(_, _, size)| *size != u64::MAX).map(|(_, _, size)| *size).sum();
    let mut done = 0;
    progress(0, total);
    for (from, to, size) in list {
        if size == u64::MAX {
            std::fs::create_dir_all(&to).map_err(|e| format!("the files could not be got ready ({e})"))?;
            continue;
        }
        if std::fs::hard_link(&from, &to).is_err() {
            std::fs::copy(&from, &to).map_err(|e| format!("{} could not be read ({e})", from.display()))?;
        }
        done += size;
        progress(done, total);
    }
    Ok(batch)
}

/// Kagaz's helper on a Windows computer (helper.ps1, which says how it
/// is asked and answers): started once a session from the Kagaz drive,
/// it holds real Windows drags of files sent from here, saves them where
/// they were let go (else Windows' folder chooser asks), or opens them.
pub const HELPER_SCRIPT: &str = include_str!("helper.ps1");

/// The helper's folder on the drive, with the helper in it.
fn helper_dir(sent: &Path) -> Result<PathBuf, String> {
    let dir = sent.join(".kagaz");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script = dir.join("helper.ps1");
    if std::fs::metadata(&script).map(|m| m.len() as usize != HELPER_SCRIPT.len()).unwrap_or(true) {
        std::fs::write(&script, HELPER_SCRIPT).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

/// Asks the helper to do `what` ("drag", "save" or "open") with a batch,
/// with the point of the screen it was let go at. The question's id.
pub fn ask(sent: &Path, what: &str, batch: &str, at: Option<(i32, i32)>) -> Result<String, String> {
    let dir = helper_dir(sent)?;
    let id = format!(
        "{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    );
    let at = at.map(|(x, y)| format!("{x},{y}")).unwrap_or_default();
    // Whole or not at all: written aside, then named.
    let part = dir.join(format!("ask-{id}.part"));
    std::fs::write(&part, format!("{what}\r\n{batch}\r\n{at}\r\n")).map_err(|e| e.to_string())?;
    std::fs::rename(&part, dir.join(format!("ask-{id}.txt"))).map_err(|e| e.to_string())?;
    Ok(id)
}

/// Takes a question back if the helper has not taken it yet (true).
pub fn take_back(sent: &Path, id: &str) -> bool {
    std::fs::remove_file(sent.join(".kagaz").join(format!("ask-{id}.txt"))).is_ok()
}

/// Tells the helper to stop holding a drag.
pub fn cancel(sent: &Path, id: &str) {
    let _ = std::fs::write(sent.join(".kagaz").join(format!("cancel-{id}.txt")), b"1");
}

/// The helper's answer `kind` ("ready" or "done") to a question, once
/// there: for a drag, Windows' drop effect (0 when nothing took them);
/// -1 when it went wrong.
pub fn answer(sent: &Path, kind: &str, id: &str) -> Option<i32> {
    let text = std::fs::read(sent.join(".kagaz").join(format!("{kind}-{id}.txt"))).ok()?;
    // Written by PowerShell, perhaps as UTF-16: the number in it.
    let text: String = text.iter().filter(|b| b.is_ascii_digit() || **b == b'-').map(|b| *b as char).collect();
    text.parse().ok()
}

/// Whether the helper is running there (it says so every two seconds).
pub fn helper_alive(sent: &Path) -> bool {
    std::fs::metadata(sent.join(".kagaz").join("helper-alive.txt"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < std::time::Duration::from_secs(6))
}

/// Whether Kagaz share on the computer starts the helper by itself (it
/// says so once it sees the drive).
pub fn helper_started_there(sent: &Path) -> bool {
    sent.join(".kagaz").join("share-here.txt").exists()
}

/// The helper that lets a Windows computer draw and encode its Remote
/// Desktop screen with its graphics chip (`on`), or puts back what was
/// there before (`off`): Windows' own settings for it (the policies "Use
/// hardware graphics adapters for all Remote Desktop Services sessions",
/// "Configure H.264/AVC hardware encoding" and "Prioritize H.264/AVC 444
/// graphics mode", and up to 60 pictures a second instead of 30). Changing
/// them takes an administrator: the helper asks Windows for that, which
/// shows its own question there. What was there before is kept under
/// `HKLM\SOFTWARE\Kagaz\Graphics` until it is put back. The result goes
/// back to the drive: 0 done, 1223 the question answered no.
pub const GRAPHICS_SCRIPT: &str = r#"param([string]$Turn = 'on')
$result = '\\tsclient\Kagaz\.kagaz\graphics-result.txt'
$me = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  # Raised from a copy of its own: a raised program may not reach the drive.
  $copy = Join-Path $env:TEMP 'kagaz-graphics.ps1'
  Copy-Item -LiteralPath $PSCommandPath -Destination $copy -Force
  try {
    $p = Start-Process powershell -Verb RunAs -WindowStyle Hidden -Wait -PassThru -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$copy`"", $Turn)
    $code = $p.ExitCode
  } catch { $code = 1223 }
  Remove-Item -LiteralPath $copy -Force -ErrorAction SilentlyContinue
  Set-Content -LiteralPath $result -Value $code
  exit
}
$ts = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services'
$ws = 'HKLM:\SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations'
$kept = 'HKLM:\SOFTWARE\Kagaz\Graphics'
$values = @(@($ts, 'bEnumerateHWBeforeSW', 1), @($ts, 'AVCHardwareEncodePreferred', 1), @($ts, 'AVC444ModePreferred', 1), @($ws, 'DWMFRAMEINTERVAL', 15))
try {
  if ($Turn -eq 'on') {
    if (-not (Test-Path $kept)) {
      New-Item -Path $kept -Force | Out-Null
      foreach ($v in $values) {
        $was = (Get-ItemProperty -Path $v[0] -Name $v[1] -ErrorAction SilentlyContinue).($v[1])
        Set-ItemProperty -Path $kept -Name $v[1] -Value $(if ($null -eq $was) { 'none' } else { "$was" })
      }
    }
    New-Item -Path $ts -Force | Out-Null
    foreach ($v in $values) { Set-ItemProperty -Path $v[0] -Name $v[1] -Value $v[2] -Type DWord }
  } else {
    if (Test-Path $kept) {
      foreach ($v in $values) {
        $was = (Get-ItemProperty -Path $kept -Name $v[1] -ErrorAction SilentlyContinue).($v[1])
        if ($was -eq 'none' -or $null -eq $was) { Remove-ItemProperty -Path $v[0] -Name $v[1] -ErrorAction SilentlyContinue }
        else { Set-ItemProperty -Path $v[0] -Name $v[1] -Value ([int]$was) -Type DWord }
      }
      Remove-Item -Path $kept -Force
      if (-not (Get-ChildItem 'HKLM:\SOFTWARE\Kagaz' -ErrorAction SilentlyContinue)) { Remove-Item 'HKLM:\SOFTWARE\Kagaz' -Force -ErrorAction SilentlyContinue }
    }
  }
  exit 0
} catch { exit 1 }
"#;

/// The line typed into Windows' Run box for a helper on the Kagaz drive,
/// given `arguments`: PowerShell, waiting up to ten seconds for the drive
/// to appear there (it is announced only a moment before).
pub fn helper_line(script: &str, arguments: &[&str]) -> String {
    let arguments: String = arguments.iter().map(|a| format!(" '{}'", a.replace('\'', "''"))).collect();
    format!(
        r#"powershell -nop -ep bypass -w hidden -c "$f='\\tsclient\Kagaz\.kagaz\{script}'; for ($i = 0; $i -lt 40 -and !(Test-Path $f); $i++) {{ sleep -m 250 }}; & $f{arguments}""#
    )
}

/// The line typed into Windows' Run box to start the helper.
pub fn helper_run_line() -> String {
    helper_line("helper.ps1", &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_keeps_folders_and_names_windows_can_hold() {
        let here = std::env::temp_dir().join(format!("kagaz-stage-test-{}", std::process::id()));
        let from = here.join("from");
        let sent = here.join("sent");
        std::fs::create_dir_all(from.join("folder")).unwrap();
        std::fs::create_dir_all(&sent).unwrap();
        std::fs::write(from.join("a:b.txt"), b"one").unwrap();
        std::fs::write(from.join("folder").join("c.txt"), b"three").unwrap();
        let mut last = (0, 0);
        let batch = stage(&sent, &[from.join("a:b.txt"), from.join("folder")], |d, t| last = (d, t)).unwrap();
        assert_eq!(batch, "Sent 1");
        assert_eq!(std::fs::read(sent.join("Sent 1").join("a_b.txt")).unwrap(), b"one");
        assert_eq!(std::fs::read(sent.join("Sent 1").join("folder").join("c.txt")).unwrap(), b"three");
        assert_eq!(last, (8, 8));
        assert_eq!(stage(&sent, &[from.join("a:b.txt")], |_, _| {}).unwrap(), "Sent 2");
        assert_eq!(shared_name(Path::new("/home/x/Some: Folder.")), "Some Folder");
        assert_eq!(
            helper_line("graphics.ps1", &["on"]),
            r#"powershell -nop -ep bypass -w hidden -c "$f='\\tsclient\Kagaz\.kagaz\graphics.ps1'; for ($i = 0; $i -lt 40 -and !(Test-Path $f); $i++) { sleep -m 250 }; & $f 'on'""#
        );
        assert!(helper_line("x.ps1", &["It's"]).ends_with(r#"& $f 'It''s'""#));
        // Windows' Run box takes up to 259 letters.
        assert!(helper_run_line().chars().count() < 259);
        // Asked and answered.
        let id = ask(&sent, "save", "Sent 1", Some((640, -2))).unwrap();
        let asked = std::fs::read_to_string(sent.join(".kagaz").join(format!("ask-{id}.txt"))).unwrap();
        assert_eq!(asked, "save\r\nSent 1\r\n640,-2\r\n");
        assert_eq!(std::fs::read_to_string(sent.join(".kagaz").join("helper.ps1")).unwrap(), HELPER_SCRIPT);
        assert!(answer(&sent, "done", &id).is_none());
        std::fs::write(sent.join(".kagaz").join(format!("done-{id}.txt")), "\u{feff}5\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>()).unwrap();
        assert_eq!(answer(&sent, "done", &id), Some(5));
        assert!(take_back(&sent, &id));
        assert!(!take_back(&sent, &id));
        assert!(!helper_alive(&sent));
        let _ = std::fs::remove_dir_all(&here);
    }

    #[test]
    fn the_drive_of_sent_files_can_be_shown_during_a_session() {
        let here = std::env::temp_dir().join(format!("kagaz-drives-test-{}", std::process::id()));
        std::fs::create_dir_all(&here).unwrap();
        let mut channel = Rdpdr::new(Box::new(Drives::new(&here, None)), "Kagaz".into()).with_drives(None);
        assert!(channel.add_dynamic_drive(SENT, SENT_NAME.into()).is_ok());
        let _ = std::fs::remove_dir_all(&here);
    }
}
