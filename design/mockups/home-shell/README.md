# Home shell mockups

The Kagaz Home screen and top-level navigation, drawn once and skinned per platform:
Android (Material 3), iPhone (iOS 26), Windows 11 (Fluent 2), GNOME (libadwaita 1.8)
and macOS. Plain HTML, CSS and JavaScript, no build step, no network.

- `index.html`: every platform at the sizes that matter, side by side. Open it in a
  browser (double-click, or `python3 -m http.server` in this folder). Dark and the
  state (normal, ringing, empty) switch at the top.
- `shell.html?os=android|ios|windows|gnome|macos&state=normal|urgent|empty&detail=fan&theme=dark`:
  one screen on its own, so the window can be resized to see the layout change at that
  OS's own breakpoints (Android 600 dp, Windows 640 and 1008 px, GNOME 550 sp, the
  870 px sheet/dialog switch).

Sizes, colours and radii come from each platform's published tokens and stylesheets;
the research behind them is in the (local, uncommitted) `research_notes/` and
`reports/` folders at the repo root. Device names and readings are made-up sample data.
