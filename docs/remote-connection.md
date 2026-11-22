# Computers & phones: remote connection

View and control your own devices from each other inside Kagaz: one screen
on every system, no account, and nothing passing through a third party's
servers. Plan agreed 2026-11-16; update this file as steps land.

## Why

Doing this by hand takes a different tool for every pair of devices
(an RDP client, a VNC client and server, scrcpy, a mesh VPN), each with its
own settings, scaling and quality. What it taught:

- **Reaching the device is the fragile part.** Some home Wi-Fi does not
  pass discovery between its 2.4 and 5 GHz bands, local addresses change,
  and a phone's hotspot hands out a random address. Kagaz tries every
  known path at once and uses the first that answers.
- **One behaviour everywhere.** Screen size, quality and input should not
  depend on which protocol happens to be underneath.
- **Say the limits.** Android asks for screen-capture consent at each
  start and no app can type into its lock screen; an iPhone can be viewed,
  never controlled.

## Shape

Three layers: **reaching** a device (saved paths, tried in parallel; later
pairing with keys and end-to-end encryption), the **session** (RDP and VNC
first; later Kagaz's own between two Kagaz devices), and per-system
**capture and input** on the device being controlled. All of it is native
code in `kagaz-core`, as every device class since the 2026-10-21 roadmap
note; the command line gets the same abilities at each step.

It is a kind of device on Home, **Computers & phones**, with the pattern of
the other kinds: saved devices, Found nearby, Add.

## Steps

### 0. Checks before building

- Try the RDP library (IronRDP, MIT or Apache-2.0) against a real Windows
  computer: resizing to the window, sound, clipboard, files, monitor
  choice. Anything missing is brought back as a choice, not dropped.
- Mockups (local pages under `design/mockups/`) of the device list, Add
  and the viewer in each system's look, approved before building.

### 1. First release: the viewer in the desktop window (Linux first)

- **Devices.** Saved devices, Found nearby, Add (found nearby, the
  Tailscale list read from this computer, or a typed address). A device
  has ways in (RDP, VNC, SSH) and paths.
- **Reaching.** Remembered addresses, the device's name, Tailscale and, on
  a hotspot, the gateway, all at once. If none answers, Kagaz looks over
  the network it is on for a remote-desktop port, recognises the device by
  its certificate or name and remembers the new address. Out of reach is
  explained in one line (home network or Tailscale; which is missing).
- **Viewer.** Over RDP the remote screen is redrawn at the window's size
  and scaling (a per-device switch keeps the remote's own size); VNC is
  scaled to fit, with zoom. One slider from Sharper to Smoother, automatic
  by default, with the measured delay shown. Clipboard text both ways;
  sound and sending files over RDP (VNC has neither, and Kagaz says
  so). Sending files (the owner, 2026-11-19, after the clipboard way did
  nothing on the real PC: "build all", plus a dialog on Windows' own
  Remote Desktop) goes one of four ways, a setting per device:
  Windows asks where to save them (default: the files are put in the
  session's own folder, shown there as the drive `\\tsclient\Kagaz`
  once something is sent, and Kagaz opens Windows' Run box and starts a
  small PowerShell helper from that drive: Windows' own folder chooser,
  then Windows' own copy with its progress, nothing installed); their
  folder opens there; on its clipboard to be pasted (the old way, now
  saying it waits, with each clipboard step in the log); or over SSH
  with scp into a folder there. A folder of this computer can also be
  shared with a device as a drive, both ways, from the next connection.
  Files dragged from this computer onto the session's window (the
  owner, 2026-11-20: "where you let go", then, after researching how
  TeamViewer, Splashtop and RustDesk do it, "all of the above"): a real
  Windows drop. Windows takes a drop only from a drag made on it, so,
  as those tools do with their own program there, Kagaz's helper there
  (helper.ps1 on the Kagaz drive, PowerShell with a little C#) holds
  one: on the drag's entering, the files go on the drive and the
  helper is asked (files on the drive: ask, ready, done, cancel); it
  puts an all but invisible window under the pointer, Kagaz presses the
  button there, the press starts Windows' own drag of the files
  (`\\tsclient\Kagaz\…` paths), the pointer moves with the drag here,
  and letting go is a drop like any: a folder, the desktop, a program,
  a page's upload box, with Windows' own highlight while over them.
  Leaving the window presses Escape there. The helper runs for the
  whole session (it ends when the drive goes) and also does Send
  files' saving; it starts from Windows' Run box once a session (the
  line pasted there, never typed: the Run box's completion lost typed
  letters; and run by the person's Enter, never pressed blind: the
  Windows key changes the taskbar at once, which passed for the Run box
  on a real PC, and keys pressed before it came went to File Explorer,
  where Enter opens and Delete deletes), or, where the
  person turned on "Files from Kagaz over Remote Desktop" in Kagaz's
  Share page on that computer, `kagaz-share --drops` (started at
  sign-in, idle outside Remote Desktop sessions) starts it from its
  own copy when the drive appears, so no Run box is seen. Let go before
  the helper holds the drag (the first drop of a session, while it
  starts), the files are saved where they were let go: into the folder
  under the point in File Explorer, else the folder that window shows,
  or the desktop; elsewhere Windows' folder chooser asks. A device set
  to send files by its clipboard gets dropped files there, to paste.
  The other way (the owner, 2026-11-20, "all of the above"): files from
  a Windows computer land where a setting per device says ("Files from
  it": ask where each time, the default; Downloads; a folder chosen
  once), by its drop zone (the helper shows a strip at the side of its
  screen while something is dragged there), by copy there and paste
  here (fetched over Windows' clipboard channel when the session's
  window loses the focus, piece by piece to disk, then put on this
  computer's clipboard as copied files), or by a real drag out of the
  window (the helper catches Windows' drag at the screen's edge, Kagaz
  lets go there and carries the drag on here with this computer's own,
  handing the files over when they have all come). The Kagaz drive is
  announced at sign-in in every session for this, its name as plain
  text (IronRDP's UTF-16 name showed as its first letter on Windows 11).
  A computer running Kagaz takes the files at once into its person's
  Downloads, beside what is there, with "N files in Downloads" on its
  sign to show them.
  Lag, measured: the time from a click, key or wheel to the next change
  shown is logged every five seconds (middle and slowest) and shown in
  the Picture panel. The owner's PC sent no H.264 at all (its processor
  drew and encoded the screen with Windows' text and progressive
  codecs), so the Picture panel offers "Use it there…": Kagaz starts a
  helper from its drive that turns on Windows' own settings for the
  graphics chip (hardware graphics adapters for Remote Desktop, H.264
  hardware encoding, H.264 4:4:4 preferred, 60 pictures a second instead
  of 30) after Windows' administrator question there, keeping what was
  there before to put back (2026-11-19). On Linux the pictures reach the
  window through a WebSocket on this computer (about 15 ms for a whole
  picture against 25 to 30 through the window's channel). Choosing a monitor was dropped on 2026-11-17: over RDP the
  computer makes one screen for the viewer. The picture comes by RDP's
  graphics pipeline, H.264 decoded by OpenH264 built in from source
  (chosen 2026-11-17), with the older picture as a per-computer switch.
  Its sound plays here through the system's own sound (2026-11-18),
  by Kagaz's own player, which keeps only as much ready as the way the
  sound arrives needs. This computer's own keys (Print Screen, the
  Windows key, Alt + Tab) go to the device in full screen only, whenever
  its window is active with the pointer inside, or never: the owner's choice per device
  (2026-11-18), with Ctrl + Esc to leave full screen and a question
  before Kagaz closes with a session open. A per-computer switch keeps
  left click left where the remote computer has its mouse buttons
  swapped.
- **Trust.** A computer's RDP certificate is remembered at the first
  connection and a change is warned about. VNC outside Tailscale shows
  "Not encrypted on this network". Passwords go in the system's password
  store behind a Remember switch.
- **Command line.** List, add, rename and remove devices; show which paths
  work now and how fast; send clipboard text and files; open the viewer;
  open the system's `ssh` (and copy files with it) over the working path
  for devices that run an SSH server.
- **Done when** it works to a Windows computer over RDP, an Android phone
  running a VNC server (Tailscale, home Wi-Fi, the phone's hotspot), a
  GNOME computer through GNOME's own RDP, and any typed address.

### 2. Kagaz on both ends; the Windows host

Moved ahead of the phone's viewer on 2026-11-18 (the owner): a computer
that someone is sitting at, seen and used from afar *while they go on
using it*, for showing a team how something is done. Windows' own Remote
Desktop cannot: it gives the screen to one person at a time. The usual
answer sends the screen through its maker's servers; Kagaz's goes from
one device to the other, over the local network or Tailscale.

- **Pairing.** A QR code or a short code, or a device found nearby; both
  show the same few words and each confirms. Each device has its own key.
- **Permissions per device.** View, control, files, terminal, unattended
  access; chosen at pairing, changed later.
- **A lost device.** Each computer has a PIN of its own, typed on the
  device that connects; a phone asks for its fingerprint or PIN before a
  session; any other paired device can remove the lost one. Kagaz does not
  keep a computer's sign-in password unless told to.
- **On the host.** A visible sign for the whole session naming the device
  connected, with a way to end it; the computer locks when a session ends
  if nobody was at it; an option to blank the real screen; a record of
  sessions kept on the computer.
- **Terminal.** A shell and running one command inside Kagaz's own
  session, as its own permission, off until granted.

**On Windows** (the owner's answers, 2026-11-18: every choice a setting).
Kagaz shares the screen of the person signed in, who goes on using it; the
device connecting shows it in a session window as any computer today. The
host speaks Remote Desktop (agreed 2026-11-18), on a port of its own, so
today's viewer works with it unchanged.
- *Off until turned on* (the owner: one use must not cloud another). Kagaz
  is installed for many reasons; nothing of the host runs, listens or sits
  in the tray until someone chooses "Share this computer" (Add a device ›
  Computers & phones, or This computer in its settings). Turning it off
  stops all of it.
- *Several devices at once* (a setting, yes by default); the sign on the
  computer names each and whether it watches or controls.
- *Port and networks* (the owner, 2026-11-18: how it listens is the
  user's choice): Kagaz's own port, 3390 unless changed, or any other,
  or Remote Desktop's usual 3389 when Windows' own is off; and where it
  listens: the local network and Tailscale, Tailscale only, or this
  computer only.
- *Its own program,* `kagaz-share` (`crates/kagaz-share`), started by
  Kagaz only while sharing is on. The published server part of the
  Remote Desktop library cannot share a build with the newer client parts
  the viewer uses; when a matching one is published it can move into
  kagaz-core. Built in order: (1) the screen, mouse and keys, started by
  hand (done 2026-11-18, tried here with the test card: Full HD at 30
  pictures a second through Kagaz's viewer); (2) sound, clipboard,
  files (done 2026-11-19: the speakers' sound as it plays, which goes on
  playing here; text and pictures both ways; files sent from the viewer
  land in a folder of their own under the temporary folder and on the
  clipboard, to paste anywhere; files copied on the computer wait for
  the viewer to take files); (3) pairing, the PIN, the question (done
  2026-11-19: pairing over the same port, protected by SPAKE2 with the
  code, three words compared on both; the device's secret made on both
  sides and kept here apart from the device list; sign-in inside TLS
  with the certificate pinned at pairing; the right PIN lets a device
  allowed to connect without asking straight in, otherwise the person
  there is asked (control, watch, or no; no answer in 30 s is no); five
  wrong PINs stop PINs for five minutes; what each device may do is
  enforced); (4) the screens (done 2026-11-19: Kagaz runs kagaz-share
  only while sharing is on, and it stops with Kagaz; This computer under
  Computers & phones, reached from Add a device, the gear and a row on
  the list while on: the switch, ask or PIN for new devices, the PIN,
  paired devices and what each may do, pairing with the code and this
  computer's addresses, locking afterwards, the record of sessions, port
  and networks; the question and the sign as small windows on top of
  everything; Control and Watch in the viewer's bar; still to come: the
  QR code, blanking the screen, several devices at once, the viewer's bar
  following a switch to watching made on the computer); (5) start
  with Windows, the tray, the helper service (built 2026-11-19, not yet
  run on Windows: while sharing is on, Kagaz waits in the tray when its
  window closes, with Open Kagaz, Stop sharing and Quit; "Start with
  Windows" puts it there at sign-in; the helper service, named
  KagazHelper ("Kagaz helper") with its folder in
  `C:\ProgramData\Kagaz`, runs kagaz-share as the system at the
  computer's own screen, following Windows to the sign-in screen and
  administrator prompts and on after a restart before anyone signs in;
  Kagaz steers it over a token-protected connection on the computer;
  with nobody there to ask, only devices allowed to connect with the
  PIN without asking get in).
- *Connecting:* "Ask here each time" (someone at the computer allows it,
  with control or watch only; no answer in 30 s is no) or "Connect with the
  PIN, without asking"; the computer's choice is the default, each paired
  device can have its own.
- *Starting:* runs while Kagaz is open; "Start Kagaz when I sign in to
  Windows" (to the tray) is a setting, off by default.
- *Sign-in screen and administrator prompts:* a setting with three ways:
  someone at the computer answers them; Windows' own Remote Desktop, which
  Kagaz already speaks (Windows Pro); or Kagaz's helper service, installed
  only when chosen, with an administrator's password, and removed when the
  setting goes back. Without it nothing of Kagaz runs with raised rights.
- *Watch or control* from either end: Control and Watch in the viewer's
  bar; Watch only and End on the computer's sign; watch-only as a
  permission per device.
- Mockups: `design/mockups/windows-host/`.

The mouse-button setting is read and corrected without a switch.

### 3. The viewer on Android and Windows

Touch where you tap (tap clicks there, long-press is right-click, two
fingers scroll, pinch zooms), with a switch to trackpad mode remembered
per device. The phone's fingerprint or PIN before each session. A frugal
start on mobile data, with the amount used shown. A saved device shared to
another Kagaz by QR code (its paths, never its passwords): a phone cannot
read Tailscale's list.

Built 2026-11-22, with Remote Desktop sessions on Android (sound through
AAudio on Android 8.0 and later, none on Android 7): tap, long-press for
right-click, one finger drags, two fingers scroll, pinch zooms the picture
here; a Keyboard button opens the phone's own keyboard (typed letters go as
characters, Backspace and Enter as keys; the picture keeps its size under
it). Still to come: the fingerprint or PIN, the frugal start on mobile
data, sharing a device by QR code.

Later the same day, at the owner's word, the pointer comes first on a phone:
the finger moves a pointer drawn over the picture, as on a touchpad (a tap
clicks where it is, a tap and then a drag drags, a long press or a
two-finger tap right-clicks, two fingers scroll there). A Pointer / Touch
button under the screen switches to touching directly, remembered for each
device. Minus and plus in the bar have the device draw its screen for a
window larger or smaller (50% to 200%, remembered for each device), as
resizing the window does on a computer.

### 4. More existing protocols

Wake-on-LAN, an adb link to an Android phone over USB or Wi-Fi, and SFTP
file browsing.

### 5. The Linux host

What step 2 built for Windows, on Linux.

- **GNOME on Wayland.** Kagaz's host runs in the signed-in session. The
  login screen (after a reboot) goes through GNOME's own remote login, set
  up once with the administrator password after showing what changes.
  Nothing of Kagaz runs as root.

### 6. The Android host

Screen sharing with Android's consent each time; control of the phone and
unattended starts are what Google Play restricts, so the split between a
Play build and a full build distributed directly is decided here against
Play's policy text (one app either way: a phone has one build or the
other). Planned with it: the remote camera (Kagaz driving the phone's
camera with the screen off) and a Bluetooth shutter, from a computer to a
phone and from a phone to a phone.

### 7. Beyond Tailscale

A relay that sees only encrypted bytes and can be self-hosted. The iPhone
as a viewer.

## Rules

- **Libraries.** Permissive licences only (MIT, Apache-2.0, BSD); nothing
  GPL, LGPL or AGPL. Their licence texts ship in About under "Third-party
  licences".
- **Testing.** Automated tests run against servers that show a generated
  test pattern. On a person's real devices the person watches the window;
  logs carry numbers (sizes, frame counts, delay), never pictures, and no
  screenshots are taken.

## Limits

- Windows Home has no Remote Desktop server: such a computer is reachable
  only once the Windows host (step 2) exists.
- Without the helper service, Kagaz's own Windows session cannot show the
  login screen or administrator prompts; those go through RDP or the person
  at the computer.
- If Google Play signs the app with its own key, moving between the Play
  build and the full build means uninstalling first; giving Play the app's
  existing key at enrolment avoids it.
