// Sharing this computer (share.rs runs kagaz-share while it is on): its
// switch and settings, the paired devices and what each may do, pairing
// a new one, and the record of sessions. Shown under Computers & phones
// as "This computer", only where this computer can share (Windows).
// Off until the person turns it on: until then nothing of it runs.

const KagazShare = (() => {
  const { invoke } = window.__TAURI__.core;
  const $ = (id) => document.getElementById(id);
  let st = null; // the latest status from share.rs
  let sub = "main"; // main | pair | device | sessions
  let deviceId = null;
  let note = "";
  // A question about the helper service waiting for a yes: "install", or
  // the choice to go back to ("person", "rdp") once it is removed.
  let helperAsk = null;

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function button(cls, text, onClick) {
    const b = el("button", cls, text);
    b.type = "button";
    b.addEventListener("click", onClick);
    return b;
  }
  function section(title, noteText) {
    const s = el("section", "set-section");
    s.append(el("h3", "", title));
    if (noteText) s.append(el("p", "muted small", noteText));
    return s;
  }
  let ids = 0;
  function row(label, noteText, control) {
    const r = el("div", "set-row");
    const l = el("label", "set-label");
    l.append(el("span", "", label));
    if (noteText) l.append(el("span", "muted small", noteText));
    const c = el("span", "set-control");
    c.append(control);
    const field = c.querySelector("input, select");
    if (field) {
      field.id = field.id || `share-field-${++ids}`;
      l.htmlFor = field.id;
    }
    r.append(l, c);
    return r;
  }
  function check(on, change) {
    const l = el("label", "switch");
    const i = el("input");
    i.type = "checkbox";
    i.checked = on;
    i.addEventListener("change", () => change(i.checked));
    l.append(i);
    return l;
  }
  function select(options, chosen, change) {
    const s = el("select");
    for (const [v, w] of options) {
      const o = el("option", "", w);
      o.value = v;
      o.selected = v === chosen;
      s.append(o);
    }
    s.addEventListener("change", () => change(s.value));
    return s;
  }
  function door(icon, label, noteText, open) {
    const r = el("div", "set-row set-door");
    r.tabIndex = 0;
    r.setAttribute("role", "button");
    const ic = el("span", "set-door-icon ic");
    ic.innerHTML = KagazIcons.svg(icon);
    const l = el("span", "set-label");
    l.append(el("span", "", label));
    if (noteText) l.append(el("span", "muted small", noteText));
    const go = el("span", "set-door-go ic");
    go.innerHTML = KagazIcons.svg("forward");
    r.append(ic, l, go);
    r.addEventListener("click", open);
    r.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" || ev.key === " ") {
        ev.preventDefault();
        open();
      }
    });
    return r;
  }
  const send = (line) => invoke("share_send", { line }).catch((e) => say("Not done: " + e));
  function say(text) {
    note = text;
    const n = $("share-note");
    if (n) n.textContent = text;
  }

  // What a device may do, as words for kagaz-share and for people.
  const PERMS = [["control", "Use its mouse and keys"], ["clipboard", "Clipboard both ways"], ["files", "Send files"], ["sound", "Hear its sound"], ["unattended", "Connect with the PIN, no asking"]];
  const mayWords = (may) => PERMS.filter(([k]) => may[k]).map(([k]) => k).join(" ");
  function mayLine(may) {
    const parts = [may.control ? "Watch and control" : "Watch only"];
    if (may.clipboard) parts.push("clipboard");
    if (may.files) parts.push("files");
    if (may.sound) parts.push("sound");
    parts.push(may.unattended ? "connects with the PIN" : "asks each time");
    return parts.join(", ");
  }
  function permsBox(may, change) {
    const box = el("div", "share-perms");
    box.append(el("label", "switch wrap disabled-look", "Watch its screen (always)"));
    for (const [k, words] of PERMS) {
      const l = el("label", "switch wrap");
      const i = el("input");
      i.type = "checkbox";
      i.checked = !!may[k];
      i.addEventListener("change", () => {
        may[k] = i.checked;
        change(may);
      });
      l.append(i, " " + words);
      box.append(l);
    }
    return box;
  }

  async function save(change) {
    const next = { ...st.settings, ...change };
    try {
      st = await invoke("share_settings", { settings: next });
      say("");
    } catch (e) {
      say(String(e));
      st = await invoke("share_status");
    }
    draw();
  }

  // ---------- the pages ----------
  function drawMain(box) {
    const s = st.settings;
    const on = section("Sharing this computer", "Off until you turn it on. While off, nothing of it runs or listens.");
    on.append(row("Share this computer", st.windows ? "Paired devices only. While on, Kagaz stays in the tray." : "Paired devices only", check(s.on, (v) => save({ on: v }))));
    const status = el("p", "muted small", "");
    status.id = "share-state";
    if (s.on && st.listening) {
      const where = (st.addresses || []).map(([k, a]) => `${a} (${k})`).join(", ");
      status.textContent = `Shared as ${st.name || "this computer"} on port ${s.port}${where ? ": " + where : ""}.`;
    } else if (s.on && st.problem) status.textContent = st.problem;
    else if (s.on) status.textContent = "Starting…";
    on.append(status);
    const n = el("p", "muted small", note);
    n.id = "share-note";
    n.setAttribute("role", "status");
    on.append(n);
    if (s.on && st.windows) on.append(row("Start with Windows", "Waits in the tray", check(!!s.startup, (v) => save({ startup: v }))));
    box.append(on);
    if (!s.on) {
      // Still installed while sharing is off: it listens for nothing, and
      // can be removed here.
      if (st.helper) box.append(drawPrompts());
      return;
    }

    const who = section("Who may connect");
    who.append(
      row("When a newly paired device connects", "Each device can have its own", select([["ask", "Ask here each time"], ["pin", "With the PIN, without asking"]], s.ask ? "ask" : "pin", (v) => save({ ask: v === "ask" }))),
    );
    // The PIN: typed here, kept by kagaz-share as a slow hash.
    const pinForm = el("form", "share-pin");
    const pin = el("input");
    pin.type = "password";
    pin.inputMode = "numeric";
    pin.autocomplete = "new-password";
    pin.placeholder = st.pin ? "A new PIN" : "4 to 8 digits";
    const set = el("button", "", st.pin ? "Change" : "Set");
    set.type = "submit";
    pinForm.append(pin, set);
    pinForm.addEventListener("submit", (ev) => {
      ev.preventDefault();
      const digits = pin.value.replace(/\D/g, "");
      if (digits.length < 4 || digits.length > 8) return say("A PIN is 4 to 8 digits.");
      send(`pin ${digits}`).then(() => say("The PIN is set."));
      pin.value = "";
    });
    who.append(row("This computer's PIN", st.pin ? "Set. Typed on the device that connects." : "Not set yet: devices can only be let in by asking here", pinForm));
    box.append(who);

    const paired = section("Paired devices");
    for (const d of st.devices || []) {
      paired.append(door("laptop", d.name, mayLine(d.may), () => go("device", d.id)));
    }
    if (!(st.devices || []).length) paired.append(el("p", "muted", "None yet."));
    paired.append(el("div", "rc-buttons start"));
    paired.lastChild.append(button("accent", "Pair a device", () => {
      send("pair");
      go("pair");
    }));
    box.append(paired);

    const during = section("While a device is connected", "A sign at the top of the screen always names who is connected, with Watch only and End.");
    during.append(
      row("Lock Windows when the session ends", "", select([["unused", "If nobody used it meanwhile"], ["always", "Always"], ["never", "Never"]], s.lock, (v) => save({ lock: v }))),
      row("Keep a record of sessions", "On this computer only", check(s.record, (v) => save({ record: v }))),
      door("past", "Past sessions", "", () => go("sessions")),
    );
    box.append(during);

    const net = section("Port and networks");
    const port = el("input");
    port.type = "number";
    port.min = 1;
    port.max = 65535;
    port.value = s.port;
    port.addEventListener("change", () => {
      const p = Number(port.value);
      if (p >= 1 && p <= 65535 && p !== s.port) save({ port: p });
    });
    net.append(
      row("Port", "3390 unless changed; Windows' own Remote Desktop keeps 3389", port),
      row("Listen on", "", select([["all", "Local network and Tailscale"], ["tailscale", "Tailscale only"], ["local", "This computer only"]], s.listen, (v) => save({ listen: v }))),
    );
    box.append(net);
    if (st.windows) box.append(drawPrompts());
  }

  // The sign-in screen and administrator prompts: Windows draws them
  // where a program run by the person cannot see or answer.
  function drawPrompts() {
    const s = st.settings;
    const sec = section("Sign-in screen and administrator prompts", "Kagaz can't see these on its own.");
    const chosen = st.helper ? "service" : s.prompts === "service" ? "person" : s.prompts || "person";
    // While its question is open, the menu shows what was picked.
    const shown = helperAsk === "install" ? "service" : helperAsk || chosen;
    const pick = select([["person", "Someone here answers them"], ["rdp", "Windows' Remote Desktop (Pro)"], ["service", "Kagaz's helper service"]], shown, (v) => {
      if (v === chosen) return;
      if (v === "service") helperAsk = "install";
      else if (st.helper) helperAsk = v;
      else return save({ prompts: v });
      draw();
    });
    sec.append(row("When one appears", "", pick));
    if (helperAsk) {
      const ask = el("div", "share-ask-inline");
      ask.setAttribute("role", "alertdialog");
      if (helperAsk === "install") {
        ask.append(
          el("p", "share-ask-title", "Install Kagaz's helper service?"),
          el("p", "", "Lets a paired device answer the sign-in screen and administrator prompts, also after a restart."),
          el("p", "muted small", "Needs an administrator's password once. Runs with system rights while the computer is on. Before anyone signs in, only devices that connect with the PIN, no asking, get in. Choosing otherwise here removes it."),
        );
      } else {
        ask.append(
          el("p", "share-ask-title", "Remove Kagaz's helper service?"),
          el("p", "muted small", "Needs an administrator's password once. Paired devices stay paired."),
        );
      }
      const yes = helperAsk === "install" ? "Install" : "Remove";
      const buttons = el("div", "rc-buttons");
      buttons.append(
        button("", helperAsk === "install" ? "Not now" : "Keep it", () => {
          helperAsk = null;
          draw();
        }),
        button("accent", yes, () => {
          const next = helperAsk === "install" ? "service" : helperAsk;
          helperAsk = null;
          say("Answer Windows' administrator question…");
          save({ prompts: next });
        }),
      );
      ask.append(buttons);
      sec.append(ask);
      return sec;
    }
    if (chosen === "service") {
      sec.append(el("p", "muted small", st.steering
        ? "Installed and running. Sharing goes on while Kagaz is closed; before anyone signs in, only devices that connect with the PIN, no asking, get in."
        : "Installed. Looking for it…"));
    } else if (chosen === "rdp") {
      if (st.rdp === true) sec.append(el("p", "muted small", "Remote Desktop is on here. Connect from Kagaz as a Remote Desktop computer, with a Windows account."));
      else if (st.rdp === false) {
        sec.append(el("p", "muted small", "Remote Desktop is off here (Windows Pro can turn it on)."));
        const b = el("div", "rc-buttons start");
        b.append(button("", "Open Windows' settings", () => send("open-rdp-settings")));
        sec.append(b);
      }
    } else sec.append(el("p", "muted small", "Meanwhile the device sees the screen stop until they are answered here."));
    return sec;
  }

  function drawPair(box) {
    const head = section("Pair a device");
    if (st.words) {
      const [words, name] = st.words;
      head.append(el("p", "", `${name} shows these words:`), el("p", "pair-words", words), el("p", "muted small", "Pair only if it shows the same."));
      const may = { control: true, clipboard: true, files: true, sound: true, unattended: !st.settings.ask };
      const box2 = section(`What ${name} may do`, "Can be changed later.");
      box2.append(permsBox(may, () => {}));
      const row2 = el("div", "rc-buttons");
      row2.append(button("", "Cancel", () => send("cancel").then(() => go("main"))), button("accent", "Pair", () => send(`confirm ${mayWords(may) || "none"}`).then(() => go("main"))));
      box.append(head, box2, row2);
      return;
    }
    if (st.code) {
      const where = (st.addresses || []).map(([k, a]) => `${a} (${k})`).join(", ") || "this computer's address";
      head.append(
        el("p", "", "On the other device: Add a device › Computers & phones › A computer running Kagaz."),
        el("p", "muted", `This computer's address: ${where}; port ${st.settings.port}.`),
        el("p", "pair-digits", st.code),
        el("p", "muted small", "Works once, for 5 minutes."),
      );
    } else head.append(el("p", "muted", "Making a code…"));
    const row2 = el("div", "rc-buttons start");
    row2.append(button("", "Cancel", () => send("stop").then(() => go("main"))));
    box.append(head, row2);
  }

  function drawDevice(box) {
    const d = (st.devices || []).find((x) => x.id === deviceId);
    if (!d) return go("main");
    const may = { ...d.may };
    const what = section(`What ${d.name} may do`);
    what.append(permsBox(may, (m) => send(`allow ${d.id} ${mayWords(m) || "none"}`)));
    const when = section("Paired");
    const since = d.since ? new Date(d.since * 1000).toLocaleDateString() : "";
    const seen = d.seen ? new Date(d.seen * 1000).toLocaleString() : "not yet";
    when.append(el("p", "muted", `Paired ${since}; last connected ${seen}.`));
    const forget = el("p");
    forget.append(button("danger-text", "Remove this device", async () => {
      if (!(await KagazAsk.confirm(`Remove ${d.name}?`, "Remove", "It can no longer connect. Pairing again brings it back."))) return;
      send(`forget ${d.id}`).then(() => go("main"));
    }));
    box.append(what, when, forget);
  }

  async function drawSessions(box) {
    const s = section("Past sessions", "Kept on this computer only.");
    let list = [];
    try {
      list = await invoke("share_sessions");
    } catch (e) {}
    if (!list.length) s.append(el("p", "muted", "None yet."));
    for (const x of list) {
      const mins = Math.max(1, Math.round((x.seconds || 0) / 60));
      s.append(el("p", "", `${x.name} · ${new Date(x.at * 1000).toLocaleString()} · ${mins} min`));
    }
    box.replaceChildren(s);
  }

  function draw() {
    const box = $("remote-share-view");
    if (!box || box.classList.contains("hidden") || !st) return;
    const keepFocus = document.activeElement && box.contains(document.activeElement) && document.activeElement.tagName === "INPUT";
    if (keepFocus) return; // do not take a field away while it is typed in
    box.replaceChildren();
    if (sub === "pair") drawPair(box);
    else if (sub === "device") drawDevice(box);
    else if (sub === "sessions") drawSessions(box);
    else drawMain(box);
    const titles = { pair: "Pair a device", sessions: "Past sessions", device: ((st.devices || []).find((x) => x.id === deviceId) || {}).name || "Device" };
    $("remote").dataset.title = titles[sub] || "This computer";
  }

  function go(to, id) {
    if (to !== "main" && sub === "main") KagazNav.push("share-sub", () => {
      if (sub === "pair") send("stop");
      sub = "main";
      draw();
    });
    if (to === "main" && sub !== "main") KagazNav.pop("share-sub");
    sub = to;
    deviceId = id || null;
    draw();
  }

  async function status() {
    try {
      st = await invoke("share_status");
    } catch (e) {
      st = null;
    }
    return st;
  }

  // Shown when the page opens (remote.js calls it).
  async function open() {
    sub = "main";
    await status();
    draw();
  }

  if (window.__TAURI__ && window.__TAURI__.event) {
    window.__TAURI__.event.listen("share", (ev) => {
      st = ev.payload;
      draw();
      if (typeof KagazRemote !== "undefined" && KagazRemote.shareChanged) KagazRemote.shareChanged(st);
    });
  }

  return { open, status, current: () => st };
})();
