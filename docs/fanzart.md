# FANZART ceiling fan: findings

Checked on 2026-11-10 from the Linux laptop, by listening only: nothing was paired, connected or sent. Parked until there is a radio transmitter to work with.

## What the fan has

- A ceiling fan with speed, light and timer, run from a remote that works without pointing at the fan and shows no LED. That makes it a radio remote, not infrared.
- No phone app exists for it.
- A Bluetooth name, `FANZART`, which belongs to a speaker built into the fan, not to the fan controls.

## The Bluetooth side is only a speaker

- It is classic Bluetooth (BR/EDR), the kind speakers and headsets use, not Bluetooth Low Energy like Atomberg's fans. A low-energy scan does not list it; a classic scan does.
- Device class `0x240404`: audio/video, headset; rendering and audio services.
- Its advert carries manufacturer key `0xd906` and the bytes `54 57 53` ("TWS", true wireless stereo), a label used by Bluetooth audio chips.
- Its service list (SDP, read without pairing) has only two entries: Advanced Audio "Audio Sink" (`0x110b`) and "AV Remote" (`0x110e`). There is no serial port, no low-energy control service, and nothing else that could carry speed, light or timer.

So Kagaz cannot control the fan over Bluetooth.

## The remote is not Bluetooth-style radio

The laptop listened to Bluetooth Low Energy adverts for two minutes while the remote's buttons were pressed (light, power, speed). Only the devices already nearby showed up, each repeating unchanged adverts: the Atomberg fans, a TV, phones, and FANZART's speaker. No new device or burst appeared at any press. Some 2.4 GHz fan remotes send Bluetooth-compatible adverts that a laptop could replay; this one does not.

The remote is therefore most likely 433 MHz (perhaps 315 MHz), or a 2.4 GHz scheme that is not Bluetooth. The laptop has no transmitter for either. **To check:** the remote's back or battery cover usually says `433MHz`, `315MHz` or `2.4G`.

## Plan, once there is a transmitter

1. Learn each button from the remote once: power, light, every speed, every timer value.
   - **Broadlink RM4 Pro** (Wi-Fi, radio and infrared): ready-made, learns and sends; Kagaz talks to it on the network. Check it covers the remote's band.
   - **ESP32 with a 433 MHz receiver and transmitter**: cheapest and open; a small firmware records and replays codes, and Kagaz talks to it over Wi-Fi or USB.
   - A 2.4 GHz remote that is not Bluetooth needs its own look: find the remote's radio chip first.
2. Add FANZART to Kagaz as a second fan brand on the Fans page, with power, speed, light and timer.
3. Nothing reports back: Kagaz can show only what it last sent, and presses on the physical remote are not seen. The page and the Home tile must say so, not show a state as if read from the fan.
4. Check every learned code on the fan before it counts as working.
