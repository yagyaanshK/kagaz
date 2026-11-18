# Session windows mockups

Each session with a computer or phone in a window of its own (asked 2026-11-18), so
that several run at once and Kagaz's main window stays free for everything else.

- `index.html`: the main window beside two sessions, the three questions (closing a
  session's window, connecting again to a device that has a window, closing the main
  window with sessions open) and a call at the door shown in a session's window.
- `win.html?os=gnome|windows|macos&scene=main|session&name=…&desk=windows|ubuntu&ask=close|again|quit&ring=1`:
  one window on its own.

The windows use the app's own `style.css`, `skins.css`, `platform.js` and `icons.js`
(from `apps/desktop/ui/`); `mock.css` holds only the mockup's extras. Devices are
made up and the "remote screens" are coloured stand-ins.
