# TVs & displays

Find the TVs on the home network, say plainly why one cannot be found,
send a photo, video, song or document to one, use it as a remote, show
this computer's screen on it, and use it as a second monitor. Planned
2026-11-18. Update this file as steps land.

## Why

The roadmap note of 2026-10-21 put TVs next, LG webOS first. The pains are
a remote on the laptop or phone, and getting the screen or a file onto
the TV. Today that takes the TV maker's phone app, a Chromecast-capable
browser, an Apple device for AirPlay, or Windows' own Connect, and each
works for only some of it.

What a look at the test network showed on 2026-11-18 (by listening and
opening ports only: nothing was sent to the TV and nothing appeared on
it):

- The 2024 LG webOS TV announces itself over mDNS four ways: Google Cast
  (`_googlecast._tcp`, port 8009), AirPlay 2 (`_airplay._tcp`, port 7000),
  **Miracast over the ordinary network** (`_display._tcp`, port 7250,
  Microsoft's [MS-MICE] protocol), and LG's own webOS control (ports 3000
  and 3001, found by probing; not announced). It answers no SSDP search,
  so DLNA discovery finds nothing on it.
- The TV answered while this laptop was on the 2.4 GHz band, so the
  router's band split did not hide it this time. It still can: unicast
  crosses the bands, but some routers drop multicast (and so mDNS) between
  them.
- This laptop's Wi-Fi (Realtek RTL8852CE, rtw89) lists P2P-client and
  P2P-GO, may run one P2P link beside the home connection on a second
  channel, and NetworkManager shows a Wi-Fi P2P device for it.

Then, with the owner's go-ahead, both Miracast routes were tried on the
LG the same day, as far as asking the TV what it can play; no picture
was sent (see step 0 for the details):

- **Over the network (MS-MICE):** the TV took Kagaz's Source Ready,
  showed a notice naming the sender, connected back for RTSP and answered.
- **Over Wi-Fi Direct:** NetworkManager found the TV in about 25 seconds
  of searching and joined its group in about 20 seconds, the TV as group
  owner on 5 GHz while the home connection stayed up on 2.4 GHz. The TV
  then connected for RTSP and gave the same answer. No root, no password
  prompt.

**A TV on no network at all** (never set up on Wi-Fi, or with no
internet and no router nearby) must still take this computer's screen:
the owner's requirement of 2026-11-18. The only standard way to such a
TV is Miracast over Wi-Fi Direct: the computer and the TV make a link of
their own, with no router, after the TV's own "Screen Share" (or "Screen
Mirroring") mode is switched on. So mirroring has two main routes, not a
main one and a fallback:

- **Wi-Fi Direct**, which works whether the TV is on a network or not.
  It rests on this computer's Wi-Fi card and its driver (Realtek's P2P
  on Linux is known to be patchy), so step 0 tries it first.
- **Miracast over the ordinary network** (MS-MICE), for a TV on the same
  network that offers it: no P2P link, no channel juggling, and the home
  connection is untouched.

Kagaz tries the network route when the TV is on the network and Wi-Fi
Direct otherwise, and lets the owner choose. Google Cast, DLNA, AirPlay
and the TV makers' remote-control protocols all need the TV on a network;
on a TV that is not, Kagaz offers mirroring only and says why.

**AirPlay on every system** (the owner's requirement of 2026-11-18: Kagaz
is meant for every platform, and many TVs now take AirPlay). On a Mac
and an iPhone, Kagaz uses the system's own AirPlay, which Apple supports
for apps. On Linux, Windows and Android, Kagaz speaks AirPlay itself:
pairing with the PIN the TV shows, then sound, photos and video by
address. Mirroring the screen over AirPlay from those systems needs
Apple's FairPlay handshake, whose sender side is not published, and no
open implementation of it was found; it is tried, and if it cannot be
done Kagaz says so and offers Miracast or Cast to the same TV instead.

**Every way, in one place** (the owner, 2026-11-18): Kagaz offers every
way a device supports to show or send to a TV, its own and the system's,
so connecting is the same simple step whatever the devices: Kagaz's
Miracast (network and Wi-Fi Direct), Cast, AirPlay, and the system's own
senders (Windows' Connect, a Mac's or iPhone's AirPlay, Android's own Cast
or Screen Cast, opened from Kagaz with a line on when each is the better
choice).

## Shape

A kind of device on Home, **TVs & displays**, with the pattern of the
other kinds: saved TVs (a name and a room), Found nearby, Add. All of it
is native code in `kagaz-core` (`crates/kagaz-core/src/tv/`), and the
command line gets the same abilities at each step.

Four layers:

- **Finding.** mDNS and SSDP through `discovery`, plus a probe of known
  control ports, and Wi-Fi Direct device discovery for TVs in Screen
  Share mode, which finds TVs that are on no network. Each TV is one
  entry whatever number of ways it answers in.
- **Ways in**, each a module under `tv/`: `cast` (Google Cast), `dlna`
  (UPnP MediaRenderer), `webos` (LG), later `androidtv`, `samsung`,
  `roku`; `miracast` (over the network and over Wi-Fi Direct). A TV lists
  which ways it has and what each can do: send, control, mirror.
- **Picture out.** Screen capture per system, H.264 encoding, and the
  streams the ways in carry (MPEG-TS over RTP for Miracast, Cast
  Streaming for Google Cast).
- **Monitors.** A virtual monitor per system, captured as in the layer
  above.

**Screen capture, kept in `tv/` for now** (decided 2026-11-18). The
remote-connection plan ([remote-connection.md](remote-connection.md),
steps 4 to 6) also needs capture on the computer being controlled, and
nothing of it is written yet (`remote/screen.rs` is the viewer's side, the
picture that arrives). TVs get their own capture, virtual monitors and
encoder under `tv/capture/`, written so they can later move to a shared
module; remote connection's files are not touched.

**Command line**, in the style of `kagaz fans` and `kagaz fan NAME`
(decided 2026-11-18):

```
kagaz tvs                      saved TVs, and which are reachable now
kagaz tvs --nearby             TVs found nearby, Wi-Fi Direct included
kagaz tvs explain [NAME]       why a TV cannot be reached
kagaz tv NAME mirror           show this computer's screen on it
kagaz tv NAME extend           use it as a second monitor
kagaz tv NAME send FILE...     photos, video, audio, documents
kagaz tv NAME pair             pair through the TV's own prompt
kagaz tv NAME key KEY          up, down, left, right, ok, back, home ...
kagaz tv NAME text TEXT        type into the TV's text field
kagaz tv NAME volume N|up|down|mute
kagaz tv NAME input NAME
kagaz tv NAME power on|off
```

**Order** (decided 2026-11-18): mirroring first, since it is the hardest
and the most wanted, and the only thing a TV on no network can do; then
sending files, the remote, and extending.

**Test TVs:** the 2024 LG webOS TV on the network; the same TV with its
Wi-Fi forgotten; and another TV that is on no network (brand and model
still to be given).

## Steps

### 0. Checks before building

- **The protocols and libraries.** Permissive licences only (MIT,
  Apache-2.0, BSD), the rule of [remote-connection.md](remote-connection.md);
  GPL code is neither used nor copied, and published specifications are the
  reference.

  | Way | What it carries | Specification | Library to try (licence) |
  |---|---|---|---|
  | Google Cast | media by URL; mirroring through Cast Streaming | none published; the protocol is CastV2 (protobuf over TLS) | `rust_cast` (MIT) or Kagaz's own over `prost` (Apache-2.0); Chromium's Open Screen `libcast` (BSD-3, C++) for Cast Streaming |
  | DLNA / UPnP | media by URL | UPnP AV MediaRenderer, AVTransport (published) | Kagaz's own: SSDP plus a few SOAP calls |
  | DIAL | starting a TV app | DIAL 2.x (published) | Kagaz's own |
  | AirPlay 2 | sound; media by URL; mirroring | not published (community-documented); pairing follows HomeKit's pair-setup (SRP, Ed25519, ChaCha20); mirroring needs Apple's FairPlay | Mac and iPhone: the system's AirPlay (AVKit). Elsewhere: Kagaz's own client for pairing, sound and media; mirroring only if the FairPlay step can be done, else said plainly |
  | Miracast over the network | mirroring | [MS-MICE] (Microsoft Open Specifications) plus the Wi-Fi Display spec (Wi-Fi Alliance, free download) | Kagaz's own: RTSP, MPEG-TS, RTP |
  | Miracast over Wi-Fi Direct | mirroring | Wi-Fi Display, Wi-Fi P2P | Kagaz's own over wpa_supplicant's or NetworkManager's D-Bus P2P API |
  | LG webOS | remote; launching apps; media by URL | none published (community-documented) | Kagaz's own WebSocket client (`tungstenite`, MIT/Apache-2.0) |
  | Android TV / Google TV Remote v2 | remote; text | none published (community-documented) | Kagaz's own (protobuf over TLS, certificate pairing) |
  | Samsung Tizen | remote | none published (community-documented) | Kagaz's own WebSocket client, token pairing |
  | Roku ECP | remote; launching apps | published by Roku | Kagaz's own (HTTP) |
  | Fire TV | remote | ADB | Kagaz's own, with the adb link of remote connection step 3 |

  GNOME Network Displays (GPL-3) is not a reference for code. Each
  library's licence is checked before it is added, and its text ships in
  About under "Third-party licences".
- **Wi-Fi Direct on this laptop, first**, since it is the only way to a
  TV on no network. *Done 2026-11-18 with the TV on the home network and
  this laptop on 2.4 GHz:* NetworkManager's D-Bus API, as the signed-in
  user (no root, no password prompt), found the TV as a Wi-Fi Direct peer
  (`StartFind`; its Wi-Fi Display information says dual role, session
  available, RTSP port 7236), and a `wifi-p2p` connection carrying
  Kagaz's Wi-Fi Display information (source, session available, port
  7236) joined its group in about 20 seconds. The TV was group owner on a
  5 GHz channel, handing out an address in 192.168.49.0/24; the laptop's
  home connection stayed up on 2.4 GHz throughout and after. The TV then
  connected to port 7236 here. NetworkManager refuses an interface name
  on such a connection (longer than 15 characters): leave it out.
  *Still to try:* the TV on no network (its Wi-Fi forgotten), this laptop
  on 5 GHz, and the Windows PC's Wi-Fi Direct API.
- **Miracast over the network.** *Done 2026-11-18:* a Source Ready
  message to port 7250 (friendly name, RTSP port, a random source ID)
  made the TV show a notice naming the sender and connect back to port
  7236 within a second; Stop Projection ended it. No PIN or DTLS was
  asked for.
- **What the TV can play**, the same over both routes (its RTSP
  `GET_PARAMETER` answer is kept in
  `crates/kagaz-core/tests/fixtures/tv/lg-webos-2024-wfd-m3.txt`):
  - Video: H.264 Constrained Baseline and Constrained High, level 4.2,
    at 1080p and below (the exact sizes and rates in its CEA, VESA and
    handheld bitmaps, and the longer Wi-Fi Display R2 list, to be decoded
    in step 2).
  - Sound: LPCM 44.1 and 48 kHz stereo, AAC 48 kHz stereo.
  - HDCP 2.1 offered, not demanded: whether it plays an unprotected
    stream is the first thing step 2 checks.
  - UIBC: the TV can send a mouse and keyboard back (generic and USB
    HID), so a keyboard or mouse plugged into the TV could work this
    computer while mirroring.
  - It answers OPTIONS with `User-Agent: LGE` and RTCP is supported.
- **Still to try on the LG**, with the owner watching, at each first try:
  webOS pairing (the on-screen prompt); Cast's Default Media Receiver with
  a test picture; whether its Cast receiver accepts a mirroring session
  from a sender that is not Chrome; AirPlay pairing with the TV's PIN,
  then sound and a test picture; whether Wake-on-LAN works over Wi-Fi
  with the TV's "Turn on via Wi-Fi" setting.
- **Mockups** (local pages under `design/mockups/`) of the TVs list, Add,
  a TV's screen (remote, send, mirror) in each system's look, approved
  before building.

### 1. Find and explain

- Discovery adds the services above to the existing mDNS and SSDP code,
  and a probe of known control ports (webOS 3000/3001, Cast 8009, Roku
  8060, Samsung 8001/8002, MS-MICE 7250) on addresses already seen.
- **Explain** says in one or two lines why a TV is not reachable, from
  what was tried: the TV asleep or off (its address does not answer;
  webOS keeps answering mDNS in standby only with "Turn on via Wi-Fi"),
  the other Wi-Fi band hiding discovery (a saved TV's address answers but
  mDNS does not), Wi-Fi client isolation (nothing on the network answers
  but the router), a different subnet, and for Miracast-only TVs, the TV's
  own "Screen Mirroring" mode not switched on.
- A TV on no network is still listed when Wi-Fi Direct sees it, marked
  "Not on a network: screen mirroring only". Found nearby says how to
  make such a TV visible: switch on its Screen Share mode.
- A saved TV whose address changed is found again by its Cast or webOS
  identity, as remote connection does with certificates.

### 2. Mirror the screen

*Progress, 2026-11-18.* The Miracast sender is in `tv/miracast/`
(MS-MICE, the Wi-Fi Display RTSP setup, H.264 by OpenH264, MPEG-TS over
RTP) with the Wi-Fi Direct link in `tv/p2p.rs`, and on the command line
as `kagaz tvs [--direct]` and `kagaz tv NAME mirror --pattern [--via
network|direct] [--size 720|1080] [--seconds N]`. It sends the test
pattern only; this computer's screen is next. A test plays a whole
session against a stand-in TV that answers as the LG did and decodes what
arrives. On the LG, with the owner watching, the pattern played for 30
seconds over each route at 720p, 30 pictures a second: the bars, the
moving square and the clock showed, the square with a little stutter.
The TV shows a notice that connects by itself after a moment, with only
a Cancel button. The first Wi-Fi Direct link right after a network
session timed out (NetworkManager gave up after 45 seconds); the second
came up in 2 seconds, so Kagaz now tries twice. Since then Kagaz picks
60 pictures a second when the TV takes it (the LG takes 720p at 60 and
1080p at 30, not 1080p at 60). At 60 the owner saw the square move
smoothly but in waves: slower after crossing each colour bar, then
faster. The stream as sent is steady (a test decodes what arrives: every
picture there, the square 4.96 pixels further each time), so the waves
come from the radio link or the TV's own playback timing; pictures are
now stamped at exactly even times rather than when each was drawn, to
give the TV no wobble of Kagaz's own to follow. With that, the owner saw
the square move smoothly (over the network, the TV's port 7250 open
again once it had restarted fully).

*This computer's screen on Linux* (2026-11-18): `tv/capture.rs` asks the
desktop's screen-sharing portal (its own dialog: a screen, or a window
with `--window`), takes the pictures from PipeWire (`pipewire` crate,
MIT; build headers as in the installations notes), and scales the newest
one to the TV's size with black bars where the shapes differ. The
command line mirrors the screen by default now; `--pattern` sends the
test pattern. Tested in a sealed headless GNOME Shell with its own
PipeWire (`crates/kagaz-core/tests/headless-screen.sh`): the test
desktop's screen arrives at its size, and a whole session to the
stand-in TV decodes every picture sent. Since then it has been used
through the real desktop's dialog onto the LG (owner watching); KDE and
Hyprland are not yet tried. PipeWire is opened only when capture starts
(`tv/pw.rs`, a small loader of Kagaz's own; checked against the 1.6.2
headers), so Kagaz starts on a computer without it.

*Faster and smoother, 2026-11-18.* Measured on the laptop at 1080p: the
processor (OpenH264) needed 17 to 24 ms a picture on a real desktop, up
to 41 during big changes, against 33 available, and it skipped pictures
to keep its bitrate, which hid animations; packets went out in bursts
the Wi-Fi link dropped. Now:

- **The graphics chip encodes** where it can (`miracast/vaapi.rs`,
  VA-API for AMD and Intel, libva opened only when used; the structures
  generated by bindgen from libva's MIT headers into `va_sys.rs`). AMD's
  driver leaves the stream headers out unless given them, so Kagaz writes
  the sequence, picture and slice headers itself (`miracast/h264.rs`),
  with no reordering and the colour description. On this laptop's AMD
  chip: 5.5 to 5.8 ms a 1080p picture, 51 to 53 dB on desktop-like
  content. OpenH264 remains the fallback.
- **Kagaz's power setting** (`power` in settings.toml: battery or
  performance; `kagaz tvs settings`): battery takes the chip's low-power
  encoder where it has one (Intel) and at most 30 pictures a second.
  `tv_encoder` chooses auto, hardware or software.
- **1080p at 60 on TVs whose basic list stops at 1080p30**, as the LG's
  does: Kagaz asks for the TV's Wi-Fi Display R2 list too and requests
  the mode in R2's form (the specification, v2.1, section 6.1.22; another
  open sender reports this works on an LG C2), falling back to the basic
  list if refused. Not yet tried on the LG.
- No picture is skipped by the encoder; each picture's packets are
  spread over its time slot; the send buffer holds a whole picture.

The owner saw 1080p at 60 through R2 on the LG, encoded on the chip:
smoother than every earlier run. The desktop still gave only about 35
pictures a second.

*All the desktop's pictures, without a copy (2026-11-18).* Why only ~35
of 60 arrived: when GNOME (mutter 50) hands pictures over in shared
memory, it records each one at an idle moment after drawing and stamps it
with that moment, and its "no faster than the screen" check then finds
about a third of them too early and drops them. Handed over in graphics
memory (DMA-BUF), each picture is stamped with the screen's own refresh
time, so none should be too early. Kagaz now asks for graphics memory,
laid out row after row, when the graphics chip encodes:

- `tv/pw.rs` offers DMA-BUF formats first (then shared memory), says
  which memory it takes once the format is settled, and passes each
  buffer on as a descriptor; `tv/capture.rs` keeps a duplicate of the
  newest.
- `miracast/vaapi.rs` brings that buffer into the chip's view (no copy),
  and the chip's picture processor scales it to the TV's size with black
  bars and turns it into BT.601 video-range NV12 for the encoder. Nothing
  passes through the processor.
- Only when the chip can take them (`Engine::wants_gpu_pictures` tries
  first). The processor reads graphics memory slowly (12 to 14 ms a
  picture with 270 ms stalls, against 7 to 8 from shared memory), so
  OpenH264 gets shared memory; if the chip refuses a buffer mid-session,
  Kagaz carries on through the processor and says so.

Measured: in a sealed test desktop with a square sliding on every frame
(60 drawn a second), 32 to 37 pictures arrived through shared memory and
46 to 49 through graphics memory. GNOME's debug log shows the rest
dropped as "too early" still, since a test desktop with no real screen
has wobbling refresh times. On the AMD chip, a 1920x1200 screen to 1080p
takes 2.2 to 2.6 ms a picture from desktop to encoded, and every picture
sent to the stand-in TV decodes. Colours match the processor's
conversion to within 1 (declaring the input as sRGB made the chip change
the tone curve too: mid-grey 132 instead of 126); smooth pictures match
the processor's scaling at 58 dB, and fine text at 31 dB, as the two
scalers blur 1-pixel lines differently. On the laptop's real screen
(numbers only, through the owner's dialog), 59 to 60 a second arrived
for most of 30 seconds, against about 35 before; on the LG, 1080p at 60
for a minute, every picture sent, about 3 ms each to make, and the owner
found it much smoother. When only the pointer moves, GNOME draws no new
frame and records the pointer at the moment of the mouse event, which
again trips its "too early" check: about 40 a second, a GNOME limit.

*NVIDIA on Linux* (2026-11-18): `miracast/nvenc.rs`, NVENC through
NVIDIA's own driver (libcuda and libnvidia-encode opened only when used;
the structures generated from NVIDIA's MIT header, API 12.0, driver 520
or newer). Tried after VA-API: same Constrained Baseline stream, no
reordering, Kagaz's own BT.601 conversion so colours match; battery takes
the fastest preset. **Not yet run on an NVIDIA chip** (there is none
here): the test skips without the driver.

*Windows* (2026-11-18, the owner: Kagaz's own sender first, Windows'
Cast beside it): `tv/capture_win.rs` takes the main screen through
Windows.Graphics.Capture (Windows 10 1903 or newer, pointer included;
Windows 11 leaves off the yellow frame), and `miracast/mf.rs` encodes
through Media Foundation: the graphics chip's encoder first (NVIDIA, AMD
or Intel; these work asynchronously and are driven by their events),
then Microsoft's own, then OpenH264; the same stream as on Linux.
Scaling and the NV12 conversion are shared (`tv/picture.rs`). Over the
network only so far (MS-MICE); a TV on no network is reached with
`kagaz tv NAME system`, which opens Windows' Cast panel (Win+K). Ctrl+C
ends a session properly there too. Windows' firewall may ask once to let
Kagaz take the TV's connection. **Built for Windows from Linux, not yet
run on Windows.** Still to do there: a window or a new monitor for the
TV, Wi-Fi Direct of Kagaz's own, and the screen straight from graphics
memory into the encoder.

`kagaz tv NAME system` opens the system's own sender on any system: the
Cast panel on Windows, GNOME Network Displays on Linux where installed,
and on a Mac it says where AirPlay's Screen Mirroring is.

*A Mac* (2026-11-18): `tv/capture_mac.rs` takes the main display through
ScreenCaptureKit (macOS 12.3 or newer, pointer included; macOS asks once
for Screen Recording permission), and `miracast/vt.rs` encodes through
VideoToolbox (the Mac's own hardware encoder, real-time, Constrained
Baseline, no reordering, low-latency rate control), turning its
length-prefixed output into the start-code form the TV takes, with the
headers before each whole picture. Over the network only (macOS has no
Wi-Fi Direct for apps); `kagaz tv NAME system` points to AirPlay's Screen
Mirroring. **Not built for macOS here** (this computer has no macOS SDK):
both files are type-checked for macOS in a small crate of their own with
objc2's bindings; the rest of Kagaz could not be checked for macOS.
All tests on a real TV use the LG for now (the owner, 2026-11-18). Saved TVs (names,
rooms) come with the window; until then `kagaz tvs` lists the TVs found
nearby.


- **Capture.** Linux: xdg-desktop-portal ScreenCast and PipeWire (through
  each desktop's own portal: GNOME, KDE, Hyprland), with the portal's own
  picker each time. Windows: Windows.Graphics.Capture. macOS:
  ScreenCaptureKit.
- **Encoding.** H.264 in hardware where present (VA-API on Linux, Media
  Foundation on Windows, VideoToolbox on macOS), else OpenH264, already
  built into Kagaz from source for Remote Desktop. Cisco's patent cover
  applies only to Cisco's own prebuilt OpenH264 binary, not to a build
  from source; this is a question to decide before shipping the encoder.
- **Ways out.** Miracast over Wi-Fi Direct, which works with a TV on no
  network, and Miracast over the network (MS-MICE) for a TV on the same
  network; both are built in this step, and both carry the same RTSP and
  MPEG-TS stream (both proven as far as the TV's answer in step 0). Cast
  Streaming after them, if step 0 finds the TV accepts it; AirPlay
  mirroring if its FairPlay step can be done. Kagaz sends no HDCP: a TV that demands it refuses, and
  Kagaz says so.
- **The home connection during Wi-Fi Direct.** Where the card can keep
  both (this laptop's says it can, on two channels), the home Wi-Fi stays
  up; where it cannot, Kagaz says before connecting that the internet
  will drop while mirroring.
- **The system's own sender** is offered beside Kagaz's, with a line on
  which is better: Windows' Connect (Win + K, Miracast) and AirPlay on a
  Mac or iPhone.
- **Delay** is measured end to end (a timestamp drawn on the test
  pattern, read back from a capture of the receiver's output) and shown:
  fine for slides, photos and video, not for games, if it is above about
  100 ms.

### 3. Send media

- Photos, video and audio, and documents rendered to pictures, through
  Cast's Default Media Receiver, AirPlay, DLNA's SetAVTransportURI, or
  webOS's media launch, whichever the TV has. On a Mac and an iPhone,
  AirPlay goes through the system's own AirPlay.
- Kagaz serves the file from a small HTTP server on this computer, for
  that TV's address only and only while sending, with correct content
  types and range requests.
- When the TV cannot play the format, Kagaz transcodes through the
  on-demand ffmpeg extra, and says it is doing so.
- A play, pause, seek and volume bar while it plays; next and previous
  for a set of photos.

### 4. Control

- Pairing through the TV's own prompt (webOS, Android TV, Samsung); the
  key goes in the system's password store, as the other kinds keep
  theirs.
- Power, volume and mute, input, the arrow and OK keys, Back and Home,
  typing text into the TV's search field, launching an app.
- Turning on by Wake-on-LAN where the TV allows it; where it does not,
  Kagaz says so.
- Keyboard on the computer (arrows, Enter, Escape, typing); touch-sized
  buttons on the phone.

### 5. Extend: the TV as a second monitor

- **GNOME:** the ScreenCast portal's virtual source (mutter makes a
  monitor that exists only for the stream). **KDE:** to check whether its
  portal offers the same. **Hyprland:** a headless output
  (`hyprctl output create headless`), captured through its portal.
- **Windows:** an IddCx indirect display driver (user mode). Written and
  tested here with test signing; shipping needs Microsoft's attestation
  signing with an EV certificate through Partner Center, under a publisher
  name the owner gives. No publisher, certificate or company name is
  chosen in this work.
- **macOS:** only the private `CGVirtualDisplay` API exists. Apple may
  break it in any release, and it rules out the Mac App Store; Kagaz says
  so where the switch is.

### 6. The Kagaz phone app (agreed 2026-11-18, after step 2's screen)

- **Mirror the phone.** Android's own consent prompt each time
  (MediaProjection), the phone's hardware H.264 encoder (MediaCodec), and
  the same Miracast session as the computer, over the network. Over Wi-Fi
  Direct an ordinary app may not set the Wi-Fi Display information that
  tells a TV it is a screen source (Android keeps that for system apps):
  tried on the LG, and said plainly if refused.
- **Android's own Cast or Screen Cast**, opened from Kagaz, for a TV with
  Google Cast built in.
- **Extend the phone onto the TV** (advanced): Android lets only the
  system put apps on a second screen, so this goes through USB debugging
  (adb, as screen-mirroring tools do): Kagaz on the computer makes a
  virtual screen on the phone, apps open there, and that screen goes to
  the TV. Samsung DeX and Android's desktop mode, where a phone has them,
  are offered too.

*Built 2026-11-18* (the owner: Windows, then the settings, the Mac and
the phone, no mockups):

- **The TVs page in the window** (`apps/desktop/ui/tvs.js`,
  `apps/desktop/src/tvs.rs`): TVs is a kind in Add a device; the TVs added
  (kept in `<config>/tvs.json`) and the ones found nearby; on each, every
  way this device has: on a computer, Show this screen (and on Linux a
  window, or the TV as a second screen) and the system's own sender; on a
  phone, Show this phone, Android's Cast or Samsung's Smart View, and
  Samsung DeX. A session's lines show on the page, with Stop. Finding a
  TV by name, then the network or Wi-Fi Direct, is shared with the
  command line (`tv/send.rs`).
- **The phone's screen** (`KagazTv.kt`, `KagazCastService.kt`,
  `KagazTvPlugin.kt`): Android's own prompt, then a foreground service
  (Android 14 hands over the grant only inside one, and shows its
  notification with Stop all along); once the TV has chosen a size, the
  phone's H.264 encoder takes a virtual display mirroring the screen
  (Android fits the phone's shape into the TV's with bars), Constrained
  Baseline, no B pictures, low latency, headers before each whole picture;
  each encoded picture goes to Kagaz's Rust (`miracast/external.rs`), which
  runs the same Miracast session as on a computer, over the network.
- **Smart View, Android's Cast and DeX** are opened, not driven: Kagaz
  opens the screen where the person picks the TV.

*The phone's screen, first run (2026-11-18, Galaxy S10, Android 12):*
the release build's shrinker had dropped the Kotlin methods only Rust
calls (`KagazTv.start` and the rest), so the session died at its first
picture with "Java exception was thrown"; a keep rule fixed it, and a Java
error now names itself. To the stand-in TV on the laptop: 1920x1080 on
the Exynos encoder, every picture decoded, none lost, about 10 a second
on a still screen (the encoder repeats one every 100 ms) and up to 53
while things moved. The encoder refused one of the low-delay wishes and
ran without them; which one is still to find. Not yet onto the LG: its
network mirroring port stayed closed after the first, unfinished try,
through a full power cycle (Cast and AirPlay still answered). Samsung's
Smart View screen cannot be opened by another app on this phone; Kagaz
opens Android's Cast screen instead, which lists the TV both as a
wireless display (Miracast over Wi-Fi Direct, Smart View's way) and as a
Google Cast receiver. DeX's own start screen (its quick-panel tile's) can
be opened by Kagaz; DeX found no eligible wireless display for the LG,
and needs Link to Windows turned off.

*A stand-in TV* (`kagaz tvs stand-in`, `miracast/stand_in.rs`, kept as a
feature at the owner's wish): this computer announces itself as a
screen-mirroring TV, takes a sender's session over the network as the LG
does, and reports what arrives in numbers only (no picture shown or
kept). Checked first with the laptop's own sender and the test pattern:
479 of 481 pictures decoded at 1080p60, none lost.

*Google Cast, Kagaz's own sender (2026-11-19).* The LG's network
mirroring service stayed off: it no longer announced `_display._tcp`,
port 7250 stayed closed through an unplugging and a restart watched from
the laptop (Cast, AirPlay and the remote's ports all came back), so
neither the phone nor the laptop reached it over the network. An Android
app cannot use Miracast over Wi-Fi Direct (the system keeps it for its
own Cast screen), which left Google Cast as the phone's one route of
Kagaz's own. Checked first on the LG: it starts Chrome's mirroring
receiver (app `0F5096E8`) for a sender Google has not certified, says it
plays H.264, HEVC, VP8, VP9, Opus and AAC on a 3840x2160 screen at 60,
and answers an offer of H.264 alone at 1080p60 (it chose H.264 over VP8
when offered both). H.264 means every encoder Kagaz has serves Cast too.

`tv/cast/`: `channel.rs` is the Cast channel (TLS to port 8009, each
message a length and a small protobuf record written by hand, JSON
inside; the TV's certificate is taken as shown, since Cast TVs make
their own, the LG's in the old version 1 form that certificate
libraries refuse); `stream.rs` is one Cast Streaming stream (each
picture encrypted with AES-128 in counter mode, the nonce from the
picture's number; cut into packets with Cast's header; kept until the
TV's report says it arrived; missing packets sent again; a sender
report every half second; a whole picture when the TV says it lost
one; pictures skipped before encoding when the TV falls behind);
`mod.rs` is the session (start the app, offer, answer, then the same
picture sources and encoders as Miracast, packets paced, the app
stopped at the end so the TV returns to what it showed). The TV shows
each picture 200 ms after its moment. The format was learned from
Chromium's Open Screen source (BSD); the code is Kagaz's own.

Tests: the cipher against NIST's counter-mode vector; 400 pictures
through a link losing every 13th packet, each arriving whole and
unchanged; a whole stream over loopback to a stand-in receiver
(`stream::Receiver`) losing every 20th packet, decoded by OpenH264.
On the LG from the laptop: the test pattern for 20 seconds at
1080p60, 1,179 pictures, the session held throughout; the owner saw
the pattern, moving smoothly.

From the phone (Galaxy S10) to the LG, the app's Mirror button: Cast
chosen by itself, 1920x1080 on the phone's Exynos encoder, 95 seconds
until the owner stopped it, about 10 pictures a second on a still
screen, the TV 16 ms away, 96 packets sent again, no whole picture
asked for. The phone's sound stays on the phone: sound is not sent yet.
On the page, a TV's card now says only "Connecting…" then "Mirroring"
(the running numbers are in the TV's sheet) and is drawn again only
when that changes: the numbers had re-wrapped the card every second.
Mirror is a filled button (the accent's own text colour on it); the
accent on its dark container had read as switched off.

*The phone's sound, hiding, and the switches (2026-11-19).* The
owner's decisions: sound goes to the TV; a quick switch for the
sound's place (the TV or the phone); a privacy switch that hides the
screen on the TV behind frosted glass; a third that hides everything
at once; **every session starts hidden** (the TV gets the frosted
picture and no sound until the person shows it); the switches in the
TV's card in Kagaz, the mirroring notification, the quick panel's
tiles and a floating button. The volume keys stay the phone's own and
the TV's volume its remote's, as with Android's own Cast (turning the
TV's volume from the keys was built, tried and taken out the same
night at the owner's word). Phone first, computers after.

Seen on the LG by the owner: the hiding, the chips, the notification
and the floating button work; Instagram's sound plays on the TV;
Spotify's does not, and cannot this way: Spotify forbids other apps
its sound (its package lacks Android's playback-capture flag and its
players carry the no-capture mark, read off the phone), Android hands
Kagaz silence for it and gives it only to its own Cast. Measured on
the phone: real sound is handed over at full level (peaks near 28,000
of 32,767) while the phone's own speaker is silenced, so silencing
the phone does not touch what goes to the TV.

- **Sound** (`KagazTv.kt`): what the phone plays, taken with the same
  screen-sharing grant (Android 10 and later; Android files it under
  the record-audio permission, asked once), encoded as Opus by the
  phone's own encoder, 20 ms a packet, sent as a second Cast stream
  (`cast/mod.rs`; each packet stamped by the samples before it, sound
  packets sent ahead of picture packets). Packets go all the time,
  silence when the sound stays on the phone, so the TV's player never
  waits for sound. While the sound is on the TV the phone's speaker is
  kept quiet: its media volume turned to nothing and put back after,
  looked at ten times a second (a press of the volume keys raises it a
  notch; it goes back). After four seconds of silence handed over
  while something plays, the app playing is taken to forbid its sound:
  the phone's volume comes back, the card says so, and the phone goes
  quiet again the moment real sound comes. A sound packet is skipped
  only when it would surely come too late (the first night skipped
  about one in eleven on a slow link, each a gap that is heard); with
  sound the TV plays 300 ms behind, not 200.
- **Each phone picture is its own Cast picture** now, stamped when it
  arrives (before, pictures that arrived together went as one); after
  300 ms with the TV behind, what waits is dropped and the stream
  starts again from a whole picture.
- **Hide** (`KagazTvPicture.kt`): the screen's pictures pass through
  the phone's graphics chip on their way to the encoder; while hidden
  each is averaged down to about 30 by 17 patches, each patch's colour
  rounded to 64 steps, and those five hundred colours spread out again
  smoothly under a dim tint: frosted glass, as the phone's own panels
  have. It cannot be undone, since the detail is thrown away on the
  phone, not scrambled: words, faces and shapes are not in what is
  sent; broad colour and large movement are. On a still screen the
  last picture is drawn again frosted at once.
- **The switches** (`KagazTv`): `screenHidden`, `soundOnTv`; hide
  everything = screen hidden and sound on the phone. Every place turns
  the same two and hears of each change: the card in Kagaz (Show or
  Hide everything as Material's filled button, a filter chip each for
  the screen and the sound); the notification (its own row, the
  same folded and unfolded: a line saying how things stand, then an
  eye for everything, a TV for the screen, a speaker for the sound,
  each always the same picture and struck through while that is kept
  from the TV, and Stop apart in red; the first try's buttons showed
  what a tap would do and swapped pictures, which confused); three
  quick-panel tiles (`KagazTvTiles`:
  Show on TV, Screen on TV, Sound on TV; lit while that goes to the TV,
  greyed out when nothing is mirrored; added from the panel's edit
  screen, whose steps the TV's sheet gives in the phone's own words,
  or by Android's own offer from Android 13); the floating button
  (`KagazTvFloat`: off until turned on in the TV's sheet and allowed
  in Android's "Appear on top"; Kagaz's mark is the handle, a tap
  opens it into a strip with the same pictures as the notification, a
  drag moves it; it is part of the screen, so the TV shows it).

Choosing the route (`tv/send.rs`): Miracast over the network when the
TV takes it there, else Google Cast when the TV has it, else Wi-Fi
Direct (Linux). `kagaz tv NAME mirror --via cast` asks for Cast; the
app's Mirror button follows the same order, and a TV with Cast counts
as ready to mirror. No sound goes over Miracast yet.

### A computer's sound, Hide and switches (2026-11-19, built; not yet seen on the TV)

The owner's choices: the sound follows the picture (the whole screen:
everything the computer plays; one window: that app's); the computer
goes quiet while its sound is on the TV; a session starts hidden; the
switches sit on the TV's card, a strip on top, keyboard shortcuts and
a tray icon while mirroring.

- **One state** (`tv/switches.rs`): live, screen hidden, sound on the
  TV, sound possible; every control turns it and hears each change
  (the app sends it to the page as "tv-controls"). A session starts
  hidden with the sound on the computer.
- **Hide on a computer** (`tv/picture.rs`, `Frost`; `switches::Guarded`
  wraps every computer's pictures, both routes): blocks of 64 x 64
  pixels averaged, each colour rounded to 64 steps a channel (about
  500 colours for a 1080p screen), a quarter of the colour taken out,
  dimmed 38 %, spread back smoothly; redone only when a block changes.
  While hidden, pictures in graphics memory go through the processor,
  where the glass is made.
- **Sound on Linux** (`tv/sound.rs`, `pw.rs`): measured first: a sink's
  monitor still carries full sound when the sink is muted or at volume
  0, so muting would not have quietened anything for the TV's sake;
  instead, while Sound on TV is on, Kagaz is a sound output of its own
  ("<TV> (Kagaz)", a PipeWire stream with the class Audio/Sink) and the
  default output (`pw-metadata` sets WirePlumber's configured default;
  measured: a player already playing moved over, and back when the
  default went back). The volume keys then set the TV's level as any
  output's. Off again, the output goes and the earlier default comes
  back; if Kagaz ends unexpectedly its output goes with it and the
  system falls back to the speakers. Opus, 160 kbit/s, 20 ms packets,
  through the phone's queue; silence fills gaps so the TV never waits.
  Not yet: one window's own app only (the portal does not say which
  app a window belongs to).
- **Sound on Windows** (`tv/sound_win.rs`; built, checked by building
  for Windows, not yet run): Windows' loopback recording of the default
  output while Sound on TV is on, the output muted meanwhile and its
  mute put back after; AAC (192 kbit/s, raw frames of 1024 samples)
  through Windows' own encoder in Media Foundation, since libopus does
  not cross-build to Windows (its SSE4.1/AVX2 files need flags the cross
  toolchain does not pass, and the toolchain cannot be overridden); the
  offer names "aac" then. To be seen: whether a muted output's loopback
  still carries the sound, and whether the LG takes AAC.
- **Sound on macOS 13 and later** (`tv/capture_mac.rs`,
  `tv/sound_mac.rs`; type-checked, not yet run): ScreenCaptureKit's
  stream carries the computer's sound beside the screen (not Kagaz's
  own); while Sound on TV is on it goes as Opus and the default output
  is muted (Core Audio), its mute put back after. To be seen: whether
  the capture's sound still comes with the output muted.
- **The strip on top** (`tv-strip.html`): state, Show/Hide everything,
  a TV and a speaker that never change picture (struck through while
  kept from the TV), Stop in red. Windows leaves it out of the TV's
  picture (`SetWindowDisplayAffinity`, WDA_EXCLUDEFROMCAPTURE), macOS
  too (`sharingType` none; untested). GNOME on Wayland lets no window
  keep itself on top (GTK 3 has no way there), so on GNOME the strip
  is not shown and the tray and shortcuts stand in.
- **Shortcuts**: Ctrl+Alt+H hides or shows everything, Ctrl+Alt+S moves
  the sound (macOS ⌃⌥). Linux: the desktop's GlobalShortcuts portal
  (`tv/keys.rs`; GNOME asks once and lets the keys be changed in its
  settings); Windows and macOS: Tauri's global-shortcut plugin. Bound
  only while a session runs; listed in the TV's sheet.
- **Tray** (Ubuntu shows it through its AppIndicator extension): only
  while mirroring; Show/Hide everything, Screen on TV, Sound on TV,
  Stop mirroring, Open Kagaz.

### A stand-in Cast TV (2026-11-19)

`kagaz tvs stand-in --cast [--lose N]` (`cast/stand_in.rs`): this
computer announces itself as a Google Cast TV (`_googlecast._tcp`,
port 8009, a certificate made at start), starts "screen mirroring" when
asked, answers the offer (H.264 and Opus), puts the stream together,
confirms each picture as soon as it is whole (as a receiver does) and
reports numbers only: pictures a second, the rate, each change of the
picture's size (read from the sequence header), whole pictures, sound
packets and seconds. `--lose N` loses every Nth stream packet. Tests
run the whole sender against it on 127.0.0.1 (the session, and sound
over a link losing one packet in 12).

What it showed at once, fixed: the "nudge" (the newest picture's last
packet again when the TV has not said it has it) went every 10 ms and
was counted as a resend, so every picture looked lost twice over; the
rate fitting then took the rate down each second, and OpenH264 started
again each time (a whole picture a second). Nudges are counted apart
now, every 20 ms at most; a resend waits at least 10 ms; and losses
lower the rate only while the stream uses half of it or the round trip
grows 40 ms past its best (queues filling), since random losses on a
stream far under its rate are the air's. Measured after: the test
pattern at 1080p60 through VA-API, 60 pictures a second, none dropped,
none sent again; over a link losing 1 in 12, the rate held, losses
resent, a second of sound each second.

### The rate fitted to the link; a phone upright at its own shape (2026-11-19, built; not yet seen on the TV)

- **Rate** (`cast/fit.rs`): once a second, from what the TV reports: a
  full link (the round trip 40 ms past its best, or losses or pictures
  held back while the stream uses half the rate) takes the rate down
  30 %; three clean seconds take it up 15 %; between an eighth of the
  starting rate and the starting rate. Pictures skipped because the
  computer was slow do not count. VA-API takes the new rate with the
  next picture (its rate control sent between whole pictures), Media
  Foundation and VideoToolbox in place, the phone's MediaCodec through
  `PARAMETER_KEY_VIDEO_BITRATE`; OpenH264 starts again at the new rate
  (a whole picture); NVENC keeps its rate (reconfiguring it is
  untried).
- **Upright** (`KagazTv.kt`): over Cast (which allows a new size
  mid-stream; Miracast fixes it), a phone standing upright encodes its
  own shape, 1920 tall (about 912 x 1920 on a 19:9 phone; within H.264
  level 4.2), 1280 where its encoder takes no more; when it turns, a new
  encoder at the new shape, the virtual display moved onto it. On a 4K
  TV an upright phone shows twice as sharp as inside a 1920 x 1080
  frame. Whether the LG's Cast receiver takes the change of size is to
  be seen.

### This computer as a screen, first step (2026-11-19, built; not yet used)

The TVs page on a computer has "This computer as a screen": "Let phones
and computers show their screen here", off until turned on (and back
on at Kagaz's start only if left on). While on (`here.rs`), Kagaz lists
this computer as a Google Cast screen under its name (port 8009) and
takes senders one at a time through the stand-in's receiving side
(`cast/stand_in.rs`, its `Screen`): each is asked about first in a small
window on top at the bottom right (`here-ask.html`, as sharing's
question: Allow or Decline, no answer in 30 seconds is Decline; a
declined sender is told the screen would not start); an allowed one
opens a window of its own (`here-view.html`: its name and screen under
Kagaz's green bar, the size and pictures a second beside it, Full screen
(F) and Stop in red; in full screen the bar fades until the pointer
moves; closing the window is Stop). The pictures go to the page as the
cameras' do and the page decodes them (WebCodecs, AVCC); the sound plays
on this computer's speakers through remote desktop's player (Opus; AAC
from a Windows sender is not played yet, nor anything on Windows, whose
Opus is not built in). Kagaz's sender gives its device's name when it
connects ("a phone" from a phone). This computer is left out of its
own list of TVs.

Who can send here now: Kagaz itself (a phone, another computer) over
Cast, since Kagaz's sender takes a screen's certificate as shown; and
over Miracast on the network (MS-MICE, port 7250, listed as
`_display._tcp`): Windows' own Cast, and Kagaz's Miracast sender. The
Miracast side (`miracast/stand_in.rs`, now on the same `Screen` as Cast)
asks when the sender says it is ready, answers the session with the
LG's abilities but plain sound only (LPCM, 48 kHz stereo, which every
sender has; played as it comes, no decoder), and hands over each picture
whole (a PES packet each). Measured with Kagaz's Miracast sender on this
computer: 60 pictures a second put together, all decoded, 1920 x 1080,
none lost. One sender at a time, whichever way it comes; a second is
declined while one shows. Not yet seen: Windows' own Cast here (it may
give up if the question waits long), and Samsung's Smart View, which
uses Miracast over Wi-Fi Direct (next). Android's and Chrome's own Cast
check the screen's certificate against Google's and will not send to
it.

## Next, agreed 2026-11-18

- **This computer as a screen** (the owner approved the plan; the first
  step, from Kagaz senders over Cast, is built, see above): on the TVs
  page, off until switched on ("Let phones and computers show their
  screen here"); while on, Kagaz announces this computer as a display;
  each sender is asked about first (Allow / Decline); the picture opens in
  its own window (full screen, Stop, a fading strip with who, size,
  pictures a second, lost packets); "Check a sender" keeps today's
  numbers-only mode and a report to copy; a gear for the name and the
  networks it announces on. Receiving: Miracast over the network first,
  then over Wi-Fi Direct (so a phone's own Cast screen lists it), then
  **Google Cast and AirPlay receiving when feasible** (the owner). The
  phone as a screen later.
- **Google Cast, further** (the sender itself is built, see above):
  Windows' and macOS's sound seen on real computers; one window's own
  app's sound on Linux; NVENC's rate fitted.
- **LG Link** (file transfer between LG Link or gram Link apps, PIN
  pairing): LG's own protocol; learn it from what LG's phone app sends to
  the TV, with step 3 (send media).
- **The phone's sound on the TV over Bluetooth** (the TV's "Play
  smartphone music"): a button that opens Bluetooth settings naming the
  TV.

## Rules

- **Nothing reaches a real TV without the owner's go-ahead at that
  moment**: no media, no key, no pairing request, no power-on. A TV
  shows things to whoever is in the room.
- **Testing.** No test captures or streams this computer's real screen,
  camera or microphone. Capture tests run in a sealed headless GNOME
  Shell (`gnome-shell --headless --wayland --no-x11 --virtual-monitor`
  inside `dbus-run-session`, with its own runtime, config, data and dconf
  folders in a fresh folder under `/tmp`), showing a generated test
  pattern. Receivers in tests are test receivers run here; protocol
  replies are recorded as fixtures under `tests/fixtures/` without real
  addresses, names or keys.
- **Libraries.** Permissive licences only, as above.

## Limits

- AirPlay mirroring from anything other than an Apple device needs
  Apple's FairPlay, whose sender side is not published; until it is done,
  Kagaz mirrors to such a TV by Miracast or Cast instead and says why.
- Content protected by DRM (a streaming service in a browser) is often
  black when captured; that is the service's choice, and Kagaz says so if
  a picture comes out black.
- No HDCP: a TV that demands it for mirroring refuses Kagaz.
- Some desktop effects never reach screen sharing, so no screen-sharing
  app shows them on a TV. Found 2026-11-18 with the Magic Lamp minimise
  effect on GNOME 50: the desktop's stream went from window to no window
  in one picture (a numbers-only check, `real_screen_changes`), and with
  the effect switched off GNOME's own minimise animation did show on the
  TV.
- A TV on no network can only be mirrored to, over Wi-Fi Direct: sending
  a file, the remote and Cast all need the TV on a network.
- macOS has no Wi-Fi Direct or Miracast for apps: a Mac reaches a TV on
  no network only through AirPlay's own peer-to-peer mode, from the
  system's AirPlay menu, and only to a TV that supports it.
- Extending on macOS rests on a private API; on Windows it needs a signed
  driver.

[MS-MICE]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-mice/
