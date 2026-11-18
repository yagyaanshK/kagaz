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
  so). Choosing a monitor was dropped on 2026-11-17: over RDP the
  computer makes one screen for the viewer. The picture comes by RDP's
  graphics pipeline, H.264 decoded by OpenH264 built in from source
  (chosen 2026-11-17), with the older picture as a per-computer switch.
  Its sound plays here through the system's own sound (2026-11-18),
  by Kagaz's own player, which keeps only as much ready as the way the
  sound arrives needs. This computer's own keys (Print Screen, the
  Windows key, Alt + Tab) go to the device in full screen only, whenever
  the pointer is on Kagaz, or never: the owner's choice per device
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

### 2. The viewer on Android and Windows

Touch where you tap (tap clicks there, long-press is right-click, two
fingers scroll, pinch zooms), with a switch to trackpad mode remembered
per device. The phone's fingerprint or PIN before each session. A frugal
start on mobile data, with the amount used shown. A saved device shared to
another Kagaz by QR code (its paths, never its passwords): a phone cannot
read Tailscale's list.

### 3. More existing protocols

Wake-on-LAN, an adb link to an Android phone over USB or Wi-Fi, and SFTP
file browsing.

### 4. Kagaz on both ends; the Linux host

- **Pairing.** A QR code or a short code, or a device found nearby; both
  show the same few words and each confirms. Each device has its own key.
- **Permissions per device.** View, control, files, terminal, unattended
  access; chosen at pairing, changed later.
- **A lost device.** Each computer has a PIN of its own, typed on the
  device that connects; a phone asks for its fingerprint or PIN before a
  session; any other paired device can remove the lost one. Kagaz does not
  keep a computer's sign-in password unless told to.
- **GNOME on Wayland.** Kagaz's host runs in the signed-in session. The
  login screen (after a reboot) goes through GNOME's own remote login, set
  up once with the administrator password after showing what changes.
  Nothing of Kagaz runs as root.
- **On the host.** A visible sign for the whole session naming the device
  connected, with a way to end it; the computer locks when a session ends
  if nobody was at it; an option to blank the real screen; a record of
  sessions kept on the computer.
- **Terminal.** A shell and running one command inside Kagaz's own
  session, as its own permission, off until granted.

### 5. The Windows host

Runs while signed in, without administrator rights and without a service.
The login screen and administrator prompts go through Windows' own Remote
Desktop, which Kagaz already speaks. The mouse-button setting is read and
corrected without a switch.

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
  only once the Windows host (step 5) exists.
- Without a service, Kagaz's own Windows session cannot show the login
  screen or administrator prompts; those go through RDP.
- If Google Play signs the app with its own key, moving between the Play
  build and the full build means uninstalling first; giving Play the app's
  existing key at enrolment avoids it.
