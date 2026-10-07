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
- [x] **Sound in the window** (2026-10-07): the stream server pulls the
      G.711 A-law out of the session it is already relaying (live or
      playback), decodes it and serves a streamed WAV on
      `/audio/<device-id or playback token>.wav`; the speaker badge on a tile
      plays it (one camera at a time). Measured: 17.8 s of sound in 17 s next
      to the video. Audio and video travel separately, so they can sit up to
      a second or two apart; tighten if it shows.
- [x] **Audio in downloads** (2026-10-07, AAC through an ffmpeg extra as
      chosen): `kagaz tapo download` and the window's Download pull the
      camera's stream straight from the relay, split its G.711 off, and
      ffmpeg (static build fetched once, checksum pinned: John Van Sickle
      6.0.1 on Linux, gyan.dev 9.0.2 on Windows, evermeet 9.0.2 on macOS)
      writes MP4 with the picture copied and the sound as AAC, lined up by
      the first timestamps. Without ffmpeg the engine saves the picture
      only. Gaps inside a span (event-only recording) let the sound drift
      after the gap: the raw G.711 carries no timestamps. `--ts` keeps the
      original.
- [ ] **Continuous recording to disk** (asked for 2026-10-07): a `kagaz`
      command or script for a server PC that keeps pulling each camera's
      footage into a folder within a storage limit, like a CCTV recorder.
      The download path above is the building block.
- [x] **Automatic reconnection** (2026-10-07): a tile that has shown "no
      data" reconnects by itself after 5, 10, 20, 40, 60, 60 s, then stops
      trying and leaves Reload. Thresholds (2 s waiting, 15 s lost) unchanged;
      tune after real use.
- [x] **Local Tapo camera, local first** (2026-10-07, as chosen): the
      cloud list carries each camera's hardware address; an ONVIF
      WS-Discovery probe plus the neighbour table pairs a camera on this
      network with its cloud entry (`cameras/local.rs`, verified on the
      C100 here). When the account password is kept (`kagaz tapo login`
      without `--no-local`, or the window's Log in), that camera's live
      view is go2rtc's own `tapo://` source on the LAN; everything else
      (other cameras, playback, downloads, sound) stays on the relay. The
      sound route opens its own small relay session when no relay video
      session feeds a camera, so a local tile still has sound. Not yet
      exercised with a kept password: log in again to try it.
- [x] **Shared seek bar and playback speed** (2026-10-07): a bar above the
      grid shows every visible camera's recordings for the day; click or
      drag moves all of them to that moment. Speed is a continuous slider
      (0.25x to 8x). The webview ignores a playback rate on the streamed
      video (measured), so Kagaz rescales the stream's timestamps (PTS,
      DTS, PCR) on the way through and, above 1x, asks the camera for its
      fast delivery; measured steady at 4x and 0.5x. Sound plays at 1x
      only. When the camera's fast delivery is slower than the chosen
      speed the tile shows "waiting" now and then.
- [x] **Gaps in a camera's day** (2026-10-07): a shared playback clock runs
      at the chosen speed from the last seek; once a second each camera is
      checked against its recording list. With nothing recorded for that
      moment it freezes on its last frame under a dark veil ("no recording
      for this time; resumes at hh:mm:ss"), its camera slot is freed, and it
      starts again by itself when the clock reaches its next recording.
      Day and time are dropdowns (dd/mm/yyyy, hh:mm:ss; a dot marks days
      with footage on the first camera).
- [x] **Per-camera clocks and bars; playback bounded by recording spans**
      (2026-10-07): each camera keeps its own playback clock; its bar shows
      12 hours around it with a cursor and seeks only that camera, the top
      bar shows 24 hours and moves all. A camera's stream skips its own
      gaps, so each playback is asked only to the end of the current
      recording span (clips with seams of 2 s or less joined); verified on
      a real 60 s gap: the stream stopped at the gap's start. The camera
      user id is now asked once per camera and retried on -71101, which
      had left a camera without its recording list.
- [ ] **Playback clocks for the grid, each group and each camera**
      (2026-10-08, written, not yet tried in the window): Play all and the
      top bar start every camera from the same instant (each waits on its
      first frame); ahead cameras pause for their clock, behind ones are
      fetched again a little ahead. Pause and speed on every camera, a
      group row (pause, 24-hour bar, speed), Pause all. A pause over 20 s
      lets the cameras' connections go and resumes them together.
- [ ] **Recordings cache and kagaz tapo fetch** (2026-10-08): written and
      checked on a real minute of footage (seek to the full frame before,
      paced at real time). The first full run of 1 October collided with
      the window's playback: a camera sends one recording at a time and the
      relay gives it to the newest request, so a fetch and window playback
      of the same camera cut each other off. Run fetches while the window
      is not playing back those cameras.
- [ ] **Engine crash under a burst of stream additions**: go2rtc 1.9.14
      died with "concurrent map writes" when seven playbacks were added at
      once; additions and removals now go one at a time. If it recurs, the
      window should notice a dead engine (`Engine::alive`) and restart it.
- [ ] **Recording lists in a burst**: asking seven cameras at once made two
      answer with a device error (-71101); the window now asks one at a
      time with a second try.
- [ ] **Xiaomi camera** (Mi Home, model unknown): it answers the miIO hello
      on the LAN (device id only, no model) and advertises nothing over
      mDNS or SSDP, so the model and the local token come only from the Mi
      cloud with the Mi account. Mi Home cameras stream over Xiaomi's own
      P2P service, not RTSP; which models have a usable path depends on the
      model. Next: a `kagaz xiaomi login` that lists the account's devices
      (model, token), then decide per model.
- [ ] **Windows and macOS ffmpeg checksums** are pinned to the builds
      downloaded on 2026-10-07; the Windows zip (gyan.dev) and macOS zip
      (evermeet) extraction paths are untested on those systems.
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
