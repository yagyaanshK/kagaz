# Windows host mockups

A Windows computer seen and used from afar while the person at it goes on using it
(plan step 2, asked 2026-11-18), with every choice as a setting.

- `index.html`: the laptop's settings, the helper service's question, one paired
  device, pairing on both ends, the question on the laptop, the sign during a
  session, and the session window with Control and Watch.
- `host.html?os=windows|gnome|macos&scene=settings|device|pair|add|desk|viewer`, with
  `ask=service`, `far=router|ipv6|meet|none` (From anywhere, on, and what it found), `step=words`, `pop=ask|sign|watch`, `wait=1`, `mode=watch`: one
  screen on its own.

The screens use the app's own `style.css`, `skins.css`, `platform.js` and `icons.js`
(from `apps/desktop/ui/`) and the settings and session-windows mockups' `mock.css`;
this folder's `mock.css` holds only what the host adds. Devices are made up and the
"screens" are coloured stand-ins.
