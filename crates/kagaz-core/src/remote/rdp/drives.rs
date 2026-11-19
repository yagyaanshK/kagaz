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

    fn add_drive(&mut self, device_id: u32) -> PduResult<()> {
        self.of(device_id).add_drive(device_id)
    }

    fn remove_drive(&mut self, device_id: u32) -> PduResult<Vec<SvcMessage>> {
        self.of(device_id).remove_drive(device_id)
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

/// The helper the computer runs (from `\\tsclient\Kagaz\.kagaz`) to save
/// a batch. Given `x,y` (files dropped on its screen there), it saves them
/// where they were let go, as Windows does with a drop: into the folder
/// under the point in File Explorer, else into the folder that window
/// shows (and chooses them there), or onto the desktop (into a folder on
/// it, if let go over one). Otherwise, and where that cannot be told,
/// Windows' own folder chooser asks. Then Windows' own copy, with its
/// progress and its questions about files already there; a chosen
/// folder opens with them. Written in PowerShell, which every Windows
/// has; nothing is installed.
pub const SAVE_SCRIPT: &str = r#"param([string]$Batch, [string]$At = '')
$ErrorActionPreference = 'Stop'
$src = Join-Path '\\tsclient\Kagaz' $Batch
# The drive can take a moment to appear.
for ($i = 0; $i -lt 40 -and -not (Test-Path -LiteralPath $src); $i++) { Start-Sleep -Milliseconds 250 }
$items = @(Get-ChildItem -LiteralPath $src -Force)
if ($items.Count -eq 0) { exit }
Add-Type -AssemblyName Microsoft.VisualBasic
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class KagazFolder {
  [ComImport, Guid("DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7")] class Dialog {}
  [ComImport, Guid("42f85136-db7e-439c-85f1-e4075d135fc8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IFileDialog {
    [PreserveSig] int Show(IntPtr parent);
    void SetFileTypes(uint count, IntPtr types);
    void SetFileTypeIndex(uint index);
    void GetFileTypeIndex(out uint index);
    void Advise(IntPtr events, out uint cookie);
    void Unadvise(uint cookie);
    void SetOptions(uint options);
    void GetOptions(out uint options);
    void SetDefaultFolder(IShellItem item);
    void SetFolder(IShellItem item);
    void GetFolder(out IShellItem item);
    void GetCurrentSelection(out IShellItem item);
    void SetFileName([MarshalAs(UnmanagedType.LPWStr)] string name);
    void GetFileName([MarshalAs(UnmanagedType.LPWStr)] out string name);
    void SetTitle([MarshalAs(UnmanagedType.LPWStr)] string title);
    void SetOkButtonLabel([MarshalAs(UnmanagedType.LPWStr)] string label);
    void SetFileNameLabel([MarshalAs(UnmanagedType.LPWStr)] string label);
    void GetResult(out IShellItem item);
  }
  [ComImport, Guid("43826D1E-E718-42EE-BC55-A1E261C37BFE"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IShellItem {
    void BindToHandler(IntPtr bindContext, ref Guid handler, ref Guid iid, out IntPtr result);
    void GetParent(out IShellItem parent);
    void GetDisplayName(uint form, [MarshalAs(UnmanagedType.LPWStr)] out string name);
  }
  [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
  static extern void SHCreateItemFromParsingName(string path, IntPtr bindContext, ref Guid iid, out IShellItem item);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
  [StructLayout(LayoutKind.Sequential)] struct Point { public int X; public int Y; }
  [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
  [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr window, uint flags);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr window, System.Text.StringBuilder name, int size);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr window, System.Text.StringBuilder text, int size);
  // The screen's own pixels, as the session's picture has them, whatever
  // the scaling there.
  public static void Aware() {
    try { if (SetProcessDpiAwarenessContext(new IntPtr(-4))) return; } catch {}
    try { SetProcessDPIAware(); } catch {}
  }
  // The top window under a point of the screen.
  public static IntPtr Under(int x, int y) {
    Point p; p.X = x; p.Y = y;
    return GetAncestor(WindowFromPoint(p), 2);
  }
  public static string Kind(IntPtr window) {
    var s = new System.Text.StringBuilder(256); GetClassName(window, s, 256); return s.ToString();
  }
  public static string Title(IntPtr window) {
    var s = new System.Text.StringBuilder(512); GetWindowText(window, s, 512); return s.ToString();
  }
  public static string Pick(string title, string start) {
    IFileDialog d = (IFileDialog)new Dialog();
    // Folders only, on a disk, that are there.
    d.SetOptions(0x20 | 0x40 | 0x800);
    d.SetTitle(title);
    d.SetOkButtonLabel("Save here");
    try {
      Guid iid = typeof(IShellItem).GUID;
      IShellItem folder;
      SHCreateItemFromParsingName(start, IntPtr.Zero, ref iid, out folder);
      d.SetFolder(folder);
    } catch {}
    if (d.Show(GetForegroundWindow()) != 0) return null;
    IShellItem result;
    d.GetResult(out result);
    string path;
    result.GetDisplayName(0x80058000, out path);
    return path;
  }
}
'@
$shell = New-Object -ComObject Shell.Application
$downloads = $shell.NameSpace('shell:Downloads').Self.Path
$start = $downloads
$to = $null
$view = $null
function Test-Folder($path) { $path -and [System.IO.Path]::IsPathRooted($path) -and (Test-Path -LiteralPath $path -PathType Container) }
if ($At -match '^(-?\d+),(-?\d+)$') {
  try {
    $x = [int]$Matches[1]; $y = [int]$Matches[2]
    [KagazFolder]::Aware()
    $window = [KagazFolder]::Under($x, $y)
    $kind = [KagazFolder]::Kind($window)
    # What is under the point: an item (its name), or the folder tree.
    $name = $null; $tree = $false
    try {
      Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
      $e = [System.Windows.Automation.AutomationElement]::FromPoint((New-Object System.Windows.Point($x, $y)))
      $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
      for ($i = 0; $e -and $i -lt 6; $i++) {
        $type = $e.Current.ControlType
        if ($type -eq [System.Windows.Automation.ControlType]::ListItem) { $name = $e.Current.Name; break }
        if ($type -eq [System.Windows.Automation.ControlType]::TreeItem) { $tree = $true; break }
        $e = $walker.GetParent($e)
      }
    } catch {}
    if ($kind -eq 'CabinetWClass') {
      # File Explorer: of its tabs, the one shown (named in its title).
      $title = [KagazFolder]::Title($window)
      $tabs = @($shell.Windows() | Where-Object { try { [int64]$_.HWND -eq $window.ToInt64() } catch { $false } })
      $w = $tabs | Where-Object { $_.LocationName -and ($title -eq $_.LocationName -or $title.StartsWith($_.LocationName + ' - ')) } | Select-Object -First 1
      if (-not $w -and $tabs.Count -eq 1) { $w = $tabs[0] }
      if ($w) {
        $folder = $w.Document.Folder
        $here = $folder.Self.Path
        if ($name -and -not $tree) {
          $item = $folder.ParseName($name)
          if (-not $item -and -not (Test-Folder $here)) { $item = @($folder.Items()) | Where-Object { $_.Name -eq $name } | Select-Object -First 1 }
          if ($item -and (Test-Folder $item.Path)) { $to = $item.Path }
        }
        if (-not $to -and -not $tree -and (Test-Folder $here)) { $to = $here; $view = $w }
        if (-not $to -and (Test-Folder $here)) { $start = $here }
      }
    } elseif ($kind -eq 'Progman' -or $kind -eq 'WorkerW') {
      $desktop = [Environment]::GetFolderPath('Desktop')
      if ($name) {
        foreach ($d in @($desktop, [Environment]::GetFolderPath('CommonDesktopDirectory'))) {
          $p = Join-Path $d $name
          if (Test-Folder $p) { $to = $p; break }
        }
      }
      if (-not $to) { $to = $desktop }
    }
  } catch { $to = $null; $view = $null }
}
$chosen = $false
if (-not $to) {
  $what = if ($items.Count -eq 1) { $items[0].Name } else { "$($items.Count) files" }
  $to = [KagazFolder]::Pick("Save $what from Kagaz", $start)
  if (-not $to) { exit }
  $chosen = $true
}
[Microsoft.VisualBasic.FileIO.FileSystem]::CopyDirectory($src, $to, [Microsoft.VisualBasic.FileIO.UIOption]::AllDialogs, [Microsoft.VisualBasic.FileIO.UICancelOption]::DoNothing)
if ($view) {
  # Chosen in the window they were let go on, once it shows them.
  for ($i = 0; $i -lt 15; $i++) {
    Start-Sleep -Milliseconds 200
    try {
      $first = $true
      foreach ($n in $items) {
        $it = $view.Document.Folder.ParseName($n.Name)
        if ($it) { $view.Document.SelectItem($it, $(if ($first) { 29 } else { 1 })); $first = $false }
      }
      if (@($view.Document.SelectedItems()).Count -ge $items.Count) { break }
    } catch {}
  }
} elseif ($chosen) {
  Start-Process explorer.exe -ArgumentList "/select,`"$(Join-Path $to $items[0].Name)`""
}
"#;

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

/// The line typed into Windows' Run box for a helper on the Kagaz drive.
pub fn helper_line(script: &str, argument: &str) -> String {
    format!(r#"powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \\tsclient\Kagaz\.kagaz\{script} "{argument}""#)
}

/// Where a batch goes there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// Windows' folder chooser asks.
    Ask,
    /// The batch's folder on the drive opens.
    Open,
    /// Let go at this point of the screen there.
    At(i32, i32),
}

/// The line typed into Windows' Run box for a batch: the save helper, or
/// the batch's folder opened.
pub fn run_line(batch: &str, place: Place) -> String {
    match place {
        Place::Ask => helper_line("save.ps1", batch),
        Place::At(x, y) => format!(r#"{} "{x},{y}""#, helper_line("save.ps1", batch)),
        Place::Open => format!(r#"explorer "\\tsclient\Kagaz\{batch}""#),
    }
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
        assert!(run_line("Sent 1", Place::At(640, -2)).ends_with(r#"save.ps1 "Sent 1" "640,-2""#));
        assert!(run_line("Sent 1", Place::Ask).ends_with(r#"save.ps1 "Sent 1""#));
        let _ = std::fs::remove_dir_all(&here);
    }
}
