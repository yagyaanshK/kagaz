// The settings of a computer or phone, and those of every session window:
// drawn the same on its page in the main window (remote.js) and behind
// the gear of a session's own window (session.js), so that a setting is
// changed without leaving the screen. Each part is a section; the page
// around it saves.

const KagazRemoteSettings = (() => {
  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  let ids = 0;
  // A row: its words (and a note under them), and one control.
  function setRow(label, note, control) {
    const r = el("div", "set-row");
    const l = el("label", "set-label");
    l.append(el("span", "", label));
    if (note) l.append(el("span", "muted small", note));
    const c = el("span", "set-control");
    c.append(control);
    const field = c.querySelector("input, select");
    if (field) {
      field.id = field.id || `remote-set-${++ids}`;
      l.htmlFor = field.id;
    }
    r.append(l, c);
    return r;
  }
  function switchRow(on, label, note, change) {
    const row = el("div", "set-row");
    const l = el("label", "switch wrap");
    const box = el("input");
    box.type = "checkbox";
    box.checked = on;
    l.append(box, " " + label);
    if (note) l.append(el("small", "muted", note));
    box.addEventListener("change", () => change(box.checked));
    row.append(l);
    return row;
  }
  function select(id, options, chosen, change) {
    const s = el("select");
    if (id) s.id = id;
    for (const [v, words] of options) {
      const o = el("option", "", words);
      o.value = v;
      o.selected = v === chosen;
      s.append(o);
    }
    s.addEventListener("change", () => change(s.value));
    return s;
  }

  const KEY_MODES = [["full-screen", "In full screen only"], ["window", "Whenever the pointer is on Kagaz"], ["never", "Never: they stay with this computer"]];

  // One device's sections. `o`: rdp (it is reached over Remote Desktop),
  // vnc (it can be connected over VNC), canRemember, and what to do with
  // a change: settings(next), user(name), remember(on).
  function device(d, o) {
    const settings = { ...(d.settings || {}) };
    const keep = () => o.settings({ ...settings });
    const screen = el("section", "set-section");
    if (o.rdp) {
      // Remote Desktop draws its screen for the viewer, and suits the
      // picture to the connection by itself.
      screen.append(el("h3", "", "Screen and sound"));
      const size = select("remote-size", [["", "Fit this window, sharp"], ["own", "A fixed size (1920 × 1080), scaled to the window"]], settings.own_size ? "own" : "", (v) => {
        if (v) settings.own_size = true;
        else delete settings.own_size;
        keep();
      });
      const sizeRow = el("div", "set-row");
      const sizeLabel = el("label", "", "Size");
      sizeLabel.htmlFor = "remote-size";
      sizeRow.append(sizeLabel, size);
      // Remote Desktop's newer picture (H.264 and its companions) is used
      // unless it comes out wrong on this computer.
      const older = switchRow(!!settings.older_picture, "Use the older picture",
        "Slower and heavier on the connection, for a computer whose screen otherwise comes out wrong or black. Takes effect at the next connection.", (on) => {
          if (on) settings.older_picture = true;
          else delete settings.older_picture;
          keep();
        });
      // Its sound plays here unless turned off for this computer.
      const sound = switchRow(!settings.mute, "Play its sound here",
        "Instead of on the computer itself. The speaker button over its screen mutes it for now. Takes effect at the next connection.", (on) => {
          if (on) delete settings.mute;
          else settings.mute = true;
          keep();
        });
      screen.append(sizeRow, older, sound, el("p", "muted small", "The computer suits its picture to the connection, and shows one screen sized for this window."));
    } else {
      screen.append(el("h3", "", "Picture"));
      const picRow = el("div", "set-row");
      const picLabel = el("label", "", "Picture");
      picLabel.htmlFor = "remote-picture";
      const slider = el("span", "rc-slider");
      const range = el("input");
      range.id = "remote-picture";
      range.type = "range";
      range.min = 0;
      range.max = 10;
      range.value = settings.picture == null ? 0 : settings.picture;
      range.disabled = settings.picture == null;
      slider.append(el("span", "", "Sharper"), range, el("span", "", "Smoother"));
      picRow.append(picLabel, slider);
      const auto = switchRow(settings.picture == null, "Adjust by itself to the connection", "", (on) => {
        range.disabled = on;
        if (on) delete settings.picture;
        else settings.picture = Number(range.value);
        keep();
      });
      range.addEventListener("change", () => {
        settings.picture = Number(range.value);
        keep();
      });
      screen.append(picRow, auto);
      if (o.vnc) screen.append(el("p", "muted small", "Over VNC the device's whole screen is shown at its own size, fitted to the window. Sound and files need Remote Desktop."));
    }

    const mouse = el("section", "set-section");
    mouse.append(el("h3", "", "Mouse"), switchRow(!!settings.swapped_buttons, "Keep left click on the left",
      "For a device whose mouse buttons are swapped: a left click here stays a left click there.", (on) => {
        if (on) settings.swapped_buttons = true;
        else delete settings.swapped_buttons;
        keep();
      }));

    // Where this computer's own keys go while its screen is used (the
    // viewer's key menu has the same choice). Each press goes to one
    // side only.
    const keys = el("section", "set-section");
    keys.append(el("h3", "", "Keyboard"));
    const keysRow = el("div", "set-row");
    const keysLabel = el("label", "", "This computer's own keys go to it");
    keysLabel.htmlFor = "remote-system-keys";
    keysRow.append(keysLabel, select("remote-system-keys", KEY_MODES, settings.system_keys || "full-screen", (v) => {
      if (v === "full-screen") delete settings.system_keys;
      else settings.system_keys = v;
      keep();
    }));
    keys.append(keysRow, el("p", "muted small", "Print Screen, the Windows key, Alt + Tab, Ctrl + Alt + Del and the like, which this computer otherwise keeps. Each press goes to one of the two, never both. Ctrl + Esc leaves full screen; the first time, this computer asks whether Kagaz may have its keys."));

    const signIn = el("section", "set-section");
    signIn.append(el("h3", "", "Sign-in"));
    if (d.paired) {
      // Paired with Kagaz on it: this device signs in as itself, with
      // the computer's PIN, or the person there is asked.
      signIn.append(el("p", "muted small", "Paired with Kagaz on it. It lets this device in with its PIN, or asks the person there."));
      if (o.canRemember) {
        signIn.append(switchRow(!!d.remember, "Remember the PIN", "In this computer's password store, once the computer accepts it. Turning this off forgets it.", (on) => o.remember(on)));
      }
      return { screen, mouse, keys, signIn };
    }
    const userRow = el("div", "set-row");
    const userLabel = el("label", "", "Sign in as");
    userLabel.htmlFor = "remote-user";
    const user = el("input");
    user.id = "remote-user";
    user.type = "text";
    user.autocomplete = "username";
    user.placeholder = "the account's user name";
    user.value = d.user || "";
    user.addEventListener("change", () => o.user(user.value));
    userRow.append(userLabel, user);
    signIn.append(userRow);
    if (o.canRemember) {
      signIn.append(switchRow(!!d.remember, "Remember the password", "In this computer's password store, once the device accepts it. Turning this off forgets it.", (on) => o.remember(on)));
    }
    return { screen, mouse, keys, signIn };
  }

  // What holds for every session window; `keep(change)` saves a change.
  function windows(w, keep) {
    const win = el("section", "set-section");
    win.append(el("h3", "", "Session windows"), el("p", "muted small", "Each session has a window of its own; Kagaz's main window stays free for everything else."));
    const closing = select(null, [["", "End every session, after asking"], ["keep", "Keep the session windows open"]], w.keep_open ? "keep" : "", (v) => keep({ keep_open: v === "keep" }));
    const ringLabel = el("label", "switch");
    const ring = el("input");
    ring.type = "checkbox";
    ring.checked = w.ring !== false;
    ring.setAttribute("aria-label", "Show a call at the door in a session's window");
    ringLabel.append(ring);
    ring.addEventListener("change", () => keep({ ring: ring.checked }));
    win.append(
      setRow("When Kagaz's main window closes", w.keep_open ? "Kagaz stays behind them; the Kagaz button in a session's bar brings its main window back." : "", closing),
      setRow("Show a call at the door in a session's window", "With Answer and Decline, also in full screen", ringLabel),
    );
    return win;
  }

  return { device, windows, setRow, KEY_MODES };
})();
