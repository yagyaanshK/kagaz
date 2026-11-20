# Kagaz's helper on a Windows computer in a Remote Desktop session with
# Kagaz: started once a session (from Windows' Run box, or by Kagaz
# share where it is turned on there), it stays until the session's Kagaz
# drive goes. Kagaz asks it things by files in \\tsclient\Kagaz\.kagaz:
#
#   ask-<id>.txt     what to do (a line each: what, the batch's folder on
#                    the drive, then a point of the screen "x,y" or "")
#                      drag  hold a Windows drag of the batch's files at
#                            the pointer, for Kagaz to press, move and let
#                            go: a real drop, wherever it lands
#                      save  save them where the point is, else ask where
#                      open  open the batch's folder
#                      catch catch a drag of files made here, at the
#                            pointer (it left Kagaz's window there): its
#                            files' count goes in caught-<id>.txt; let go
#                            on it, Windows copies them to out-<id> on the
#                            drive, and out-<id>.txt says they are there;
#                            nothing caught, done-<id>.txt says 0
#   cancel-<id>.txt  stop holding that drag (or catching)
#
# The other way, files from here to Kagaz: while something is dragged
# here (the left button held and moved), a strip at the side of the
# screen takes files dropped on it; Windows copies them to the drive's
# in-<id> folder and in-<id>.txt says they are all there. name.txt holds
# the name of Kagaz's computer, for the strip to say.
#
# and it answers ready-<id>.txt (the drag is held) and done-<id>.txt (how
# it went: for a drag Windows' drop effect, 0 when nothing took them).
# helper-alive.txt is written every two seconds while it runs.
# Written in PowerShell, which every Windows has; nothing is installed.
#
# The drive's name there is "Kagaz", or "K" on a Windows that reads only
# its first letter: this runs from the drive (its folder is where it
# is), or from Kagaz share's copy, which names the folder (-Dir).
param([string]$Dir = $PSScriptRoot)
$ErrorActionPreference = 'Stop'
# What goes wrong is said there in a box that stays (it runs without a
# window).
trap {
  (New-Object -ComObject WScript.Shell).Popup("Kagaz's helper: $_", 0, 'Kagaz', 16) | Out-Null
  exit 1
}
$dir = $Dir
$drive = Split-Path -Parent $dir
Add-Type -AssemblyName Microsoft.VisualBasic
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.IO;
using System.Runtime.InteropServices;
using System.Windows.Forms;
public static class Kagaz {
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
  [StructLayout(LayoutKind.Sequential)] struct Spot { public int X; public int Y; }
  [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Spot point);
  [DllImport("user32.dll")] static extern bool GetCursorPos(out Spot point);
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
    Spot p; p.X = x; p.Y = y;
    return GetAncestor(WindowFromPoint(p), 2);
  }
  public static string Kind(IntPtr window) {
    var s = new System.Text.StringBuilder(256); GetClassName(window, s, 256); return s.ToString();
  }
  public static string Title(IntPtr window) {
    var s = new System.Text.StringBuilder(512); GetWindowText(window, s, 512); return s.ToString();
  }

  // Windows' own folder chooser.
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

  // A drag of files, held at the pointer: an all but invisible window
  // follows the pointer until Kagaz presses the button on it, and the
  // press starts Windows' own drag of the files from there. Kagaz then
  // moves and lets go, and wherever that is takes them as any drop
  // (File Explorer, the desktop, a program, a page's upload box).
  class Catcher : Form {
    readonly string[] paths;
    readonly string cancel;
    readonly DateTime until;
    readonly Timer follow = new Timer();
    int ticks;
    bool started;
    public int Effect;
    public Catcher(string[] paths, string cancel, int millis) {
      this.paths = paths;
      this.cancel = cancel;
      until = DateTime.Now.AddMilliseconds(millis);
      FormBorderStyle = FormBorderStyle.None;
      ShowInTaskbar = false;
      TopMost = true;
      StartPosition = FormStartPosition.Manual;
      BackColor = Color.Black;
      Opacity = 0.01;
      Size = new Size(96, 96);
      follow.Interval = 15;
      follow.Tick += (s, e) => Follow();
    }
    protected override bool ShowWithoutActivation { get { return true; } }
    protected override CreateParams CreateParams {
      get {
        var p = base.CreateParams;
        // A tool window that never takes the focus.
        p.ExStyle |= 0x80 | 0x08000000;
        return p;
      }
    }
    protected override void OnLoad(EventArgs e) {
      base.OnLoad(e);
      Follow();
      follow.Start();
    }
    void Follow() {
      if (started) return;
      if (DateTime.Now > until || (++ticks % 16 == 0 && File.Exists(cancel))) { Close(); return; }
      Spot c;
      if (GetCursorPos(out c)) Location = new Point(c.X - Width / 2, c.Y - Height / 2);
    }
    protected override void OnMouseDown(MouseEventArgs e) {
      if (e.Button != MouseButtons.Left || started) return;
      started = true;
      follow.Stop();
      Hide();
      var data = new DataObject(DataFormats.FileDrop, paths);
      // Copied, never moved off the drive.
      data.SetData("Preferred DropEffect", new MemoryStream(BitConverter.GetBytes(1)));
      Effect = (int)DoDragDrop(data, DragDropEffects.Copy);
      Close();
    }
  }
  public static int Drag(string[] paths, string cancel, int millis) {
    var c = new Catcher(paths, cancel, millis);
    Application.Run(c);
    return c.Effect;
  }

  [DllImport("user32.dll")] static extern short GetAsyncKeyState(int key);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
  // The (left, or primary) button held now.
  public static bool Held() {
    int key = GetSystemMetrics(23) != 0 ? 2 : 1;
    return (GetAsyncKeyState(key) & 0x8000) != 0;
  }
  public static int[] Pointer() {
    Spot c;
    GetCursorPos(out c);
    return new int[] { c.X, c.Y };
  }

  // The drop zone: a strip at the side of the screen the pointer is on,
  // shown while something is dragged, taking files dropped on it.
  public class Zone : Form {
    static readonly Color Dark = Color.FromArgb(32, 33, 36);
    static readonly Color Green = Color.FromArgb(46, 125, 80);
    readonly string words;
    string[] dropped;
    public Zone(string to) {
      words = "Drop files here\nto send them to\n" + to;
      FormBorderStyle = FormBorderStyle.None;
      ShowInTaskbar = false;
      TopMost = true;
      StartPosition = FormStartPosition.Manual;
      AllowDrop = true;
      BackColor = Dark;
      ForeColor = Color.White;
      Opacity = 0.94;
      DragEnter += (s, e) => {
        bool files = e.Data.GetDataPresent(DataFormats.FileDrop);
        e.Effect = files ? DragDropEffects.Copy : DragDropEffects.None;
        if (files) { BackColor = Green; Invalidate(); }
      };
      DragLeave += (s, e) => { BackColor = Dark; Invalidate(); };
      DragDrop += (s, e) => {
        dropped = e.Data.GetData(DataFormats.FileDrop) as string[];
        BackColor = Dark;
        Invalidate();
      };
    }
    protected override bool ShowWithoutActivation { get { return true; } }
    protected override CreateParams CreateParams {
      get {
        var p = base.CreateParams;
        p.ExStyle |= 0x80 | 0x08000000;
        return p;
      }
    }
    protected override void OnPaint(PaintEventArgs e) {
      base.OnPaint(e);
      using (var f = new Font("Segoe UI", 11f))
      using (var b = new SolidBrush(ForeColor)) {
        var at = new StringFormat { Alignment = StringAlignment.Center, LineAlignment = StringAlignment.Center };
        e.Graphics.DrawString(words, f, b, ClientRectangle, at);
      }
    }
    // At the right side of the screen the pointer is on, halfway down.
    public void Come() {
      if (Visible) return;
      Spot c;
      GetCursorPos(out c);
      var area = Screen.FromPoint(new Point(c.X, c.Y)).WorkingArea;
      float k = DeviceDpi / 96f;
      if (k <= 0) k = 1;
      Size = new Size((int)(200 * k), (int)(130 * k));
      Location = new Point(area.Right - Width - (int)(12 * k), area.Top + (area.Height - Height) / 2);
      BackColor = Dark;
      Show();
    }
    public void Go() { if (Visible) Hide(); }
    public string[] Take() { var d = dropped; dropped = null; return d; }
  }

  // A drag of files made here, caught at the pointer as it leaves
  // Kagaz's window there: an all but invisible window under it, which
  // Windows' drag enters (its files are known then) and is let go on.
  public class Net : Form {
    public string[] Caught;
    public string[] Dropped;
    public Net() {
      FormBorderStyle = FormBorderStyle.None;
      ShowInTaskbar = false;
      TopMost = true;
      StartPosition = FormStartPosition.Manual;
      BackColor = Color.Black;
      Opacity = 0.01;
      AllowDrop = true;
      Spot c;
      GetCursorPos(out c);
      Size = new Size(160, 160);
      Location = new Point(c.X - 80, c.Y - 80);
      DragEnter += (s, e) => {
        var files = e.Data.GetData(DataFormats.FileDrop) as string[];
        if (files != null && files.Length > 0) { Caught = files; e.Effect = DragDropEffects.Copy; }
        else e.Effect = DragDropEffects.None;
      };
      DragOver += (s, e) => { e.Effect = Caught != null ? DragDropEffects.Copy : DragDropEffects.None; };
      DragDrop += (s, e) => { Dropped = e.Data.GetData(DataFormats.FileDrop) as string[]; };
    }
    protected override bool ShowWithoutActivation { get { return true; } }
    protected override CreateParams CreateParams {
      get {
        var p = base.CreateParams;
        p.ExStyle |= 0x80 | 0x08000000;
        return p;
      }
    }
  }
}
'@
[Kagaz]::Aware()

$shell = New-Object -ComObject Shell.Application
function Test-Folder($path) { $path -and [System.IO.Path]::IsPathRooted($path) -and (Test-Path -LiteralPath $path -PathType Container) }
# An answer: a number, which Kagaz reads once it is whole.
function Write-Answer($name, $value) {
  Set-Content -LiteralPath (Join-Path $dir $name) -Value $value
}

# Where a batch let go at `$At` ("x,y") goes, as Windows does with a
# drop: into the folder under the point in File Explorer, else into the
# folder that window shows (and chosen there), or onto the desktop (into
# a folder on it, if let go over one). Otherwise, and where that cannot
# be told, Windows' own folder chooser asks. Then Windows' own copy, with
# its progress and its questions about files already there; a chosen
# folder opens with them.
function Save-Batch($src, $items, $At) {
  $start = $shell.NameSpace('shell:Downloads').Self.Path
  $to = $null
  $view = $null
  if ($At -match '^(-?\d+),(-?\d+)$') {
    try {
      $x = [int]$Matches[1]; $y = [int]$Matches[2]
      $window = [Kagaz]::Under($x, $y)
      $kind = [Kagaz]::Kind($window)
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
        $title = [Kagaz]::Title($window)
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
    $to = [Kagaz]::Pick("Save $what from Kagaz", $start)
    if (-not $to) { return 0 }
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
  return 1
}

# Files dropped on the zone: copied to the drive by Windows (its own
# progress), then said to be there.
function Send-Dropped($paths) {
  $id = [DateTime]::UtcNow.Ticks
  Copy-Out $paths (Join-Path $dir "in-$id")
  Write-Answer "in-$id.txt" $paths.Count
}

# Files copied by Windows into a folder of the drive, then said to be there.
function Copy-Out($paths, $to) {
  New-Item -ItemType Directory -Path $to -Force | Out-Null
  $ui = [Microsoft.VisualBasic.FileIO.UIOption]::AllDialogs
  $no = [Microsoft.VisualBasic.FileIO.UICancelOption]::DoNothing
  foreach ($p in $paths) {
    $name = Split-Path -Leaf $p
    if (Test-Path -LiteralPath $p -PathType Container) {
      [Microsoft.VisualBasic.FileIO.FileSystem]::CopyDirectory($p, (Join-Path $to $name), $ui, $no)
    } else {
      [Microsoft.VisualBasic.FileIO.FileSystem]::CopyFile($p, (Join-Path $to $name), $ui, $no)
    }
  }
}

$here = 'Kagaz'
try { $here = (Get-Content -LiteralPath (Join-Path $dir 'name.txt') -TotalCount 1).Trim() } catch {}
# A drag being caught (see "catch"): its id, its window, until when.
$catching = $null
$zone = New-Object 'Kagaz+Zone' -ArgumentList $here
$pressed = $null
$beat = [DateTime]::MinValue
$looked = [DateTime]::MinValue
while ($true) {
  # The zone's window and drops are Windows messages: taken here.
  [System.Windows.Forms.Application]::DoEvents()
  # Something dragged: the left button held a moment and moved.
  if ([Kagaz]::Held()) {
    $now = [Kagaz]::Pointer()
    if (-not $pressed) { $pressed = @{ At = Get-Date; X = $now[0]; Y = $now[1] } }
    elseif (((Get-Date) - $pressed.At).TotalMilliseconds -gt 300 -and ([Math]::Abs($now[0] - $pressed.X) + [Math]::Abs($now[1] - $pressed.Y)) -gt 24) { $zone.Come() }
  } else {
    $pressed = $null
    $zone.Go()
  }
  $paths = $zone.Take()
  if ($paths) {
    try { Send-Dropped $paths } catch {}
  }
  # A drag caught as it left Kagaz's window: said once its files are
  # known; let go on, copied to the drive; else, after a while or when
  # Kagaz says, given up.
  if ($catching) {
    $c = $catching
    try {
      if ($c.Net.Caught -and -not $c.Told) {
        Write-Answer "caught-$($c.Id).txt" $c.Net.Caught.Count
        $c.Told = $true
      }
      if ($c.Net.Dropped) {
        $paths = $c.Net.Dropped
        $c.Net.Close()
        $catching = $null
        Copy-Out $paths (Join-Path $dir "out-$($c.Id)")
        Write-Answer "out-$($c.Id).txt" $paths.Count
      } elseif ((Get-Date) -gt $c.Until -or (Test-Path -LiteralPath (Join-Path $dir "cancel-$($c.Id).txt"))) {
        $c.Net.Close()
        $catching = $null
        Write-Answer "done-$($c.Id).txt" 0
      }
    } catch { $catching = $null }
  }
  if (((Get-Date) - $looked).TotalMilliseconds -lt 200) {
    Start-Sleep -Milliseconds 30
    continue
  }
  $looked = Get-Date
  if (-not (Test-Path -LiteralPath $dir)) {
    # The drive is gone: the session ended, or Kagaz there was closed.
    break
  }
  $asks = @()
  try {
    if (((Get-Date) - $beat).TotalSeconds -ge 2) {
      Write-Answer 'helper-alive.txt' $PID
      $beat = Get-Date
    }
    $asks = @(Get-ChildItem -LiteralPath $dir -Filter 'ask-*.txt' | Sort-Object LastWriteTime)
  } catch {}
  foreach ($a in $asks) {
    $id = $a.BaseName.Substring(4)
    $done = 0
    try {
      $lines = @(Get-Content -LiteralPath $a.FullName)
      Remove-Item -LiteralPath $a.FullName -Force
      $what = $lines[0]; $batch = $lines[1]; $at = if ($lines.Count -gt 2) { $lines[2] } else { '' }
      if ($what -eq 'catch') {
        if ($catching) { $catching.Net.Close() }
        $net = New-Object 'Kagaz+Net'
        $net.Show()
        $catching = @{ Id = $id; Net = $net; Until = (Get-Date).AddSeconds(8); Told = $false }
        continue
      }
      $src = Join-Path $drive $batch
      $items = @(Get-ChildItem -LiteralPath $src -Force)
      if ($items.Count -gt 0) {
        if ($what -eq 'drag') {
          Write-Answer "ready-$id.txt" 1
          $done = [Kagaz]::Drag(@($items | ForEach-Object { $_.FullName }), (Join-Path $dir "cancel-$id.txt"), 30000)
        } elseif ($what -eq 'open') {
          Start-Process explorer.exe -ArgumentList "`"$src`""
          $done = 1
        } else {
          $done = Save-Batch $src $items $at
        }
      }
    } catch { $done = -1 }
    try {
      Write-Answer "done-$id.txt" $done
      Remove-Item -LiteralPath (Join-Path $dir "cancel-$id.txt") -Force -ErrorAction SilentlyContinue
    } catch {}
  }
}
