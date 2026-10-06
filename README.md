# Kagaz

**Devices that just work. Any OS, any device, any connection.**

Kagaz (कागज़, "paper") is a free, open-source toolbox for the devices around
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
kagaz tapo login
kagaz cameras
```

Tapo cameras anywhere in the world, through your own TP-Link account and
TP-Link's relay, exactly as the Tapo app reaches them, but shown the way
you want: `kagaz tapo login` once (email code supported), then `kagaz
cameras` serves every camera on the account and starts the video engine
(go2rtc, downloaded and checksum-verified on first use, never bundled). The
window's "Cameras" view shows them side by side, any number, click to
enlarge. `kagaz tapo record <camera> --seconds 10` saves raw video to prove
the path. No Tapo Care subscription is needed; only your own cameras are
reachable.

```
kagaz tapo recordings <camera> --date 2026-10-06
kagaz tapo download <camera> --from "2026-10-06 10:00" --to +10
```

What is on a camera's SD card, and saving some of it: `recordings` lists
the card, the days with footage and one day's clips (times on the camera's
own clock); `download` saves a span as MP4 through the video engine (or raw
MPEG-TS with `--ts`), as fast as the camera sends it. In the window, switch
"Live" to "Playback": pick a day and a time and press "Play all", or click a
camera's timeline, and every camera shows the same moment from its card;
"Download…" under a camera saves a span. The speaker badge on a tile plays
that camera's sound (Kagaz decodes the camera's G.711 itself; no extra
needed). Cameras can be arranged into
groups ("rooms"): "Rearrange", "Add group", drag cameras between groups,
collapse a group, "View" one alone, play one group from a chosen time. The
layout is kept in `~/.config/kagaz/cameras.toml`.

The desktop window (`cargo run -p kagaz-desktop`) shows the same devices,
the same plain-words explanation and the same scan options; on Linux it
needs the webkit2gtk and gtk3 development packages to build.

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
- **M7 More device classes, each its own module.** Robot vacuums (starting
  with devices on the Tuya/SmartLife platform), IP cameras and doorbells
  (ONVIF, RTSP), and onward as contributors bring devices. Tapo cameras over
  the TP-Link cloud are in (pulled forward by need); local ONVIF/RTSP cameras
  next.
- **M8 AI assistant module.** Configurable model backends (local servers,
  OpenRouter, OpenAI-compatible and Anthropic endpoints, the Claude Agent SDK
  as the agent runtime), used to draft, test and package a module for a
  device that has none.

## Contributing

Issues and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).
The driver database in `drivers/` is the easiest place to help: one file per
model, pointing at the vendor's official downloads.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE). Contributions are accepted under the [CLA](CLA.md), which lets the project also be licensed commercially to parties who cannot use the GPL.
