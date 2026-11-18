# Navigation mockups

The left navigation put in order (asked 2026-11-18): only the kinds of device the
person has added, by name; Home on top; Tools and Settings at the foot; Add a device
at the end of the person's kinds. Each size of window keeps the navigation its system
has for it (sidebar, icons, a phone's bar).

- `index.html`: the proposal at the sizes that matter, side by side, with two ways
  for a phone's bar. Open it in a browser.
- `frame.html?os=gnome|windows|macos|android|ios&have=some|none|many&at=home|<kind>&phone=steady|own`:
  one window on its own; resize it to see the navigation change at that system's
  breakpoints.

The frame uses the app's own `style.css`, `skins.css`, `platform.js` and `icons.js`
(from `apps/desktop/ui/`); `mock.css` holds only what the proposal adds. Devices and
counts are made-up sample data.
