# Computers & phones mockups

The screens of remote connection (plan: `docs/remote-connection.md`), for
approval before they are built: the device list, Add, one device's page and
the viewer.

- `index.html`: every screen side by side, with the desktop look (Ubuntu,
  Windows, macOS), the phone look (Android, iPhone) and Dark chosen at the
  top. Open it in a browser.
- `screen.html?os=…&screen=list|add|device|viewer&state=…&open=keys|picture`:
  one screen on its own, to resize it; the states are listed at its top.

The look is the app's own: `screen.html` loads `apps/desktop/ui/style.css`,
`skins.css`, `platform.js` and `icons.js`, and only `mock.css` and `mock.js`
are new. Device names, addresses (documentation ranges) and readings are
made-up sample data; the remote screen is a drawing, never a capture.
