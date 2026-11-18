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
stand-in TV decodes every picture sent. Not yet tried with a real
desktop's dialog or on the TV; KDE and Hyprland not yet tried.
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
- A TV on no network can only be mirrored to, over Wi-Fi Direct: sending
  a file, the remote and Cast all need the TV on a network.
- macOS has no Wi-Fi Direct or Miracast for apps: a Mac reaches a TV on
  no network only through AirPlay's own peer-to-peer mode, from the
  system's AirPlay menu, and only to a TV that supports it.
- Extending on macOS rests on a private API; on Windows it needs a signed
  driver.

[MS-MICE]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-mice/
