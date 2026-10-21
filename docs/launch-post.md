# Launch post: how Kagaz differs from home automation

Draft material for the LinkedIn launch post. Three lengths, one argument.
Pick one, cut freely. No device names, addresses or screenshots of a home
network belong in the post.

## The argument in one line

Home automation makes the house act on its own. Kagaz makes the device in
front of you work, now, without the vendor's app, account or hub.

## Short post (about 120 words)

Every device I own came with bad software.

The printer wanted a vendor download and a second tool for its Scan button.
The cameras wanted an account and only showed one at a time. The doorbell
wanted an app that signed me out whenever my partner signed in.

So I built Kagaz: one small, free, open-source app for Linux, Windows,
macOS and Android that finds the device, says in plain words what this
computer can do with it, and lets you do it. Print, scan, copy an ID card,
watch every camera at once, answer the door.

It is not a smart-home hub. Nothing runs when you are not using it. No
server, no account, no rules. Just the device, doing its job.

Kagaz ("paper" in Hindi). Link in the comments.

## Medium post (about 300 words)

People ask how Kagaz is different from Home Assistant. Fair question, and
the answer is the whole idea.

Home automation treats a device as a thing with a state: on or off, a
level, a temperature. The product is rules over those states, run by a box
that is always on, for the whole house. It is excellent at that, and the
person it serves is the one who set it up.

Kagaz treats a device as a thing with a job. Print this. Scan that. Copy
both sides of my ID onto one page. Show me all my cameras. Someone is at
the door, let me talk to them. A printer has no state worth automating,
and no hub can crop a scan.

That changes everything about the shape of the software:

- No hub, no server, no account. Kagaz is a tool you open on a laptop or
  phone, use, and close. Nothing runs in between.
- The first five minutes matter most. Find the device, explain what works
  on this computer and what would fix it, install the vendor's own driver
  only after you have seen what it will do.
- The vendor's software is wrapped, not replaced. When a device needs its
  official driver, Kagaz fetches it from the vendor's own server and runs
  it with your consent. When a device only answers to its cloud, Kagaz
  reaches it exactly as the vendor's app does, with your own login, and
  shows it the way you want.
- Honest about limits. If this device cannot work on this operating
  system, Kagaz says so in plain words instead of pretending.

Think VLC, not a smart home: one small app that opens whatever device is in
front of you.

Today: printers and scanners (driverless on every OS), Tapo cameras, and
video doorbells. Next: TVs and ceiling fans.

Free, open source, GPL. Link in the comments.

## Long post or article (about 600 words)

### Devices that just work

Most devices made in the last decade speak open standards. Printers speak
IPP, scanners speak eSCL and WSD, cameras speak ONVIF and RTSP. On a good
day the operating system notices and everything works. On a bad day you are
on a vendor website guessing which of twelve downloads applies to you, then
discovering that the "Scan to PC" button needs a second tool with four
manual steps. Kagaz is built for the bad days.

### Why not just use Home Assistant?

Because it answers a different question.

Home automation starts from the house: a box that is always on, a model of
every device as an entity with state, and automations that act on that
state. It is the right tool when you want the lights to follow the sun.

Kagaz starts from the device in front of you and the job you want done with
it right now. That sounds like a small difference. It is not.

**The unit is the device, not the house.** Kagaz has no server and no
account of its own. You open it, use the device, close it. Nothing runs in
between, nothing to maintain.

**The verb comes first.** Print, scan, copy an ID card onto one page, tidy
and crop a scan, watch all cameras side by side, play back one moment on
every camera at once, answer the door and open it. These are things an app
does, not things a rule engine does.

**The first five minutes are the product.** Kagaz finds the device on the
network or the cable, and then explains, in plain words, what this
computer can do with it right now, why, and what would fix it. If a driver
is needed, it identifies the exact model, fetches the vendor's own package
from the vendor's own server, verifies it, shows you every step, and runs
the installer only after you say yes.

**The vendor is wrapped, never second-guessed.** Where a hub would
re-implement a device from scratch, Kagaz runs the official driver exactly
as the vendor's own software would, and reads the vendor's settings the way
the vendor's scripts read them. Where a device only answers to its cloud,
Kagaz signs in with your own account and reaches it exactly as the vendor's
app does, then shows it the way you want: any number of cameras at once,
on a laptop and a phone alike, with the recordings on the SD card a click
away.

**Honesty is a feature.** If a device cannot work on this operating system
without a driver that does not exist, Kagaz says so instead of pretending.

### What it is today

One Rust core, a command line that can do everything the window can, and a
small desktop and Android app.

- Printers and scanners: found over mDNS, WS-Discovery, SNMP and USB;
  driverless print and scan on every OS; the Scan button on the printer
  itself; ID copy; Tidy, Crop and Compress on any scan or file.
- Security cameras: every Tapo camera on an account, live and from the SD
  card, with sound, on desktop and phone, no subscription.
- Doorbells: cloud accounts and door stations on the local network; rings
  arrive as a call, even with the app closed, with Answer, Decline and
  Open door.

### What comes next

TVs and ceiling fans, starting with what is in my own home. Then robot
vacuums, local cameras over ONVIF, and whatever contributors bring. Each
device class follows the same shape: find it, explain it, use it, wrap the
vendor, tell the truth.

Kagaz means paper in Hindi. Free and open source under the GPL. Link in the
comments.

## Hooks to try as the first line

- "Every device I own came with bad software."
- "Home Assistant runs my house. Nothing ran my printer."
- "I did not want a smart home. I wanted the scanner to scan."
- "Think VLC for devices."
