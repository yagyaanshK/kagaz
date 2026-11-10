# The Sleep Company adjustable bed: findings

Checked on 2026-11-10 from the Linux laptop, by listening only. Parked, like the [FANZART fan](fanzart.md), until there is a radio transmitter.

## What is known

- A motorised adjustable bed from The Sleep Company, run from a remote that works pointed in any direction and shows no LED: a radio remote, not infrared.
- No Wi-Fi or Bluetooth control is known, and no app.
- A 25-second Bluetooth listen (classic and low energy) from the laptop heard nothing new except two Apple devices (manufacturer `0x004c`, "nearby" adverts). The bed may have been out of the laptop's range, so this does not rule out a Bluetooth box.
- The Sleep Company publishes nothing on which motor its beds use. Adjustable bases are usually built on a motor maker's control box (OKIN, Keeson, Dewert, Limoss), and the remote speaks that box's radio.

## To find out next

1. **The control box under the bed**: its label gives the maker and model (for example `OKIN`, `Keeson`, `Dewert`), which decides everything below.
2. **The remote's back or battery cover**: `433MHz`, `315MHz`, `2.4G`, or an FCC ID to look up.
3. **Whether the box takes a Bluetooth receiver**: several OKIN and Keeson boxes accept a plug-in Bluetooth module (they then show up as, for example, `OKIN-BLE…` or `OKIN-Receiver`). With one fitted, Kagaz could drive the bed over Bluetooth directly, with no radio transmitter.

## Routes

- **433 or 315 MHz remote**: the same transmitter as the fan (Broadlink RM4 Pro, or ESP32 with a 433 MHz receiver and transmitter) learns each button and replays it.
- **2.4 GHz remote that is not Bluetooth** (common on OKIN "RF" remotes): a 433 MHz transmitter cannot send it; it needs a 2.4 GHz radio such as an nRF24L01 on an ESP32, after identifying the remote's chip and protocol.
- **Bluetooth module in the control box**: Kagaz talks to the box directly, as it does with Atomberg fans.

## Safety, whatever the route

A bed that moves under software control must stay as safe as its remote:

- Movement runs only while a button is held, and stops when it is let go, the window loses focus, or the link drops. Never "move to a position" and walk away.
- No automation and no remote access (from another network) that moves the bed unseen.
- A large Stop control, always visible while the bed can move.
- Every learned code checked at the bed before it counts as working.
- Nothing reports back: Kagaz knows only what it sent.
