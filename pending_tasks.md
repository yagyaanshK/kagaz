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

- [x] Window Cameras view: all seven tiles live and smooth for minutes
      (user, 2026-10-07) with "playback: direct" (go2rtc's MPEG-TS, chunked,
      in a plain <video>, one loopback address per tile). MSE and WebRTC
      flicker in this webview and stay as options only.
- [x] **Playback and downloads from the SD card** (2026-10-07): the cloud
      passthrough (`services-sync`), days with footage, one day's clips, the
      relay's recorded-stream type with the app's playback request (paced,
      ended by measuring the video's own clock) and its download request
      (as fast as the camera sends, ends by itself), MP4 through the engine.
      Verified on the C100 4.0 cameras: one minute of footage as raw TS in
      7 s and as MP4 (video only) through the engine; the window's paced
      route through the per-tile proxy ends cleanly. Window UI (date, time,
      timelines, Play all, Download…) and groups with rearrange are written
      and await a look in the window.
- [ ] **First-generation C100 (hardware 2.0, firmware 1.3.x) refuses
      playback over the relay.** Its component list has `playback` v4 and no
      `recordDownload` (the 4.0 models have `playback` v6, `recordDownload`
      v2, `cipcV2Relay`). The app's playback request gets -52402
      (VOD_INVALID_REQUEST), the download request -51416, and after a few
      tries -52407 (TOO_MANY_CLIENT) for a long while. Live view and the
      recording lists work on them. Find the request shape the app uses for
      playback v4 (`examples/tapo_probe.rs` tries frames one at a time; the
      component list comes from `getAppComponentList`).
- [ ] **Audio in downloads.** The camera's download stream carries G.711
      (MPEG-TS stream type 0x90, TP-Link private); go2rtc's MPEG-TS reader
      (`pkg/mpegts/producer.go`) only takes H.264, H.265, AAC and its private
      Opus type from an HTTP source, so the engine's MP4 keeps only the
      video. `--ts` keeps everything. Fix: remux in Kagaz (TS → MP4 with
      G.711 as `alaw`/`ulaw`), or a go2rtc change upstream.
- [ ] **Continuous recording to disk** (asked for 2026-10-07): a `kagaz`
      command or script for a server PC that keeps pulling each camera's
      footage into a folder within a storage limit, like a CCTV recorder.
      The download path above is the building block.
- [x] **Automatic reconnection** (2026-10-07): a tile that has shown "no
      data" reconnects by itself after 5, 10, 20, 40, 60, 60 s, then stops
      trying and leaves Reload. Thresholds (2 s waiting, 15 s lost) unchanged;
      tune after real use.
- [ ] **Local ONVIF/RTSP cameras** (the local C100 and any camera
      on the LAN): discovery is already filtered out of the printer list; add
      them to the Cameras view as RTSP sources for go2rtc, with the camera
      account credentials entered once.
- [x] **Token refresh and re-login** (2026-10-07): the window has "Log in"
      (email, password, email code) and "Log out"; an expired token is
      refreshed when the cameras start and when a relay request is refused.
      Refresh and the window login are written from the CLI flow and not
      yet exercised against an expired token.
- [ ] **Audio**: Tapo carries G.711 on private MPEG-TS stream types; go2rtc may
      not pass it. Video only for now.
- [x] **Resolution choice** per tile (2026-10-07): the HD/VGA badge on a
      tile toggles it for that camera and is kept in the layout file;
      enlarged tiles are always HD.
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
