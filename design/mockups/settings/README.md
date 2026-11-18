# Settings mockups

Settings put in order (asked 2026-11-18), as more of them arrive: one rule for where
a setting lives (this device, this kind of device, all of Kagaz), one door to all of
them, a kind's setting for all its devices with each device's own where needed, and a
field that finds any of them.

- `index.html`: the rules, the three levels side by side, finding a setting, and the
  same on a phone; below them, where each of today's settings would live.
- `set.html?os=gnome|windows|macos|android|ios&screen=index|kind|device|search`: one
  screen on its own.

The screens use the app's own `style.css`, `skins.css`, `platform.js` and `icons.js`
(from `apps/desktop/ui/`); `mock.css` holds only what the proposal adds. Devices are
made up.
