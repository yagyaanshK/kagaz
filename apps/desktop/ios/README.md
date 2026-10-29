# Kagaz for iPhone, built on Linux

No Mac and no Xcode download: `build.sh` makes `dist/kagaz-ios.ipa` with the
iPhone toolchain from [omarchy-apple-dev](https://github.com/joshuaswarren/omarchy-apple-dev)
(its sdk-free mode: link stubs cut from Apple's public iOS update).

```
source /data/installations/apple-dev/env.sh
source /data/installations/rust/env.sh
apps/desktop/ios/build.sh
```

How it fits together:

- The app is the same Rust library as on Android (`src/lib.rs`), built for
  `aarch64-apple-ios`; `main.c` starts it.
- Tauri's iPhone layer expects a few functions from its Swift package and
  from swift-rs. Kagaz has no iPhone plugins, so `tauri-glue.c` provides
  them in C; a plugin call is refused, and the Rust side treats that as "not
  available on this phone". No Swift is compiled.
- Frameworks the sysroot has no stub for (WebKit, CoreBluetooth and a few
  more) get small stubs written by `build.sh`: a stub only names what the
  app takes from a framework, and the iPhone has the framework itself.
- `Info.plist` is the app's; `build.sh` fills in the version and build
  number (the commit count, as on Android).

## Putting it on an iPhone

Once, on this computer: `xtool auth` (a free Apple ID; two-factor code once).
On the iPhone: Settings › Privacy & Security › Developer Mode on (it restarts).

Then, with the iPhone on USB and unlocked (tap Trust the first time):

```
xtool install dist/kagaz-ios.ipa
```

`xtool install` signs the app with the Apple ID for this iPhone. On the
iPhone, Settings › General › VPN & Device Management › trust the Apple ID's
developer profile once. With a free Apple ID the app runs for 7 days, then
needs installing again; a paid Apple Developer account removes that limit.

## Not on iPhone yet

Everything that Android does in Kotlin has no iPhone side: ringing with
the app closed (an iPhone needs push notifications through a server for
that), the ring notification, the loudspeaker switch, saving into a chosen
folder, the system's keychain for saved sign-ins, and the phone's own
Bluetooth for the fans (Kagaz's computer Bluetooth code is built in and may
work). Finding devices by multicast needs Apple's multicast entitlement,
which a free Apple ID cannot get; devices with a known address still work.
