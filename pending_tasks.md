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

## OneTouch doorbells

- [x] **Remember sign-in securely and offer Reconnect** (2026-10-14): desktop
      credentials use the OS vault. Reconnect requires a click, sign-out keeps
      the remembered account, and Forget saved sign-in removes it. Synthetic
      Linux vault and UI checks pass; real-account reconnect awaits verification.
- [x] **Discovery fallback** (2026-10-14): try the APK's backup redirect service
      after network/temporary HTTP failures. Preserve API versions and validate
      the returned gateway. Primary service availability remains external.
- [ ] **Smart Cloud account and real media validation** (2026-10-13): account
      sign-in, device list, live RTSP adapter, cloud snapshot/video history,
      token refresh and logout are implemented and locally tested. User reports
      snapshots only in the working OneTouch Smart Cloud app. Desktop sign-in
      and device discovery are verified. Native playback exposed stale H.264
      configuration and damaged UDP frames; fixes now reorder packets and drop
      incomplete pictures. The actual Kagaz window ran for over three minutes
      with over 4,000 rendered frames and no playback error or restart.
      The user confirmed continuous playback on 2026-10-14. Cloud history currently returns an empty list. Verify live video, snapshots,
      expired-token refresh, and logout/reopen after the remaining fixes.
- [ ] **Legacy OTIPVDP account validation** (2026-10-13): separate service
      selection, device-MAC discovery, plain/encrypted login and legacy history
      formats are implemented. Needs an unmigrated account for a real check.
- [ ] **OneTouch two-factor accounts and Indoor-monitor recordings**: not supported
      by this implementation. Existing direct SIP calls remain separate.

## Ceiling fans (Atomberg)

- [x] **Bluetooth from the computer** (2026-10-21): scan, state read and a
      command verified on a Renesa (R1) with the CLI; the window's page is
      written (rooms, names, remembered fans, Found nearby, controls) and
      awaits a look in the window.
- [x] **Decode on, speed and Boost from the real fans** (2026-10-21):
      captured on an Aris with its remote. Bit 0x10 is on; speeds 1 to 5;
      Boost is speed 6 with bit 0x40, and 0x40 stays set after turning off
      from Boost. Fields 11 and 13 count seconds running and switched off,
      shown on the fan's menu. Fixtures in `fans/atomberg.rs`.
- [x] **A fan takes only so many new connections** (2026-10-21). After
      many connections a fan stops listing services, then refuses to
      connect, until switched off at the wall. Kagaz keeps one connection
      per fan while in use; a 20-command console session on one connection
      left the Aris answering normally.
- [x] **Commands checked on the Aris** (2026-10-21): power, speeds, Boost
      on/off, light, colours, brightness 50/100. Sleep and timer are taken
      and reported back.
- [x] **The timer switches the fan off** (2026-10-22): a 1-hour timer set
      from the window switched an Aris off an hour later. The fan reports
      only whole hours left, so Kagaz keeps the moment the timer ends and
      shows a countdown on the card and on Home; when it is up, the page
      asks the fan how it is (or does so when next opened). A timer set with
      a remote shows as an estimate ("about").
- [ ] **What Sleep does over time** (the timer part is done, above): both are taken by the Aris
      and the Efficio (the Efficio blinks its indicator to confirm), but the
      state shows whole hours left with no countdown, and the known timer
      takes only 1, 2, 3 or 6 h. The Efficio's remote has Sleep and Timer
      buttons (the Aris's has neither): set them with that remote and read
      the state, to learn the remote's timer lengths. Then set "Turn off
      after: 1 h" and see the fan switch off an hour later; leave Sleep on
      overnight.
- [x] **Brightness levels** (2026-10-21): the remote steps 10, 25, …, but
      the fan stores any whole value from 10 to 100 and shows a change of
      2 or more (30 to 31 was too close to tell). The card's slider moves
      in single steps.
- [x] **Short timer** (2026-10-21): none known. Timer index 5 (outside the
      known 0 to 4) cancels the timer, as 0 does; the app's analysis may
      still show another way.
- [ ] **Renesa**: needs a power cycle at the wall before Kagaz can reach it.
- [ ] **Read the Atomberg phone app** (later, as asked 2026-10-21): every
      command it can send, to learn whether the firmware has a finer speed
      control than 1 to 5 + Boost, or a smooth white-to-yellow light, and
      how the app itself connects (one connection kept, or one per
      command). Then confirm on the fan with the console.
- [x] **Light read from the real fan** (2026-10-21): on/off, three colours
      (warm, white, natural white) and brightness 10 to 100 decoded from an
      Aris set with its remote; the card shows colour and brightness.
- [ ] **Light, sleep, timer, brightness on real fans.** Written from the
      community's notes; only power and speed are exercised so far. Check
      an Aris (I1, dimmable, colour modes) and an Efficio (M1).
- [ ] **Push instead of read**: the command characteristic allows notify
      (checked on an Efficio, 2026-10-22). Subscribing would let a fan tell
      Kagaz when its remote changes it, without reading on demand.
- [ ] **How a fan gets onto Wi-Fi** (2026-10-22): an unconfigured fan sends
      nothing on Wi-Fi (no network of its own, no broadcasts) and nothing
      over classic Bluetooth; its Wi-Fi radio only scans (it lists the
      networks it sees over Bluetooth). Setup goes through the Wi-Fi
      characteristic, encrypted per the community notes; one more
      characteristic, undocumented, reads "2". Either put a fan on Wi-Fi
      with the Atomberg app, or learn the setup format from the app's code.
- [ ] **Wi-Fi route.** No fan here is on the network. The broadcast
      listener (UDP 5625) and the command path (UDP 5600) are written from
      the community's notes; put a fan on Wi-Fi with the Atomberg app and
      run `kagaz fans` to verify, then record a datagram as a fixture.
- [ ] **Bluetooth on the phone** (written 2026-10-22, not yet tried on a
      phone): `KagazBle.kt` scans and keeps one GATT connection per fan,
      reached from Rust through `KagazNative` (`fans/phone.rs` stands in
      for `ble.rs` on Android). It asks for Nearby devices (location before
      Android 12) and to turn Bluetooth on when needed. Check on a phone:
      the permission question, finding the fans, every control, and that
      a write as long as `{"light_mode":"daylight"}` goes through (the
      connection asks for a larger MTU first).
- [ ] **Connection takes a few seconds.** BlueZ refuses the first connect
      now and then ("abort-by-local"); Kagaz rescans and tries again, so a
      command takes 3 to 10 s. Keeping a connection open while the page
      is on screen would make it instant.

## More devices to connect (as of 2026-10-22)

- [ ] **Orient ceiling fan.** It announces nothing over Bluetooth. Orient's
      smart fans are set up with the Orient Smart app, which appears to run
      on the Tuya platform (unconfirmed). Tuya devices announce themselves on
      the local network every few seconds, encrypted with a per-device key.
      Several such devices are on the test network; find out which one is
      the fan (or whether it has no smart features at all).
- [ ] **A fan with Bluetooth and speakers, maker unknown.** Identify the
      maker and model first (label on the fan or remote, the app it pairs
      with, the name it advertises), then see how it is reached.
- [ ] **Smart switches.** Probably the other Tuya devices on the test
      network; one Tuya route in Kagaz could cover them, the Orient fan and
      the robot vacuums on the roadmap.

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

- [x] **The navigation, put in order** (asked, approved and built
      2026-11-18; mockup in `design/mockups/navigation/`): only the
      kinds of device the person has added (Home says which, by the same
      rule as its own groups), by the name each shows; Home on top;
      Tools and Settings at the foot; Add a device at the end of the
      person's kinds; the kinds scroll by themselves between the two. A
      kind's page that is open has its entry even before its first
      device. A phone's bar stays Home, Tools, Settings (the owner's
      choice of the two drawn). Home's groups are by name too. The
      entries' icons are green on Ubuntu and Windows as on macOS (the
      owner). Checked in the headless page on every system's look; not
      yet seen in the real window by a person.
- [x] **One name for a kind everywhere** (the owner, 2026-11-18): Air
      purifiers, Computers & phones, Doorbell, Fans, Printers &
      scanners, Security cameras (kept apart from the Photo cameras to
      come), Society gate: in the sidebar, on Home, in Settings, as page
      titles and in Add a device. Under an icon (Android's rail) a short
      word stands for the longer names.
- [x] **Settings, first step: one door** (2026-11-18): the Settings page
      lists each kind the person has, by name, and a row opens what that
      kind's gear opens (or its page, where it has no settings of its
      own yet). **Still to move, kind by kind**, as in
      `design/mockups/settings/`: accounts gathered under Settings ›
      Accounts with sign in and out there (Xiaomi is not listed yet); a
      kind's setting for all its devices with each device's own
      ("Same as all") for computers' keys, sound and size; the cameras'
      playback method and tiles in a row remembered and on a page of
      their own; "Ring me here" in one place; a Brother's Scan button
      under Printers & scanners; the field that finds a setting.

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
- [x] **Xiaomi cameras** (2026-10-22): sign-in by texted code, password or
      QR code (window and `kagaz xiaomi`); live view, sound and SD-card
      recordings spoken natively (CS2 + miss), so the phone gets them too;
      grouped by brand. Checked live on a CS2 camera.
- [ ] **Xiaomi, still open**: talk-back is built but not yet tried on the
      camera; the phone app not yet tried on a phone; Xiaomi's own sign-in
      page in a window (when it demands one) untested; cameras on TUTK
      (older firmware) or sending H.265 are not spoken yet; saving footage
      and playback speed are Tapo's only; the card's list is asked every
      ten minutes, so a shorter gap may not show. Viewing with no internet
      would need replacement firmware (thingino: the case opened and the
      flash chip reprogrammed) and is not planned.
- [x] **A camera's own screen** (2026-10-24, after the Xiaomi Home app):
      the list is cards (picture, name, ⋮); a tap opens the camera with
      sound, quality and full screen over the picture, Talk, Screenshot,
      Record and Recordings under it, and its day on a ruler under a fixed
      line (pinch to zoom, day by day). Sideways on a phone fills the
      screen. Recording copies the camera's H.264 into an MP4 with its
      sound ('sowt' PCM), checked with ffmpeg on a real Xiaomi recording.
      Files go to a folder chosen in Settings (Android's folder picker on a
      phone), else Downloads/Kagaz. The list's "Playback" for every camera
      stays.
- [ ] **Camera screen, still open**: the desktop layout is the phone's
      widened (to be designed next); not yet tried on a phone; recording
      works only in Kagaz's own player (not the helper modes); an MP4's
      'sowt' sound plays in VLC, QuickTime and Android's players but not in
      a web browser; the timeline has no thumbnails.
- [ ] **Camera engine restart** (`cameras_restart`, after an account is
      added or removed) leaves the old local server's listening threads
      idle; harmless, but a restart per sign-in adds a few.
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
      under tapo-analysis/ (git-ignored) in case TP-Link changes the relay.

## Computers & phones (remote connection)

Plan: `docs/remote-connection.md` (agreed 2026-11-16).

- [ ] **Step 0: the RDP library on a real Windows computer.** Resizing to
      the window, sound, clipboard, files, monitor choice.
- [x] **Step 0: mockups** of the device list, Add and the viewer
      (`design/mockups/computers-phones/`, approved 2026-11-16).
- [x] **Step 1, reaching** (2026-11-16): the saved list
      (`remote.toml`), every path tried at once, what a host speaks, the
      certificate a Remote Desktop computer shows, the look over the local
      network, Tailscale's list, and `kagaz remote` (list, add, nearby,
      paths, path, rename, forget, ssh, copy). Checked against a test
      server here; not yet against a real Windows computer, a phone's VNC
      server or over a hotspot. The gateway is read on Linux only.
- [x] **Step 1, the window's page** (2026-11-16): the list with how each
      device does, Found nearby (this network and Tailscale), Add, and a
      device's page with its paths, as in
      `design/mockups/computers-phones/`; on Home and in Add a device.
      Checked in a browser with a stand-in backend (Ubuntu, Windows, macOS,
      Android and iPhone looks), not yet looked at in the real window.
      Connect, the screen settings and the password come with the viewer.
- [x] **Step 1, VNC sessions and the viewer** (2026-11-17): VNC written
      in Kagaz (Raw, CopyRect, RRE, Hextile, ZRLE, Tight with JPEG; the
      pointer's shape; a changed screen size; clipboard text in any
      script; the delay measured), the viewer in the window (picture,
      mouse, wheel, keys, send keys, clipboard on Linux, sharper or
      smoother, own size, full screen, left kept left), `kagaz remote
      open`, `try`, `clip` and `pattern`. Checked against Kagaz's own test
      card (`kagaz remote pattern`) in the tests and in the real window on
      Ubuntu (1280x720 moving 30 times a second: every update shown, under
      1 ms to paint one). **Not yet** against a phone's or a computer's
      VNC server, over Tailscale or a hotspot.
- [ ] **Step 1, to check on real devices**: a phone's VNC server (the
      keyboard there, clipboard, turning the phone), a Mac's Screen
      Sharing, how the automatic picture setting behaves on a slow link
      (it goes by the measured delay alone).
- [x] **Step 1, Remote Desktop sessions, the base** (2026-11-17):
      IronRDP's newest parts under Kagaz's own session (one thread, woken
      by input): the certificate compared with the one remembered before
      the sign-in is sent, and a changed one is no longer taken on
      silently by a check either; network-level sign-in with the user
      name and password; the picture as changed areas; the pointer's
      shape; mouse, wheel and keys by their scancodes (letters without a
      key as Unicode); left kept left; the screen drawn at the viewer's
      size and scale at the start and again when the window changes
      (Display Control), or a fixed 1920x1080 when asked; the delay read
      from the connection (Linux). In the window: the user name on the
      sign-in card, the certificate card, the size choice on the device's
      page. In `kagaz remote try`. Tested against `tools/rdp-test-server`
      (the library's server part showing the test card; a program of its
      own because it cannot share a build with the newest client parts).
      In the real window against the test server since 2026-11-17
      (signed in, drawn to fit). **On the owner's real Windows PC the
      same day**: signed in, picture and typing right, full screen sharp,
      left kept left as meant; about 10 to 19 updates a second while
      things moved, 28 to 50 ms delay on the local network. Not yet
      GNOME. Computers only: phones come with step 2.
- [x] **Step 1, the viewer after the first real session** (2026-11-17):
      its bar is Kagaz's green (the owner's ask: told apart from the
      device's screen under it); a call at the door, or any other page,
      no longer leaves the viewer's bar and full-window layout behind:
      the session goes on, its bar steps aside and Computers leads back.
      The log now says the size asked for and the size the device took.
- [x] **Heard from the owner on the real Windows PC** (2026-11-18):
      a window made smaller and larger is redrawn well; the newer
      picture "feels high res". Its log: version 10.7 agreed, the text
      codec and the progressive one, no H.264 (Windows uses that for
      all of the screen only by policy). The delay shown had swollen to
      95 to 400 ms on the home network: it was the connection's running
      average, which grows while Windows holds its acknowledgements
      back; the lowest of the last minutes is shown now.
- [x] **The sound on the owner's real Windows PC** (2026-11-18): it
      played once Kagaz also offered the channel for redirected devices
      (with no device on it): Windows starts its sound only for a viewer
      that offers both. It came by the older channel as mono ADPCM, 22
      kB/s, Windows' thinnest, and "lagged a lot" (the owner; which lag
      is asked). Since then: on a link that answers within 20 ms only
      full sound (plain, stereo, 44100 or more) is taken where the
      computer has it; the computer is told how long each block was
      held; the log says how far ahead of the speakers the sound is
      (about 170 ms on this laptop with the test server's tone: the
      player's 100 ms and the system's own).
- [x] **Keys and buttons that belong to the device** (2026-11-18): full
      screen is the window's own, so Esc reaches the device; the mouse's
      back and forward buttons go to a Remote Desktop computer and no
      longer step back in Kagaz (which ended the session); Caps Lock,
      Num Lock and Scroll Lock are told to it as they are here. Not yet
      tried by a person.
- [x] **Pictures on the clipboard over Remote Desktop** (2026-11-18,
      the owner's ask): one copied on the computer (as PNG where it has
      it so, else from Windows' own form) is put on this clipboard as
      PNG, with the text copied along with it; one copied here is told
      to the computer and handed over when pasted there, as PNG or in
      Windows' form. The picture last seen, from either side, is not
      said again, so the computer's own never goes back to it. Tested
      against the test server (`copy-picture`, `paste-picture`). **Not
      yet** on the real PC, and the part on this system's clipboard was
      not run at all: a test of it would overwrite what the owner has
      copied.
- [x] **The sound stuttered: Kagaz's own player** (2026-11-18; the
      owner: "stuttering"). The night's log showed little work on the
      picture (under 70 ms a second) and the sound coming in long blocks
      (fewer than 50 in 25 s, so a third of a second or more each). The
      library's player steers how full its store is on the whole to 0.1
      s; with long blocks that average is high while the store runs
      empty before each block, so it kept playing faster until it ran
      dry: in a replay, a gap a second with quarter-second blocks, and a
      third of the sound thrown away with half-second ones. `rdp/play.rs`
      replaces it (cpal and ringbuf directly, libopus for Opus; the
      library's player is no longer used): it watches the lowest the
      store falls over three seconds and steers that to a reserve of 60
      ms, one part in a hundred faster or slower; the reserve doubles
      after a gap (to 480 ms at most) and comes down after a calm
      minute; what piles up in a stall is let go; a sound that ends is
      not a gap; gaps come in and go out softly. Tests replay blocks of
      20 ms to 1 s, a jumpy link, a stall. Through the real window
      against the test server, on a silent test output (nothing on the
      speakers): no gaps with 20 ms and 250 ms blocks, and the system's
      own queue down from about 100 ms to 30 ms (short turns asked of
      it). On a slower link the computer is now asked to suit the sound
      to the link (Remote Desktop's "dynamic" quality). The log says the
      longest block, gaps, the reserve and how far ahead of the speakers
      the sound is. **Not yet** heard on the real PC.
- [x] **This computer's own keys** (2026-11-18, the owner's choices):
      Print Screen, the Windows key, Alt + Tab and the like go to one
      side only. A choice per device (its page, Keyboard; the viewer's
      key menu): to the device in full screen only (default), whenever
      the pointer is on Kagaz while it is in front, or never. The window
      takes the keyboard (a GTK seat grab of the keyboard alone, which
      on Wayland asks the system to leave its shortcuts to the window;
      the system asks the person the first time). On Wayland nothing
      changes while the window is out of focus, so that the system's own
      question does not withdraw itself. Ctrl + Esc leaves full screen
      and does not reach the device. Closing the window with a session
      open asks first. Checked in the headless page (which keys go
      where, the choice saved, the question). **Not yet** tried in the
      real window: the grab itself, GNOME's question, whether the
      laptop's volume and brightness keys still act here in full screen.
- [x] **A pinch zoomed Kagaz, not the PDF on the computer** (2026-11-18,
      the owner on the real PC): the webview's own pinch-to-zoom enlarged
      Kagaz's page, the screen's area in it changed with every move of
      the fingers, and the computer redrew its screen each time. While a
      device's screen shows, the webview's pinch is off (WebKitGTK keeps
      it on its widget as `wk-view-zoom-gesture`; its phase is set to
      none) and Kagaz's own tells the page how far the fingers have
      spread; that goes to the device as Ctrl + wheel at the pointer, a
      notch for every eighth. On Kagaz's other pages the pinch is the
      webview's again. Checked: the page's part in the headless page;
      in the real window (test card) the webview's pinch is found and
      turned off. **Not yet** tried with fingers.
- [ ] **Kagaz on the computer being used: the Windows side, next**
      (the owner, 2026-11-18: an office laptop the owner teaches the
      team on from afar, while they go on using it; Windows' Remote
      Desktop gives the screen to one person, and a service that sends
      the screen through its maker's servers is not trusted). Moved
      ahead of the phone's viewer in `docs/remote-connection.md` (now
      step 2). Answers in (every choice a setting: ask each time or the
      PIN, start with Windows off by default, three ways for the
      sign-in screen and administrator prompts including an optional
      helper service, watch or control from either end); mockups in
      `design/mockups/windows-host/`, approved with changes: shorter
      words, off until "Share this computer" is chosen (nothing runs or
      listens before), several devices at once. The host speaks Remote
      Desktop on a port of its own. First tried on the Windows PC the
      owner already reaches by Remote Desktop. Its port and networks
      are a setting too (3390 unless changed, or 3389 with Windows'
      own off; local network and Tailscale, Tailscale only, or this
      computer only). **Step 1 built** (`crates/kagaz-share`, its own
      program; `dist/kagaz-share-windows-x64.exe`): the main screen
      through DXGI (GDI when that fails) with this computer's pointer
      drawn in, H.264 through the graphics pipeline, mouse and keys by
      SendInput (keys by their place on the keyboard; Caps, Num and
      Scroll Lock follow the viewer), `--watch`, `--pattern` for the
      test card. Tried here with the test card through `kagaz remote
      try` (1080p, 30 a second, 13.6 ms to encode each); **not yet run
      on Windows**. Known gaps: one viewer at a time; the library turns the middle
      button and the sideways wheel of a standard mouse message into
      nothing; no sign or pairing yet. **Step 2 built** (2026-11-19):
      the speakers' sound through WASAPI's loopback (16-bit stereo,
      48000 or 44100 as the speakers play, blocks of 20 ms), the
      clipboard both ways through a hidden window (text, pictures in
      Windows' form and PNG; what came from the viewer is not offered
      back), files sent from the viewer into
      `%TEMP%\kagaz\received\<time>\` and onto the clipboard to paste
      (folders older than a day let go); `--no-sound`, `--no-clipboard`,
      `--no-files`; watching takes nothing from the viewer. Tried here
      on the test card: text (24 characters) and two files (3 MB, sums
      equal) through `kagaz remote clip` / `send`, and the made tone
      through the new `kagaz remote try --sound` to a silent output (303
      blocks, no gaps). Files copied on the computer are not offered:
      the viewer takes no files from a computer yet. **Step 3 built**
      (2026-11-19): pairing (`remote/link.rs`, built by both programs:
      SPAKE2 with the code, proofs both ways, three words, the secret
      derived on both sides; `remote/pair.rs`; `kagaz remote pair`; Add a
      device › A computer running Kagaz; the secret in
      `remote-secrets.toml`, 0600, never in the list the page sees),
      sign-in by TLS + a validator over the paired list (`paired.rs`:
      PIN as a slow hash, five wrong stop PINs five minutes, the person
      asked in Windows' own small question or on the input with
      `--ask-on-input`, each device's permissions enforced on input,
      clipboard, files and sound), a PIN card and "Asking …" in the
      viewer. Tried here on the test card: pairing (same words both
      sides), right PIN, wrong PIN (turned away, and said so), no PIN
      (asked), forgetting removes the secret. The library sends its
      refusal in a form the viewer's newer library cannot read; the
      viewer says "turned away" for a session that breaks before any
      picture. **Step 4 built** (2026-11-19): `apps/desktop/src/share.rs`
      runs kagaz-share `--driven` (found next to Kagaz; `@`-JSON events;
      settings in `share.json`) only while sharing is on, resumes it at
      start only if left on, and it exits when Kagaz goes; `ui/share.js`
      (This computer: switch, who may connect, PIN, paired devices, pair
      with code and addresses, lock after, record, past sessions, port
      and networks), `share-ask.html` and `share-sign.html` (small
      windows on top: the question; the sign with Watch only / Let it
      control / End), Control and Watch in the viewer's bar for paired
      computers, "Asking…" kept until the first picture. kagaz-share:
      `watch`, `control`, `end`, `devices`, `lock`, `--bind`, `--lock`,
      `sessions.jsonl`, low-level hooks counting the person's own input
      (for "lock if nobody used it"). Tried here with Kagaz as the
      sharing side on the test card: started from its settings, a device
      in with the PIN, asked without it (the owner answered in the real
      window and used both sign buttons), stopped with Kagaz. Not built
      yet from the mockup: the QR code, blanking the screen, several
      devices at once, the viewer's bar following a switch made on the
      computer. Menu choices shortened and the second key mode renamed
      "Window active, pointer inside" (the owner, 2026-11-19). **For the
      owner to try, when ready** (put off 2026-11-18):
      1. send `dist/kagaz-desktop-windows-x64.exe` and
         `dist/kagaz-share-windows-x64.exe` to one folder on the PC with
         Kagaz's Send a file, and open Kagaz there; 2. Add a device ›
         Computers & phones › Set up sharing: turn Share this computer
         on (allow it on private networks if the firewall asks) and set
         a PIN; 3. Pair a device there; here Add a device › Computers &
         phones › A computer running Kagaz (its address and the code
         shown there); compare the words, They match here, Pair there;
         4. hand the session to the PC's own screen from an administrator
         Command Prompt:
         `for /f "skip=1 tokens=3" %s in ('query user %USERNAME%') do %windir%\System32\tscon.exe %s /dest:console`
         (the PC's monitor then shows the desktop unlocked); 5. connect
         here: with the PIN, or Ask someone there; 6. try mouse, keys,
         scroll, Num Lock, a video, Control and Watch, the sign's buttons;
         copy text and a picture each way, send a file and paste it in a
         folder there, play something (`kagaz remote try "<name>" --sound`
         prints how its sound arrives); 7. turn sharing off and go back to
         Windows' own Remote Desktop. **Step 5 built** (2026-11-19, the
         owner's answers: while sharing is on, closing Kagaz's window
         leaves it in the tray; the helper reaches the sign-in screen
         after a restart too; names `KagazHelper`, "Kagaz helper",
         `C:\ProgramData\Kagaz`): Start with Windows (HKCU Run entry
         `"<Kagaz>" --tray`, only while sharing is on and chosen; off by
         default), the tray (Open Kagaz, Stop sharing, Quit) only while
         sharing is on, the window hiding to it on close; "Sign-in screen
         and administrator prompts": someone here / Windows' Remote
         Desktop (says whether it is on, opens Windows' page) / Kagaz's
         helper service, installed and removed through Windows'
         administrator question (`kagaz-share helper install|remove`,
         sc.exe, icacls; the paired devices, certificate and record move
         to the helper's folder and back). The service runs
         `kagaz-share --system` as the system in the console session,
         following Windows between the person's desktop and the secure
         one; Kagaz steers it over a token-protected connection on this
         computer (control.rs); with nobody steering, Windows' own small
         question asks, so before sign-in only devices allowed in with
         the PIN without asking get in. Tried here with the test card
         and a stand-in for the service: pairing, the PIN, the record,
         a change of port restarting it, turning it off; nobody steering
         turned a device without the PIN away. **Not yet run on Windows
         at all** (service, desktop switching, tray, Run entry). Not done:
         Ctrl + Alt + Del at the sign-in screen (Windows lets a service
         send it only under a policy), and a helper left at an older
         version after Kagaz is updated (choose it again to update it).
         **Step 5 on the PC, after the steps above:** 8. turn on Start
         with Windows, sign out and in: Kagaz should wait in the tray,
         sharing; close its window: still in the tray; 9. choose Kagaz's
         helper service, answer Windows' question, connect from here;
         press the Windows key + L there: the lock screen should show
         here and take the PIN or password typed here; open something
         that asks for an administrator: the prompt should show and be
         answerable; restart the PC and connect before signing in;
         10. choose "Someone here answers them" again: the helper and
         `C:\ProgramData\Kagaz` should be gone, the paired devices kept.
         Until
      it exists: a VNC server on that laptop, reached over Tailscale,
      with today's Kagaz (not yet tried against a real Windows VNC
      server).
- [x] **Settings from a session's own window** (the owner, 2026-11-18:
      change a setting without leaving the screen): a gear in the green
      bar opens a sheet over the screen's right side (the device does
      not redraw for it) with the device's own settings (keyboard, screen
      and sound, mouse, sign-in; "Kept for this device only") and every
      session window's, and a way to its page for addresses and the rest.
      Drawn by `remote-settings.js`, the same code as its page in the
      main window; a change in either reaches the other
      (`remote-device-settings`), and where the keys go changes at once.
      Answered the same day: settings are remembered per device already;
      the planned "same as all / its own" defaults (design/mockups/settings)
      come with the settings move. Checked in the headless page and the
      real window's log on the test card; not yet clicked by a person.
- [x] **Num Lock came out the other way round on the computer** (the
      owner on the real PC, 2026-11-18): WebKitGTK tells a page about
      Caps Lock only and says "off" for Num Lock and Scroll Lock
      whatever they are, so the computer was told "Num Lock off" at the
      start of every session; with it on here, the two then stayed
      apart. The system is asked instead (`remote_locks`, GDK's keymap)
      when the session opens, when the person comes back to the screen
      and after a lock key is let go; the log says what was told.
      Checked in the headless page; **not yet** on the real PC.
- [x] **Heard from the owner on the real Windows PC** (2026-11-18,
      evening): the sound "works very well" and everything is "buttery
      smooth" with Kagaz's own player.
- [x] **Session windows, built** (2026-11-18, the owner: "proceed as
      recommended"): `remote_windows.rs` (a registry of the windows,
      opening one per Connect, the two closing questions, keeping them
      open with the main window out of sight), `session.html` and
      `session.js` (the viewer alone, the house button to the main
      window, the ring strip over the screen), the device list ("In its
      own window", Show window, the question before another), the gear
      of Computers & phones (Session windows: what closing the main
      window does, the ring in a session's window; Each device), stored
      in remote.toml under `[windows]`. This computer's keys and the
      pinch are each window's own. A second connection is never
      refused by Kagaz: the device decides (Windows Server allows
      several, the home editions one). Checked: both pages in the
      headless browser, and the real app opens a test card in a window
      of its own. **Not yet** done by a person: the closing questions,
      a real ring's strip, keeping windows open, two real devices at
      once.
- [x] **Several sessions at once, each in a window of its own** (the
      owner, 2026-11-18): today one session takes over Kagaz's window.
      Asked for: every connection in its own window, closing it ends
      that session (after a question), the main window free meanwhile
      for the rest of Kagaz (a call at the door, a TV's remote). The
      owner's answers: closing the main window ends every session after
      one question, unless Settings says the session windows stay;
      Connect on a device that has a window asks (show that window, or
      connect again: a Windows computer has one screen per account, so
      the first window loses it; checked 2026-11-18); a call at the
      door shows as a strip in the session's window, with a setting to
      turn it off. Mockup in `design/mockups/session-windows/`, waiting
      for the owner's word. On phones: Android can show each session as
      its own entry among the recent apps (a native screen per session,
      to be built with the phone's viewer, step 2); an iPhone has one
      window per app, so there a switcher inside Kagaz.
- [ ] **Settings, put in order** (the owner, 2026-11-18: "simple,
      minimal and sorted and still customizable"): a setting lives with
      the smallest thing it changes (device, kind, all of Kagaz);
      Settings is the one door to all of them (Kagaz's own, then each
      kind the person has, by name; a kind's row is its gear's page);
      a kind's setting holds for all its devices, a device says "Same
      as all" until given its own; a field finds any setting. Mockup in
      `design/mockups/settings/`, waiting for the owner's word.
- [ ] **To hear from the owner on the real Windows PC**: a pinch on a
      PDF there; the sound now
      (gaps, how late), and the log's line on it; this computer's keys
      in full screen and in a window; a picture copied each way; sending
      files; the certificate card; Remember typed in the window.
- [x] **Step 1, Remote Desktop, clipboard text** both ways, in any
      script (2026-11-17): each side says it has text and hands it over
      when asked; Kagaz asks at once, so what is copied on the computer
      is on this device's clipboard without a further step. Tested
      against the test server, not yet against real Windows.
- [ ] **Sending files, four ways** (the owner, 2026-11-19: three .exe
      files offered by the clipboard never arrived on the real PC; Ctrl +
      V in a folder there did nothing; Kagaz's log shows "files offered
      3, 52847616 bytes" and nothing after, so Windows never asked for a
      byte. Not explained yet: the viewer now logs each clipboard step
      while files are on offer (the list taken or refused, kept, size or
      pieces asked, pushed aside by a copy on either side). Built, **not
      yet tried on the PC**: a setting per device, "Files sent from
      here": (1) it asks where to save them (default): the files are put
      in the session's own folder (`~/.cache/kagaz/sent/…`, gone with the
      session), shown there as the drive `\\tsclient\Kagaz` once
      something is sent (IronRDP's filesystem backend, two folders routed
      by drive), and Kagaz types Windows + R and a line starting
      `\\tsclient\Kagaz\.kagaz\save.ps1` (Windows' folder chooser, then
      Windows' own copy with progress, then Explorer on the result);
      (2) their folder opens there; (3) on its clipboard (says it waits
      until pasted); (4) over SSH (system scp, the session's account and
      password through a private askpass helper, Downloads unless said).
      "Share a folder with it": a folder of this computer as a drive
      there, both ways, from the next connection. A computer running
      Kagaz: files into its person's Downloads, "N files in Downloads" on
      the sign. **To try on the PC:** send two files and a folder with
      each way; the dialog should open on the PC within a few seconds;
      press nothing there while Kagaz types into the Run box; share a
      folder, reconnect, open This PC there. If the clipboard way still
      does nothing, send the `files:` lines from Kagaz's log
      (`~/.cache/kagaz/cameras.log`).
- [x] **Step 1, Remote Desktop, sending files** (2026-11-17): by the
      computer's clipboard, as Windows' own viewer does. Kagaz says which
      files there are (names made fit for Windows, sizes, when last
      changed; never where they are kept); the computer reads them piece
      by piece when they are pasted there, and Kagaz tells how far each
      has got. An offer the computer asked to keep stays to be had while
      something else is copied; text already on the clipboard here does
      not push the files aside, text the person asks to send does. In
      the window: the Send files button and its notes. `kagaz remote
      send` opens the window with the files on offer (pasting takes a
      person), or stays in the terminal with `--here`. Tested against
      the test server (which "pastes" on command and says the checksum
      of what arrived), from the command line and in the real window
      started by `kagaz remote send`. **Not yet** against real Windows
      or GNOME; the file chooser behind the button was not pressed by
      anyone yet. Whole folders are refused with a reason. Files from
      the computer to here are not built (the library carries them).
- [x] **Step 1, Remote Desktop, the newer picture with H.264**
      (2026-11-17): the library's graphics pipeline channel (H.264 in
      both its forms, the text codec, RemoteFX and its progressive form,
      planar; frames acknowledged) under Kagaz's session, with OpenH264
      built in from source, the owner's choice. The picture's size
      follows the pipeline's own word. "Use the older picture" on a
      computer's page turns it off for that computer. The log and
      `kagaz remote try` say how the picture came and how much work it
      was. The test server sends the card as H.264 (`--h264`, encoded by
      OpenH264 with Remote Desktop's colour rule) and tells each
      acknowledgement. Measured against it at 1920x1080, full frames:
      about 19 ms of work a frame here, 0.15 Mbit/s where the older
      picture took megabytes. **Not yet** against real Windows, which
      chooses its codecs itself (H.264 for all of the screen only when
      its policy says so; else H.264 for video and the others for the
      rest): the log will tell which came. OpenH264's fast routines
      need `nasm` when Kagaz is built (it falls back to plain C without
      saying so): on the building computer it is unpacked without root,
      outside the system, and found through cargo's `NASM` setting,
      since 2026-11-18. Most of a full frame's work turned out to be
      moving the picture, not decoding it: one copy on Kagaz's side was
      taken out.
- [x] **Choosing a monitor** dropped for Remote Desktop (the owner,
      2026-11-17): the computer makes one screen for the viewer. The
      button shows only for a kind of session that has monitors, which
      none has yet.
- [x] **Step 1, Remote Desktop, the computer's sound** (2026-11-18):
      the library's sound channel and its player (through cpal to the
      system's own sound; plain, ADPCM and Opus, the last by libopus
      built in from source, not on Windows where that build does not
      cross). It plays here instead of on the computer; "Play its sound
      here" on the computer's page turns the channel off for it; the
      speaker button turns it off for the moment; it waits while another
      page shows; a session in the terminal plays none. The log tells how
      much sound arrived. The test server sends a made tone (`--sound`,
      `tone <ms>`); in the tests nothing is played and every byte is
      counted. In the real window against the test server the system
      showed Kagaz's stream playing. **On the owner's real Windows PC
      (2026-11-18) no sound came**, though Windows had switched to
      "Remote Audio": not a byte arrived on the older channel. Windows
      since 8 sends its sound through dynamic channels where the viewer
      has them, so Kagaz now listens on those as well (the library's
      own; there is no test peer for them here), and the log says which
      channel and format the sound came by, or how far the two got.
      **To hear from the owner.**
      The build needs ALSA's development files (unpacked without root on
      the building computer, found through `PKG_CONFIG_PATH`).
- [x] **The test server no longer says the text a viewer copied**
      unless asked with `--say-text`, which only the tests do
      (2026-11-18): the real window offers whatever is on its person's
      clipboard, and in a real-window run a line of the owner's
      clipboard had reached the test server's output.
- [ ] **Step 1, Remote Desktop, what is left**: whole folders, and files
      from the computer to here; a sign-in through a company domain (Kerberos) is refused
      with a reason; decoding on a thread of its own if a real computer
      shows it holding up the keys; the "Third-party licences" page in
      About, now owed for OpenH264's notice as well.
- [x] **Step 1, the password in the system's password store** behind a
      Remember switch (2026-11-17): one entry per device under
      `app.kagaz.computers` (the owner's name for it), kept once the
      device accepted it, forgotten with the switch or the device. The
      store's round trip is tested on Ubuntu, and the real window signed
      in to the test server with a remembered password (2026-11-17);
      typing one and switching Remember on in the real window is not
      tried yet. The command line still asks each time.
- [ ] **Step 1, smaller things left in the viewer**: zoom beyond fit and
      own size; VNC over TLS (VeNCrypt), which some Linux VNC servers
      offer alone; the clipboard on Windows and macOS (comes with their
      viewers in step 2); a "Third-party licences" page in About.

## TVs & displays

Plan in [docs/tv-displays.md](docs/tv-displays.md) (2026-11-18): mirroring
first, then sending files, the remote, and extending. Nothing built yet.

- [x] **Step 0: Miracast to the LG, both routes** (2026-11-18): over the
      network (MS-MICE) and over Wi-Fi Direct (NetworkManager, no root, home
      Wi-Fi kept up), as far as the TV's RTSP answer; no picture sent.
- [ ] **Step 0: Wi-Fi Direct still to try**: the LG with its Wi-Fi
      forgotten (on no network), this laptop on 5 GHz, the Windows PC.
- [ ] **Step 0: what else the LG accepts**: webOS pairing, Cast's media
      receiver, a Cast mirroring session from a sender that is not Chrome,
      AirPlay pairing with its PIN, Wake-on-LAN over Wi-Fi. Each with the
      owner's go-ahead at the moment.
- [x] **Step 2, first part** (2026-11-18): the Miracast sender with the
      test pattern, both routes, played on the LG (720p30, a little stutter).
- [x] **Step 2: 60 pictures a second on the LG** (2026-11-18): smooth once
      pictures were stamped at even times.
- [ ] **Step 6: the phone** (agreed 2026-11-18, after the computer's
      screen): mirror over Miracast, Android's own Cast, and extend through
      adb.
- [ ] **Step 2: this computer's screen on the TV, Linux**: built and tested
      in the sealed test desktop (2026-11-18); still to see through a real
      desktop's dialog onto the LG, and on KDE and Hyprland.
- [ ] **Step 2, still to do**: sound (the LG takes LPCM and AAC), the delay
      measured, the screen on Windows (Windows.Graphics.Capture) and macOS
      (ScreenCaptureKit), Windows' own Wi-Fi Direct.
- [ ] **AirPlay mirroring from Linux, Windows and Android** needs FairPlay's
      sender side, which is not published; find out whether it can be done
      before promising it.
- [ ] **Step 0: each library's licence** checked before it is added.
- [ ] **The TV on no network**: brand and model to hear from the owner.
- [ ] **Mockups** of the TVs list, Add and a TV's screen, approved before
      building.

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
