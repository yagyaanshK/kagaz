# Pending tasks

Things that are written but not yet verified, or known gaps. Tick them off
with the commit that closes them.

## Needs hardware or a person at the device

- [ ] **Feeder, multi-page scan over WSD.** `kagaz scan brother --source feeder`
      with a few pages loaded. The single-page glass path and the empty-feeder
      fallback are verified; the "keep retrieving until the job is gone" loop
      for the feeder is not.
- [ ] **eSCL on a real device.** `crates/kagaz-core/src/scan/escl.rs` is written
      from the specification with hand-built fixtures
      (`tests/fixtures/escl/`). Needs any eSCL/AirScan scanner. Once verified,
      record real fixtures and make the dispatcher in `scan/mod.rs` prefer
      eSCL over WSD.
- [ ] **USB discovery live check.** Plug the Brother in over USB and run
      `kagaz discover --no-mdns --no-wsd --no-snmp`. Expect the classic printer
      class (7/1/2) with the IEEE 1284 ID read from sysfs. Record the ID as a
      fixture under `tests/fixtures/usb/` and point the test in
      `discovery/usb.rs` at it.
- [ ] **Windows run.** On the Windows PC: `kagaz discover`,
      `kagaz explain`, `kagaz scan` against the Brother. Known gap: the IEEE
      1284 ID of USB printers is not read on Windows (usbprint.sys owns the
      device); the USB product string is used instead.
- [ ] **macOS run.** No Mac available.

## Known gaps in the code

- [ ] **eSCL over TLS only** (`_uscans` with no `_uscan`): refused with a
      message, because `ureq` is built without TLS. Decide between rustls
      (size) and native-tls (per-OS libs) when a device needs it.
- [ ] **mDNS TXT record missed now and then.** About 2 runs in 20, the
      Brother's IPP TXT record does not arrive within the 800 ms fallback
      window in `discovery/mdns_txt.rs`, so the print verdict drops to
      "WSD" only (or "AirPrint" only via the SNMP CMD field). A second TXT
      query, or a longer window, would likely fix it. Kept as is on request.
- [ ] **IPP-USB scan capability** is reported as "likely, not checked": no
      eSCL probe over the USB cable yet.
- [ ] **Duplex over WSD** is coded (MediaBack) but untested: the Brother has
      no duplex feeder.
- [ ] **Scan preview** (low-dpi quick scan to screen) waits for the desktop
      app.

## Scanning engines

- [ ] **Driver engine at high resolution.** `kagaz scan --engine driver --dpi 600`
      (grey, full A4, Brother brscan4 over Wi-Fi) did not finish in five
      minutes on 2026-10-06; 100 dpi takes 36 s. Find out whether scanimage
      hangs or is just that slow, and show progress or a time estimate.
- [ ] **Driverless resolutions are probed, not advertised.** The Brother lists
      100-300 over WSD but validates and scans 600; Kagaz asks about 150, 400,
      600 and 1200 at connect time. Check the same on an eSCL device.

## Desktop app polish

- [x] Illustrations next to each option in the Scan pane and a badge per
      verdict in About (inline SVG, 2026-10-06). Worth a look in the window.

## Cameras (Tapo over TP-Link's cloud)

- [x] Window Cameras view: live tiles verified by the user (2026-10-07), with
      the playback route measured in a real WebKitGTK view: go2rtc's MPEG-TS
      over HTTP in a plain <video>, chunked, each camera on its own loopback
      address (WebKit allows six connections per host). Stall overlay and
      Reload per tile and for all.
- [ ] **Stall detection is time-based** (clock not advancing): tune the 2 s /
      15 s thresholds after real use; consider reconnecting automatically after
      a long silence.
- [ ] **Local ONVIF/RTSP cameras** (the local C100 and any camera
      on the LAN): discovery is already filtered out of the printer list; add
      them to the Cameras view as RTSP sources for go2rtc, with the camera
      account credentials entered once.
- [ ] **Token refresh and re-login** in the window when the TP-Link session
      expires (the CLI says "not logged in; run kagaz tapo login").
- [ ] **Audio**: Tapo carries G.711 on private MPEG-TS stream types; go2rtc may
      not pass it. Video only for now.
- [ ] **Resolution choice** per tile (HD/VGA) to save bandwidth with many cameras.
- [ ] The Tapo APK static analysis and the emulator capture are no longer
      needed; the protocol came from public sources and works. Keep the tools
      under installations/tapo-analysis/ in case TP-Link changes the relay.

## Roadmap, not yet started

- [x] M3 driverless print over IPP: `kagaz print` sends PWG Raster; one test
      page printed on the Brother (job 91, 2026-10-05).
- [ ] **AirPrint-only printers (URF, no PWG Raster).** `kagaz print` refuses
      them with a message. Apple Raster is PWG Raster with a different header;
      add it when such a printer is around to test.
- [ ] **Two-sided printing** is coded (sides keyword, back-side rotation per
      pwg-raster-document-sheet-back) but only one-sided has been printed.
- [ ] M4 driver finder: `drivers/` TOML database and the consent-based vendor
      installer flow, starting with Brother on Linux (brscan4 + brscan-skey,
      reference in `installations/brother-scanner/`).
- [ ] **Desktop app: the new 'Scan button' tab and the illustrations have not
      been looked at yet.**
- [ ] **Desktop app: a scan from the Scan pane.** The window builds, runs, lists
      the Brother and fills the About pane (checked by the user on
      2026-10-05); nobody has yet clicked Scan in it. `cargo run -p kagaz-desktop`;
      the crate is outside the default members so `cargo build`/`cargo test`
      stay green without the webkit2gtk/gtk libraries.
