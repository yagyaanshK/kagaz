// The Windows host: a computer seen and used from afar while the person at
// it goes on using it. Drawn with made-up devices.
//   host.html?os=windows|gnome|macos
//            &scene=settings|device|pair|desk|viewer|add
//            &ask=service                       (settings: the helper service's question)
//            &step=code|words                   (pair, add: before and after the code is typed)
//            &pop=ask|sign|watch                (desk: the question, or the sign while connected)
//            &wait=1                            (viewer: waiting for someone at the computer)
//            &mode=control|watch                (viewer)
(() => {
  const q = new URLSearchParams(location.search);
  const root = document.documentElement;
  const $ = (id) => document.getElementById(id);
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  const icon = (name, cls) => {
    const s = el("span", cls || "ic");
    s.dataset.icon = name;
    return s;
  };
  const button = (text, cls) => {
    const b = el("button", cls || "", text);
    b.type = "button";
    return b;
  };
  const scene = q.get("scene") || "settings";
  root.dataset.layout = "sidebar";
  root.dataset.scene = scene === "desk" ? "desk" : "app";

  // ---------- the frame ----------
  const active = scene === "viewer" ? "remote" : scene === "add" ? "add" : "remote";
  const entry = (id, label, ic, cls) => {
    const li = el("li", cls || "");
    li.append(icon(ic, "cat-icon"), el("span", "cat-name", label));
    if (id === active) li.classList.add("active");
    return li;
  };
  const scroll = el("li", "cat-scroll");
  const kinds = el("ul", "cat-kinds");
  kinds.append(el("li", "cat-head", "Your devices"), entry("remote", "Computers & phones", "computer"),
    entry("print", "Printers & scanners", "print"), entry("add", "Add a device", "plus", "cat-add"));
  scroll.append(kinds);
  $("categories").append(entry("home", "Home", "home"), scroll, entry("tools", "Tools", "tools", "cat-foot"), entry("settings", "Settings", "settings"));

  // ---------- parts of a settings page ----------
  const body = $("settings-body");
  const section = (title, note) => {
    const box = el("section", "set-section");
    box.append(el("h3", "", title));
    if (note) box.append(el("p", "muted small", note));
    body.append(box);
    return box;
  };
  const row = (label, note, control) => {
    const r = el("div", "set-row");
    const l = el("label", "set-label");
    l.append(el("span", "", label));
    if (note) l.append(el("span", "muted small", note));
    const c = el("span", "set-control");
    for (const one of [].concat(control)) c.append(one);
    r.append(l, c);
    return r;
  };
  const door = (ic, label, note) => {
    const r = el("div", "set-row set-door");
    r.tabIndex = 0;
    r.setAttribute("role", "button");
    const l = el("span", "set-label");
    l.append(el("span", "", label));
    if (note) l.append(el("span", "muted small", note));
    r.append(icon(ic, "set-door-icon ic"), l, icon("forward", "set-door-go ic"));
    return r;
  };
  const select = (options, chosen) => {
    const s = el("select");
    options.forEach((o, i) => {
      const op = el("option", "", o);
      op.selected = i === (chosen || 0);
      s.append(op);
    });
    return s;
  };
  const onoff = (on) => {
    const l = el("label", "switch");
    const i = el("input");
    i.type = "checkbox";
    i.checked = on;
    l.append(i);
    return l;
  };
  const back = (word) => {
    $("bar-back").classList.remove("hidden");
    $("bar-back").querySelector(".bar-back-word").textContent = word;
  };
  const question = (title, detail, keep, act, extra) => {
    const dlg = el("dialog", "ask");
    const form = el("form");
    form.append(el("p", "ask-title", title));
    for (const d of [].concat(detail)) form.append(el("p", "muted small", d));
    if (extra) form.append(extra);
    const r = el("div", "actions");
    const k = button(keep);
    r.append(k, button(act, "accent"));
    form.append(r);
    dlg.append(form);
    document.body.append(dlg);
    dlg.showModal();
    k.focus();
  };
  const choices = (name, list, chosen) => {
    const box = el("div", "ask-choices");
    list.forEach(([label, note], i) => {
      const l = el("label", "ask-choice switch wrap");
      const r = el("input");
      r.type = "radio";
      r.name = name;
      r.checked = i === (chosen || 0);
      l.append(r, el("span", "", label));
      if (note) l.append(el("small", "muted", note));
      box.append(l);
    });
    return box;
  };

  const PERMS = [["Watch its screen", true, true], ["Use its mouse and keys", true], ["Clipboard both ways", true], ["Send files", true],
    ["Hear its sound", true], ["Connect without asking, with the PIN", false], ["Terminal (later)", false, true]];
  const perms = (given) => {
    const grid = el("div", "perm-grid");
    PERMS.forEach(([label, on, fixed], i) => {
      const l = el("label", "switch wrap");
      const c = el("input");
      c.type = "checkbox";
      c.checked = given ? given[i] : on;
      if (fixed) c.disabled = true;
      l.append(c, el("span", "", label));
      grid.append(l);
    });
    return grid;
  };

  if (scene === "settings") {
    // On the computer being shared: Computers & phones › This computer.
    $("bar-title").textContent = "This computer";
    back("Computers & phones");
    const on = section("Sharing this computer", "Paired devices see this screen while you go on using it, over the local network or Tailscale. Nothing passes through anyone else's servers.");
    on.append(row("Let paired devices connect", "", onoff(true)),
      row("Start Kagaz when I sign in to Windows", "It waits in the tray, so a device can connect when nobody has opened Kagaz", onoff(false)),
      row("When a paired device connects", "Asking needs someone at this computer. Each device can be given its own below",
        select(["Ask here each time", "Connect with the PIN, without asking"])),
      row("This computer's PIN", "Typed on the device that connects", button("Change")));
    const during = section("While a device is connected", "A sign at the top of this screen names the device and can end the session. It is always shown.");
    during.append(row("Blank this computer's screen", "For when nobody should see what is done on it", select(["Never", "When I connect without asking", "Ask each time"])),
      row("Lock Windows when the session ends", "", select(["If nobody used it during the session", "Always", "Never"])),
      row("Keep a record of sessions", "Kept on this computer only", onoff(true)),
      door("past", "Past sessions", "12 sessions, the last today at 10:40"));
    const admin = section("Sign-in screen and administrator prompts", "Kagaz runs as the person signed in, so it cannot see the sign-in screen or Windows' administrator prompts.");
    admin.append(row("When one appears", "The helper service is installed once, with an administrator's password",
      select(["Someone at this computer answers it", "Windows' own Remote Desktop (Windows Pro)", "Kagaz's helper service"], q.get("ask") === "service" ? 2 : 0)));
    const paired = section("Paired devices");
    paired.append(door("laptop", "Asha's ThinkPad", "Watch, control, clipboard, files, sound · connects with the PIN"),
      door("mobile", "Asha's phone", "Watch only · asks each time"));
    const add = button("Pair a device", "accent");
    paired.append(add);
    if (q.get("ask") === "service") {
      question("Install Kagaz's helper service?",
        ["It lets a paired device see and answer the sign-in screen and administrator prompts on this computer. Windows asks for an administrator's password once to install it.",
          "It runs with the system's rights whenever this computer is on, also when nobody is signed in. Turning this setting back removes it."],
        "Not now", "Install");
    }
  } else if (scene === "device") {
    // One paired device: what it may do here.
    $("bar-title").textContent = "Asha's ThinkPad";
    back("This computer");
    const p = section("What it may do", "Watching is what pairing gives; everything else is chosen here and can change at any time.");
    p.append(perms([true, true, true, true, true, true, false]));
    const c = section("Connecting");
    c.append(row("When it connects", "", select(["Same as all: ask here each time", "Ask here each time", "Connect with the PIN, without asking"], 2)),
      row("Paired", "18 November 2026 · seen today at 10:40", button("Remove this device", "danger")));
  } else if (scene === "pair") {
    // Pairing, on the computer being shared.
    $("bar-title").textContent = "Pair a device";
    back("This computer");
    if (q.get("step") === "words") {
      const w = section("Check the words", "The other device shows three words. Pair only if they are the same.");
      w.append(el("div", "host-words", "maple · river · seven"));
      const g = section("What Asha's ThinkPad may do", "Chosen now, changed later in its settings.");
      g.append(perms());
      const r = el("div", "actions");
      r.append(button("Cancel"), button("Pair", "accent"));
      body.append(r);
    } else {
      const s = section("On the other device", "Kagaz › Add a device › Computers & phones › Pair with a code. Scan this or type the number. It works for 5 minutes, once.");
      const code = el("div", "pair-code");
      code.append(el("div", "pair-qr"), el("div", "pair-digits", "482 917"));
      s.append(code);
      s.append(el("p", "muted small", "Found nearby also lists this computer while this page is open."));
    }
  } else if (scene === "add") {
    // Pairing, on the device that will connect.
    $("bar-title").textContent = "Pair with a code";
    back("Add a device");
    if (q.get("step") === "words") {
      const w = section("Check the words", "Office laptop shows three words. Pair only if they are the same.");
      w.append(el("div", "host-words", "maple · river · seven"));
      w.append(el("p", "muted small", "Then type Office laptop's PIN the first time you connect; Kagaz remembers it in this computer's password store if you let it."));
      const r = el("div", "actions");
      r.append(button("Cancel"), button("They match", "accent"));
      body.append(r);
    } else {
      const s = section("The computer's code", "On the computer: Kagaz › Computers & phones › This computer › Pair a device.");
      const t = el("input");
      t.type = "text";
      t.inputMode = "numeric";
      t.placeholder = "6 digits";
      s.append(row("Code", "", [t, button("Scan the QR code")]));
      const nearby = section("Found nearby");
      nearby.append(door("laptop", "Office laptop", "Kagaz · ready to pair · local network"));
    }
  } else if (scene === "viewer") {
    // The device connecting: a session window, as today, with watch or control.
    root.dataset.viewer = "on";
    $("settings").classList.add("hidden");
    $("remote").classList.remove("hidden");
    $("bar-title").textContent = "Office laptop";
    const tools = $("bar-remote");
    tools.classList.remove("hidden");
    const chip = el("span", "rc-chip");
    chip.append(icon("lock"), el("span", "", "Local network · 3 ms"));
    const mode = el("span", "rc-mode");
    const watching = q.get("mode") === "watch";
    const ctl = button("Control", watching ? "" : "on");
    const wat = button("Watch", watching ? "on" : "");
    mode.append(ctl, wat);
    tools.append(chip, mode);
    for (const [ic, label] of [["keyboard", "Send keys"], ["clipboard", "Send the clipboard"], ["file", "Send a file"], ["speaker", "Its sound"], ["picture", "Picture"], ["full", "Full screen"]]) {
      const b = button("", "bar-btn");
      b.title = label;
      b.append(icon(ic));
      tools.append(b);
    }
    const view = el("div", "rc-viewer");
    const stage = el("div", "rc-stage");
    const desk = el("div", "desk windows");
    desk.append(el("div", "desk-win one"), el("div", "desk-win two"), el("div", "desk-bar"));
    stage.append(desk);
    if (q.get("wait")) {
      const w = el("div", "rc-wait");
      w.append(icon("guard"), el("strong", "", "Waiting for someone at Office laptop to allow it"),
        el("span", "muted small", "Office laptop asks each time. It says no by itself after 30 seconds."), button("Cancel"));
      stage.append(w);
    }
    view.append(stage);
    $("remote").append(view);
  } else {
    // The shared computer's own desktop, with Kagaz's small windows on it.
    const d = $("desktop");
    d.classList.remove("hidden");
    const desk = el("div", "desk windows");
    desk.append(el("div", "desk-win one"), el("div", "desk-win two"), el("div", "desk-bar"), el("div", "host-tray"));
    d.append(desk);
    const pop = q.get("pop") || "ask";
    if (pop === "ask") {
      const box = el("div", "host-pop");
      const h = el("h4");
      h.append(icon("guard"), el("span", "", "Asha's ThinkPad wants to connect"));
      box.append(h, el("p", "muted small", "A paired device, over the local network."));
      box.append(choices("how", [["Let it use the mouse and keys", "You both can; whoever moves last is in control"], ["Watch only", "It sees this screen; only you use it"]]));
      const r = el("div", "actions");
      r.append(button("Don't allow"), button("Allow", "accent"));
      box.append(r, el("p", "muted small", "Not allowed by itself in 30 seconds if nobody answers."));
      d.append(box);
    } else {
      const sign = el("div", "host-sign");
      const watch = pop === "watch";
      sign.append(el("span", "sw-dot"), el("span", "", watch ? "Asha's ThinkPad is watching this screen" : "Asha's ThinkPad can use this computer"),
        button(watch ? "Let it control" : "Watch only"), button("End", "end"));
      d.append(sign);
    }
  }
  if (typeof KagazIcons !== "undefined") KagazIcons.fill(document);
})();
