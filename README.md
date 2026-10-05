# Kagaz

**Printers and scanners that just work. Any OS, any device, any connection.**

Kagaz (कागज़, "paper") is a free, open-source tool that removes the friction of
connecting printers and scanners: finding the device, knowing what it can do,
getting the right driver when one is needed, and then printing and scanning
from one fast, small app. It runs on Linux, Windows and macOS from the same
code, as a desktop app and as a command-line tool.

> Status: early development. Nothing to download yet. See [Roadmap](#roadmap).

## Why

Most devices made in the last decade speak open standards: IPP for printing,
eSCL and WSD for scanning, mDNS and WS-Discovery to be found, IPP-USB over a
cable. On a good day the operating system notices and everything works. On a
bad day you are on a vendor website guessing which of twelve downloads applies
to you, then discovering the "Scan to PC" button needs a second tool with four
manual steps. Kagaz is built for the bad days.

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

## How it is built

```
crates/kagaz-core   library: discovery, device model, protocols, driver database, scan/print
crates/kagaz-cli    the `kagaz` command, a thin layer over the core
apps/desktop        the desktop app (Tauri), another thin layer over the core
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

finds every printer and scanner on your network (mDNS/DNS-SD and
WS-Discovery) and says which of them can print and scan without a driver.
`--json` gives machine-readable output.

## Roadmap

- **M1 Find and explain.** Discover printers and scanners on the network and
  USB (mDNS/DNS-SD, WS-Discovery, SNMP, IPP-USB); show model, address,
  protocols, and what this OS can do with the device right now. CLI first,
  then the window.
- **M2 Driverless scan.** Scan over eSCL and WSD (glass and feeder, preview,
  PDF/JPEG/PNG), identical on all OSes.
- **M3 Driverless print.** Print over IPP, with job and supplies status.
- **M4 Driver finder.** The `drivers/` database and the consent-based
  vendor-installer flow, starting with Brother on Linux (which is where this
  project was born).
- **M5 Extras, all optional downloads.** OCR to searchable PDF (Tesseract),
  the printer's Scan-button listener (cross-vendor push scan), smart filing
  with on-device models, print queue and supplies view.

## Contributing

Issues and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).
The driver database in `drivers/` is the easiest place to help: one file per
model, pointing at the vendor's official downloads.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
