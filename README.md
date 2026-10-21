<p align="center"><img src="assets/logo.svg" alt="Kagaz" width="96" height="96"></p>

# Kagaz

**Devices that just work. Any OS, any device, any connection.**

Kagaz (कागज़, "paper") is a free, source-available toolbox for the devices around
you that come with bad software: printers and scanners first, then robot
vacuums, IP cameras, doorbells and whatever else answers on the network or a
cable. It finds the device, tells you what it can do, gets the official
driver when one is needed, and lets you use the device from one fast, small
app or from the command line. No always-on server, no hub, no account: one
small binary on your laptop, on Linux, Windows or macOS.

The base app stays tiny because it knows nothing about any particular kind
of device. Support for each device class is a module you download on demand,
and so is the optional AI assistant that can work out how to talk to a device
nobody has written a module for yet.

> Status: early development. Nothing to download yet. See [Roadmap](#roadmap).

## Why

Most devices made in the last decade speak open standards: IPP for printing,
eSCL and WSD for scanning, mDNS and WS-Discovery to be found, IPP-USB over a
cable. On a good day the operating system notices and everything works. On a
bad day you are on a vendor website guessing which of twelve downloads applies
to you, then discovering the "Scan to PC" button needs a second tool with four
manual steps. Kagaz is built for the bad days.

## What it is not

Not a smart-home hub. Home Assistant is excellent at running a house from an
always-on box; Kagaz is the tool you open on a laptop when you want to deal
with a device, and it borrows freely from the protocol work the Home Assistant
and Valetudo communities have already done, without asking you to run either.

## Principles

1. **Standards first.** Driverless protocols are used whenever the device
   offers them, with the same code on every OS. No driver, no vendor app.
2. **Official drivers only, by default.** When a device needs a driver, Kagaz
   identifies the exact model, fetches the vendor's own package from the
   vendor's own server, verifies it, shows you what it will do, and runs the
   vendor installer after you consent. Community-written backends (for example
   open-source SANE drivers) exist as an opt-in fallback, off by default, with
   a clear warning.
3. **Fast and bloat-free.** A native Rust core, a small UI, instant start.
   Heavy optional pieces such as OCR engines and language data are never
   bundled; they are downloaded in-app only if you ask for them, and
   everything else works without them.
4. **CLI is a first-class citizen.** `kagaz` on the command line can do
   everything the window can, is scriptable, and is the thing power users and
   automation reach for.
5. **Honest about limits.** If a device cannot work on this OS without a
   driver that does not exist, Kagaz says so in plain words instead of
   pretending.
6. **Modules, on demand, sandboxed.** Each device class is a module compiled
   to WebAssembly: one file that runs the same on every OS, written in any
   language, and allowed to reach only the device and ports it declares. The
   core and the UI are native and stay on the fast path; modules only do
   device talk, which the network bounds, not the CPU.
7. **AI only when you ask for it.** An optional assistant module can
   write and test a driver for an unknown device on the fly. It runs with any
   model you configure, local or remote, with your own keys, and anything it
   writes runs in the same sandbox as every other module, after you have seen
   what it will do. Nothing of this ships in the base app.

## How it is built

```
crates/kagaz-core   library: discovery, device model, module host, driver database
crates/kagaz-cli    the `kagaz` command, a thin layer over the core
apps/desktop        the desktop app (Tauri 2, plain HTML/CSS/JS), another thin layer over the core
modules/            device-class modules (first: printers and scanners), built to WebAssembly
drivers/            community-maintained database: model -> official driver per OS
docs/               design notes and protocol references
```

Rust for the core and CLI; Tauri for the desktop window. One repository, one
version, three operating systems.

## Platforms

| OS | Packages planned |
|---|---|
| Linux, Debian/Ubuntu family | `.deb`, static tarball |
| Linux, Arch family (including [Omarchy](https://omarchy.org)) | AUR package, static tarball |
| Linux, Fedora family | `.rpm`, static tarball |
| Windows 10/11 | installer and portable `.exe` |
| macOS 12+ | `.dmg`, Homebrew |

The desktop app follows the desktop it runs on: GNOME, KDE and tiling Wayland
compositors such as Hyprland (Omarchy's default) are all first-class, with
no GNOME-only or systemd-only assumptions in the core.

## Try it today

```
cargo run -p kagaz-cli -- discover
```

finds every printer and scanner on your network (mDNS/DNS-SD,
WS-Discovery, and an SNMP broadcast for printers that announce nothing) and
on your USB ports (IPP-USB and the classic printer class,
with the device's IEEE 1284 identity), and says which of them can print and
scan without a driver. `--json` gives machine-readable output.

```
kagaz explain 1
```

(or `kagaz explain 192.168.1.20`, `kagaz explain brother`) says in plain
words whether this computer can print to and scan from that device right
now, why, and what would fix it.

```
kagaz scan brother
```

scans without a driver (WSD-Scan, verified on a Brother; eSCL written from
the specification, not yet verified on a device) to `scan-<date>-<time>.pdf`
in the current directory: the feeder if it has paper, else the glass, at
300 dpi in colour. `--source`, `--dpi`, `--mode gray|bw`, `--format jpeg|png`,
`--paper`, `-o file` change that, and `--max-size 2M` keeps a file under a
size by re-encoding and shrinking pages, so no separate compressor is needed.

```
kagaz status brother
kagaz identify brother
```

ask a printer over IPP how it is doing (state, problems in plain words,
toner, loaded paper, queue) and make it flash its display so you know which
one it is.

```
kagaz print brother document.pdf --sides long
```

prints a PDF, JPEG or PNG without a driver on any IPP Everywhere printer:
rendered here (pure Rust) at the printer's resolution and paper size, sent
as PWG Raster. `--copies`, `--paper`, `--gray`, and `--dry-run` to check
with the printer without printing.

```
kagaz driver brother --plan
kagaz driver brother
```

finds the vendor's official driver for a device in the `drivers/` database
(or, for Brother on Linux, on Brother's own download server), shows every
package with its checksum status and every command it will run, downloads
and verifies the packages, and runs the administrator steps through the OS
prompt only after you type yes. `--remove` undoes it.

```
kagaz button-settings
kagaz button-settings --set image resolution=300
```

shows and changes what a Brother's own Scan to PC button does (resolution,
paper, both sides) per action. The values live in Brother's files; Kagaz
writes only your personal copy in `~/.brscan-skey/` and `--reset` removes
it so Brother's default applies again. The window has the same under
"Scan button".

```
kagaz fans
kagaz fan 4a24 add --name "Bedroom fan" --room Bedroom
kagaz fan "Bedroom fan" speed 3
kagaz fan "Bedroom fan" off
```

Atomberg ceiling fans, the way their app and remote reach them: over
Bluetooth from this computer (a fan answers from the day it is unboxed, no
Wi-Fi setup needed) and over the Wi-Fi network once a fan is on it. `fans`
lists the fans you added, by room, with how each is now (Off, Speed 3,
Boost, light on, hours left on the timer) or "out of reach", then the fans
within reach that are not added yet. `fan <name> on|off|speed N|light on|off|
brightness N|sleep on|off|timer 1|2|3|6|read` does one thing at a time, as
the fan wants it; `read` also gives the fan's own count of how long it has
run and been switched off to date, and `read --raw` its state as it came.
`fan <id> add`, `rename`, `room` and `forget` keep the
list, and `kagaz rooms` keeps the rooms, in `~/.config/kagaz/fans.toml`.
Verified over Bluetooth: found, state read, on, speeds 1 to 5 and Boost
decoded from an Aris set with its remote, a command taken by a Renesa.
Light, colour, brightness, sleep and timer are still from the community's
notes, as is Wi-Fi, which awaits a fan on the network.

```
kagaz tapo login
kagaz cameras
```

Tapo cameras anywhere in the world, through your own TP-Link account and
TP-Link's relay, exactly as the Tapo app reaches them, but shown the way
you want: sign in once in the window's **Security cameras** page (or
`kagaz tapo login`; email code supported), and it shows every camera on
the account side by side,
any number, tap to enlarge, on the desktop and on a phone alike. The
window plays the video itself: Kagaz takes the relay's stream apart into
pictures and sound and the page decodes them with the browser's own
decoder, so no helper program is needed. (`kagaz cameras` and the
desktop's fallback modes still use the go2rtc engine, downloaded and
checksum-verified on first use, never bundled.) `kagaz tapo record
<camera> --seconds 10` saves raw video to prove the path. No Tapo Care
subscription is needed; only your own cameras are reachable.

```
kagaz tapo recordings <camera> --date 2026-10-06
kagaz tapo download <camera> --from "2026-10-06 10:00" --to +10
```

What is on a camera's SD card, and saving some of it: `recordings` lists
the card, the days with footage and one day's clips (times on the camera's
own clock); `download` saves a span as MP4 with sound (AAC, through a
checksum-pinned ffmpeg fetched once; or raw MPEG-TS with `--ts`), as fast as
the camera sends it. In the window, switch
"Live" to "Playback": pick a day and a time and press "Play all", or click a
camera's timeline, and every camera shows the same moment from its card;
the bar above the grid moves all of them together, and a slider sets the
speed (0.25x to 8x);
"Download…" under a camera saves a span. The speaker badge on a tile plays
that camera's sound (Kagaz decodes the camera's G.711 itself; no extra
needed). A Tapo camera on the same network as the computer is viewed
straight from the camera, as the app does on the same Wi-Fi, when the
account password is kept in the private session file (`kagaz tapo login
--no-local` to never keep it); the rest goes through the relay. Cameras can
be arranged into
groups ("rooms"): "Rearrange", "Add group", drag cameras between groups,
collapse a group, "View" one alone, play one group from a chosen time. The
layout is kept in `~/.config/kagaz/cameras.toml`.

`scripts/install-desktop-entry.sh` puts a build from this checkout in the
Linux app list and dock with Kagaz's icon (`--remove` takes it out).

The desktop window (`cargo run -p kagaz-desktop`) shows the same devices,
the same plain-words explanation and the same scan options; on Linux it
needs the webkit2gtk and gtk3 development packages to build.

## The app

The window opens on **Home**: the devices set up on this computer or
phone, grouped by kind (doorbell, security cameras, printers and
scanners, ceiling fans), each kind collapsed until you tap it. **Add a device** starts
from the kind of device, then its brand: brands Kagaz works with show in
colour, the ones planned show greyed as "coming soon" (cameras such as
DSLRs, robot vacuums, TVs, fridges, washing machines and air conditioners
are listed the same way). Each kind of device keeps its own settings on
its page, behind its gear: the doorbell's hold the ring sounds (calm tones
Kagaz makes itself, or your own songs), first for every doorbell, then
each brand's own (OneTouch: ringing here, door stations, family, one
device; stations on this network). The app's **Settings** holds what
applies to the whole app: the accounts Kagaz is signed in to, and about.

On **Printers & scanners**, **Add** keeps a printer the search found in
**My printers**. Kagaz remembers how to reach it, so it is listed at once
the next time, with how it is: Ready, Printing, Needs attention (in the
printer's own words, such as "out of paper") or Offline. A search still
runs alongside, lists new printers under **Found nearby**, and catches a
printer that the router has given a new address.

A scan shows first, then you save it: press **Scan**, look the page over,
use the tools on it, then **Save…** (PDF, JPEG or PNG, under the size
chosen). A scan under way can be stopped with **Cancel scan**: the scanner
is told to stop.

Scan and Copy can also take **both sides of an ID card or a passport page
on one page**, as a printer's 2-in-1 ID copy does, for any scanner Kagaz
reaches, including ones whose panel offers ID copy only for copying. Put the
card face down in the half of the glass by its corner mark and scan the
front; turn it over in the same place and scan the back. The page has the
front's half above the back's, each just where it lay; Scan saves it, Copy
prints it at real size.

The same three **tools** work on every scan, every ID page, and any PDF,
JPEG or PNG on this device from the **Tools** page (no printer needed; the
original stays as it was):

- **Tidy**: **Clean background** turns the lid's grey white; **Find the
  card** puts a frame around each card (one per half of an ID page), as
  Adobe Scan does. Drag a frame's corners onto the card, a tilted one too,
  and a side's middle to bend it for a page that bows; corners are rounded
  as a card's are, and outside the frames turns white. Remove a frame for a
  one-sided ID.
- **Crop**: a frame around what to keep (a photo, a signature), then just
  that area at its real size, enlarged to fill the page without
  stretching, or the whole page with the rest white.
- **Compress**: under 200 KB to 5 MB, as PDF, JPEG or PNG; it says the size.

Undo goes back one tool, Start over to the page as it came. Nothing is
stretched by Tidy or plain Crop, so a card or signature keeps its real size.
The tools are written once (kagaz-core `tools`, the window's `bench.js`), so
a new source, such as a camera, only adds a button.

On **Ceiling fans**, the fans you added show at once, by room, each with
how it was last seen; a look around runs alongside and the ones within
reach get their controls: Turn on and off, the speed, the light (on fans
that have one), Sleep and a timer. A timer shows as a countdown on the
card and on Home; when it ends, Kagaz asks the fan how it is. A fan that does not answer stays listed
as out of reach rather than vanishing. **Found nearby** lists the fans
within reach that are not added yet; **Add** asks for a name and a room.
**Rearrange** makes, renames and removes rooms, and moves fans between
them; the fan's own menu does the same for one fan. Rooms and names live
in `~/.config/kagaz/fans.toml`, shared with the command line.

Each page keeps its place: leave a printer's page with a scan under way
for the cameras, and coming back finds the scan where it was. Camera
pictures play only while their page is chosen (on a phone, also only
while Kagaz is on screen), sparing the connection; downloads carry on.
Wide windows have a sidebar; phones a bottom bar.

On a phone, Kagaz keeps listening for doorbell calls after you close it,
with a small "Ready for doorbell calls" notification; some phones stop
background apps to save battery, so allow Kagaz to run in the background
there.

A ring (from OneTouch or from a door station on your network) shows as a
call's notification, as in WhatsApp, also when Kagaz's window is closed: **Decline** stops
the ring on this device (the others ring on), **Answer** opens Kagaz and
picks up, and tapping the card opens the ring without answering. A volume
key quiets the ring. The doorbell's settings choose, for each device, what
a ring does: **Notification and ring sound** (the default), or **Silent
notification** (on a computer: a notification that opens the call when
clicked, no chime, no window jumping forward); on a phone, **Vibrate**
goes with either. During a call on a phone, Home (or Back with nowhere
left to go in Kagaz) shrinks the call into a small floating window, as
WhatsApp's calls do, with Hang up on it; tap it to come back to the call.

## OneTouch doorbell accounts

In the app, open **Doorbell → Sign in**. Select **OneTouch Smart Cloud**
for migrated accounts, or **OTIPVDP (legacy)** for an older account. After
sign-in, each device of the account is one card: tap it and its live view
plays inside it, with a **Call** button under the picture. Live view is
picture only (the relay sends no sound for it); a call brings the door's
sound, your microphone and **Open door**. Wide pictures fill the width with
their controls along the bottom, tall ones fill the height with their
controls down the side, and full screen turns a wide picture on its side on
an upright phone. **Ring me here** makes this device ring when someone calls
the account. **Recordings & snapshots** shows cloud history. Local intercom
calls continue to work separately.
Enable **Remember sign-in securely** to keep the login in your password
store (the desktop's vault, or the Android Keystore on a phone). OneTouch
allows one session per account, so Kagaz signs in again only when you open
the Doorbell pane; ringing works without that. Sign out keeps the saved
login; **Forget saved sign-in** removes it. Give each person in the home
their own login (OneTouch's family accounts, up to ten): then nobody signs
anyone else out, each is listed under the doorbell's devices with a Call
button. The doorbell's **History** tab shows the visitors (with the
station's picture of them) and the calls, by day, with filters for
visitors, calls, missed calls and stations on this network.

The integration builds and passes protocol/UI checks; authenticated playback
still needs verification with a real account. See [protocol notes and limits](docs/onetouch.md).

## Roadmap

- **M1 Find and explain.** Discover printers and scanners on the network and
  USB (mDNS/DNS-SD, WS-Discovery, SNMP, IPP-USB); show model, address,
  protocols, and what this OS can do with the device right now. CLI first,
  then the window.
- **M2 Driverless scan.** Scan over eSCL and WSD (glass and feeder, preview,
  PDF/JPEG/PNG), identical on all OSes. WSD done; eSCL written, awaiting a
  device to verify it on.
- **M3 Driverless print.** Print over IPP, with job and supplies status.
  Done for IPP Everywhere (PWG Raster) printers; AirPrint-only (URF) printers next.
- **M4 Driver finder.** The `drivers/` database and the consent-based
  vendor-installer flow, starting with Brother on Linux (which is where this
  project was born). Database, Brother lookup, plan and install flow done;
  Windows and macOS entries to come.
- **M5 Extras, all optional downloads.** OCR to searchable PDF (Tesseract),
  the printer's Scan-button listener (cross-vendor push scan), smart filing
  with on-device models, print queue and supplies view.
- **M6 Module host.** Move the printer/scanner support into the first
  WebAssembly module; the base app becomes device-agnostic.
  > *Update, 2026-10-21:* kept as written, but no longer the next step.
  > The three classes shipped so far (printers and scanners, Tapo cameras,
  > doorbells) all needed things a sandbox cannot do on its own: multicast
  > and SIP, sound and video decoding, raster rendering, the OS password
  > vault, notifications, a foreground service on Android. A module runtime
  > would add megabytes and a serialisation boundary without making any of
  > that faster or safer. New classes stay native in kagaz-core, split by
  > Cargo features if binary size ever matters. The sandbox is for M8,
  > where code nobody has reviewed must run.
- **M7 More device classes, each its own module.** Robot vacuums (starting
  with devices on the Tuya/SmartLife platform), IP cameras and doorbells
  (ONVIF, RTSP), and onward as contributors bring devices. Tapo cameras over
  the TP-Link cloud are in (pulled forward by need); local ONVIF/RTSP cameras
  next.
  > *Update, 2026-10-21:* the next two classes are **TVs** and **ceiling
  > fans**, before robot vacuums and local ONVIF cameras.
  >
  > **TVs, LG webOS first.** The pains to solve: a remote on the laptop or
  > phone, and sending the screen or a file to the TV. Discovery is mDNS
  > plus a probe of the webOS control ports: a 2024 webOS TV on the test
  > network answered no SSDP search at all, which most webOS libraries rely
  > on. Pairing goes through the TV's own on-screen prompt, the way its
  > app does. Order: pair and remote (keys, text entry, volume, input,
  > launch an app, turn on by Wake-on-LAN), then casting (a file or page
  > from this computer; mirroring last, it is the hardest). Other brands
  > follow by protocol family: Android TV / Google TV (one pairing
  > protocol covers Xiaomi, TCL, Sony, OnePlus and some Hisense and
  > Philips), Samsung Tizen, Roku, Sony Bravia, Hisense VIDAA.
  >
  > **Ceiling fans, Atomberg first** (Bluetooth route in since 2026-10-21:
  > find, remember by room and name, control; Wi-Fi written, unverified).
  > Most fans have no network at all;
  > Kagaz will say so honestly, and whether a bridge (Bond, Broadlink, an
  > RF dongle) is ever acceptable is left open. Atomberg fans advertise
  > over Bluetooth LE even before they are put on Wi-Fi, and three on the
  > test home were reachable that way and not on the network, so the
  > first route is Bluetooth from a laptop or phone (no hub, no account).
  > Wi-Fi comes second: the community-documented local protocol (state
  > broadcast over UDP, JSON commands with a direct reply; unofficial and
  > may change with firmware) and, if needed, Atomberg's cloud API with
  > the key from their developer portal. Verbs: on and off, speed, light,
  > timer, sleep. This is the first class where a home-automation hub has
  > more to offer than Kagaz; Kagaz's part is the vendor-app replacement,
  > not rules.
- **Print from anywhere.** Send a document to your own printer at home
  from a phone or laptop elsewhere, through a Kagaz device on the printer's
  network that you set up for it; no vendor cloud.
- **M8 AI assistant module.** Configurable model backends (local servers,
  OpenRouter, OpenAI-compatible and Anthropic endpoints, the Claude Agent SDK
  as the agent runtime), used to draft, test and package a module for a
  device that has none.

## Contributing

Issues and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).
The driver database in `drivers/` is the easiest place to help: one file per
model, pointing at the vendor's official downloads.

## License

Kagaz is source-available under the [Kagaz License 1.2.0](LICENSE) (KGZ-1.2.0: the Kagaz Additional Terms with the PolyForm Strict License 1.0.0). It is not open source.

- **Use:** anyone may use the official releases, or the code from this repository built without changes, for any purpose, at home or at work, free of charge.
- **Contribute:** anyone may read the code, check it, and send changes back to this repository, under the [CLA](CLA.md).
- **Not allowed without written permission:** passing on Kagaz or any build of it, changed or not; forks or other works based on it (except contributions here); copying its code into other software; translating or porting it to another language; giving its code to an AI agent or model to rewrite Kagaz, even "from scratch"; using its code to train or fine-tune AI models, or in datasets for that; and making money from it. (Using AI to read, check or contribute to the code is fine. A rewrite by someone who never worked from this code is outside these terms.)

To ask for permission, open an issue.
