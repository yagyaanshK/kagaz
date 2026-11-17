// The viewer of Computers & phones: the window becomes the device's
// screen. The picture arrives from remote_view.rs as binary packets over
// a channel (updates of changed areas, moves, the pointer's shape, news)
// and is painted on a canvas; each painted update is confirmed, so that
// Kagaz never sends more than the page can show. The mouse, the wheel and
// the keys typed while the screen has the focus go to the device.
// Uses app.js's globals: $, invoke; icons.js; nav.js.

const KagazRemoteView = (() => {
  const LEFT = 1, MIDDLE = 2, RIGHT = 4;
  let cur = null; // the session shown, or being opened
  let attempt = 0; // which opening is the latest
  let hooks = {};
  const stage = $("rv-stage");
  const canvas = $("rv-canvas");
  const ctx = canvas.getContext("2d", { alpha: false });
  const card = $("rv-card");
  const pop = $("rv-pop");

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
  const setIcon = (holder, name) => (holder.innerHTML = KagazIcons.svg(name));
  const low = (label) => (label === "Tailscale" ? label : label.charAt(0).toLowerCase() + label.slice(1));
  const log = (line) => invoke("page_log", { line: "remote: " + line }).catch(() => {});

  // ---------- cards over the stage ----------
  function showCard(...children) {
    card.replaceChildren(...children);
    card.classList.remove("hidden");
    closePop();
  }
  const hideCard = () => card.classList.add("hidden");
  function buttons(...list) {
    const row = el("div", "rc-buttons");
    row.append(...list);
    return row;
  }

  function connectingCard(d) {
    const list = el("ul", "rc-paths");
    for (const p of d.paths) {
      const li = el("li");
      const state = el("span", "muted");
      state.append(el("span", "rc-spin"), " trying");
      li.append(el("span", "", `${PATH[p.kind] || p.kind}${p.host ? " · " + p.host : ""}`), state);
      list.append(li);
    }
    const status = el("p", "muted", "Every way is tried at once; the first to answer is used.");
    status.setAttribute("role", "status");
    showCard(el("h3", "", `Connecting to ${d.name}`), list, status, buttons(button("", "Cancel", () => close())));
  }
  const PATH = { address: "Local network", name: "Name", tailscale: "Tailscale", gateway: "Its hotspot" };

  function lostCard(d, failed) {
    const list = el("ul", "rc-paths");
    for (const p of failed.paths || []) {
      const li = el("li");
      li.append(el("span", "", `${p.label}${p.host ? " · " + p.host : ""}`), el("span", "muted", p.trouble || "no answer"));
      list.append(li);
    }
    showCard(
      el("h3", "", `${d.name} is out of reach`),
      el("p", "", failed.why),
      list,
      buttons(
        button("", "Edit its addresses", () => close()),
        button("accent", "Try again", () => connect(null)),
      ),
    );
  }

  function failedCard(d, why) {
    showCard(
      el("h3", "", `Could not connect to ${d.name}`),
      el("p", "", why),
      buttons(button("", "Back", () => close()), button("accent", "Try again", () => connect(null))),
    );
  }

  function endedCard(d, why) {
    showCard(
      el("h3", "", `The session with ${d.name} ended`),
      el("p", "", why.charAt(0).toUpperCase() + why.slice(1) + "."),
      buttons(button("", "Back", () => close()), button("accent", "Connect again", () => connect(null))),
    );
  }

  function passwordCard(d, wrong) {
    const vnc = (d.ways.find((w) => w.speaks !== "ssh") || {}).speaks === "vnc";
    const form = el("form", "rc-form");
    const label = el("label", "", "Password");
    const wrap = el("span", "rc-pass");
    const input = el("input");
    input.type = "password";
    input.autocomplete = "current-password";
    input.setAttribute("aria-describedby", "rv-pass-hint");
    const eye = button("", "", () => {
      input.type = input.type === "password" ? "text" : "password";
      eye.setAttribute("aria-label", input.type === "password" ? "Show the password" : "Hide the password");
    });
    eye.setAttribute("aria-label", "Show the password");
    setIcon(eye, "eye");
    wrap.append(input, eye);
    label.append(wrap);
    const hint = el("small", "muted", (vnc ? "A VNC password: only its first eight letters count. " : "")
      + "Kagaz asks each time for now; keeping it in this computer's password store comes next.");
    hint.id = "rv-pass-hint";
    const go = el("button", "accent", "Connect");
    go.type = "submit";
    form.append(label, hint, buttons(button("", "Cancel", () => close()), go));
    form.addEventListener("submit", (ev) => {
      ev.preventDefault();
      if (!input.value) return input.focus();
      connect(input.value);
    });
    const say = el("p", wrong ? "rc-wrong" : "muted", wrong ? "The device did not accept that password." : `${d.name} asks for a password.`);
    say.setAttribute("role", "status");
    showCard(el("h3", "", `Sign in to ${d.name}`), say, form);
    input.focus();
  }

  // ---------- opening and closing ----------
  function open(device, h) {
    hooks = h || {};
    cur = { device, session: null, owed: 0, width: 0, height: 0, held: new Map(), buttons: 0, frames: 0, paintMs: 0, bytes: 0, told: null, ownSize: false };
    document.documentElement.dataset.viewer = "on";
    $("bar-remote").classList.remove("hidden");
    canvas.classList.add("hidden");
    canvas.style.cursor = "";
    $("rv-banner").classList.add("hidden");
    stage.classList.remove("own-size");
    setTools(null);
    setChip(null);
    if (hooks.show) hooks.show();
    connect(null);
  }

  async function connect(password) {
    if (!cur) return;
    const mine = ++attempt;
    const state = cur;
    if (state.session != null) {
      invoke("remote_disconnect", { session: state.session }).catch(() => {});
      state.session = null;
    }
    state.owed = 0;
    connectingCard(state.device);
    const channel = new window.__TAURI__.core.Channel();
    channel.onmessage = (m) => {
      if (cur === state && attempt === mine) feed(m);
    };
    let connected;
    try {
      connected = await invoke("remote_connect", { id: state.device.id, password: password || null, channel });
    } catch (failed) {
      if (cur !== state || attempt !== mine) return;
      const f = typeof failed === "object" && failed ? failed : { kind: "other", why: String(failed) };
      if (f.kind === "password") passwordCard(state.device, false);
      else if (f.kind === "wrong-password") passwordCard(state.device, true);
      else if (f.kind === "unreachable") lostCard(state.device, f);
      else failedCard(state.device, f.why || String(failed));
      return;
    }
    // Closed, or opened again, while this one was on its way.
    if (cur !== state || attempt !== mine) {
      invoke("remote_disconnect", { session: connected.session }).catch(() => {});
      return;
    }
    state.session = connected.session;
    state.info = connected.info;
    state.path = connected.path;
    state.protected = connected.protected;
    state.settings = connected.settings || {};
    state.since = performance.now();
    for (; state.owed > 0; state.owed--) invoke("remote_painted", { session: state.session }).catch(() => {});
    hideCard();
    $("rv-banner").classList.toggle("hidden", connected.protected);
    setTools(connected.info);
    setChip(connected.millis);
    stage.focus();
    pushClipboard(true);
    log(`open ${connected.info.speaks} ${connected.width}x${connected.height} over ${low(connected.path)} in ${connected.millis} ms`);
    if (hooks.connected) hooks.connected(state.device.id);
  }

  // Leave the viewer: the session ends, the page it came from returns.
  function close() {
    if (!cur) return;
    const state = cur;
    cur = null;
    attempt++;
    releaseKeys(state);
    if (state.session != null) {
      log(`close after ${Math.round((performance.now() - state.since) / 1000)} s, ${state.frames} updates painted`);
      invoke("remote_disconnect", { session: state.session }).catch(() => {});
    }
    closePop();
    hideCard();
    if (document.fullscreenElement && document.exitFullscreen) document.exitFullscreen().catch(() => {});
    delete document.documentElement.dataset.viewer;
    $("bar-remote").classList.add("hidden");
    canvas.classList.add("hidden");
    if (hooks.closed) hooks.closed();
  }

  // ---------- the bar's chip and tools ----------
  function setChip(millis) {
    const chip = $("rv-chip");
    chip.classList.toggle("hidden", !cur || cur.session == null);
    if (!cur || cur.session == null) return;
    setIcon($("rv-chip-icon"), cur.protected ? "lock" : "unlock");
    chip.classList.toggle("open", !cur.protected);
    // The path's name gives way first where the bar is short.
    $("rv-chip-path").textContent = millis != null ? `${cur.path} ·` : cur.path;
    $("rv-chip-text").textContent = millis != null ? `${millis} ms` : "";
    chip.title = cur.protected ? "How this session travels. Nobody in between can read it."
      : "How this session travels. It is not encrypted on this network.";
  }

  function setTools(info) {
    const on = !!info;
    for (const id of ["rv-keys", "rv-clip", "rv-picture", "rv-full"]) $(id).disabled = !on;
    document.querySelectorAll("#rv-touchbar button").forEach((b) => (b.disabled = !on));
    const not = (what) => `${what}: VNC carries none of this; the device needs Remote Desktop for it`;
    const set = (id, has, title, without) => {
      $(id).disabled = !(on && has);
      $(id).title = on && !has ? without : title;
    };
    set("rv-file", info && info.files, "Send a file", not("Sending files"));
    set("rv-sound", info && info.sound, "The device's sound", not("The device's sound"));
    set("rv-screens", info && info.monitors, "Choose a monitor", not("Choosing a monitor"));
  }

  let toastTimer = 0;
  function toast(text) {
    const t = $("rv-toast");
    t.textContent = text;
    t.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => t.classList.add("hidden"), 3500);
  }

  // ---------- the picture ----------
  function feed(m) {
    const u8 = m instanceof Uint8Array ? m : new Uint8Array(m);
    if (!u8.length || !cur) return;
    const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
    if (u8[0] === 1) {
      const began = performance.now();
      cur.bytes += u8.length;
      const count = dv.getUint32(8, true);
      let at = 12;
      for (let i = 0; i < count; i++) {
        const kind = u8[at];
        const x = dv.getUint16(at + 4, true), y = dv.getUint16(at + 6, true);
        const w = dv.getUint16(at + 8, true), h = dv.getUint16(at + 10, true);
        const fx = dv.getUint16(at + 12, true), fy = dv.getUint16(at + 14, true);
        at += 16;
        if (kind === 1) resize(w, h);
        else if (kind === 2) {
          const len = w * h * 4;
          ctx.putImageData(new ImageData(new Uint8ClampedArray(u8.buffer, u8.byteOffset + at, len), w, h), x, y);
          at += len;
        } else if (kind === 3) ctx.drawImage(canvas, fx, fy, w, h, x, y, w, h);
      }
      cur.frames++;
      cur.paintMs += performance.now() - began;
      if (cur.session != null) invoke("remote_painted", { session: cur.session }).catch(() => {});
      else cur.owed++;
    } else if (u8[0] === 2) {
      let news;
      try {
        news = JSON.parse(new TextDecoder().decode(u8.subarray(1)));
      } catch (e) {
        return;
      }
      if (news.type === "ended") ended(news.why);
      else if (news.type === "clipboard") {
        toast(news.kept ? `Copied on ${cur.device.name}: ${news.characters} characters are on this computer's clipboard now.`
          : `Text was copied on ${cur.device.name}; the clipboard is not carried on this system yet.`);
      }
    } else if (u8[0] === 3) {
      const w = dv.getUint16(4, true), h = dv.getUint16(6, true);
      const hx = dv.getUint16(8, true), hy = dv.getUint16(10, true);
      pointerShape(w, h, hx, hy, new Uint8ClampedArray(u8.buffer, u8.byteOffset + 12, w * h * 4));
    }
  }

  function resize(w, h) {
    cur.width = w;
    cur.height = h;
    canvas.width = w;
    canvas.height = h;
    canvas.classList.remove("hidden");
    fit();
  }

  // The canvas on the stage: as large as fits, or the device's own size.
  function fit() {
    if (!cur || !cur.width) return;
    const { width: w, height: h } = cur;
    const dpr = window.devicePixelRatio || 1;
    let scale = 1 / dpr; // one device pixel for one of the screen's
    if (!cur.ownSize) {
      const room = Math.min(stage.clientWidth / w, stage.clientHeight / h);
      // Shrink to fit; grow a small screen, but not into a blur.
      scale = room < 1 / dpr ? room : Math.min(room, 2 / dpr);
    }
    canvas.style.width = `${Math.max(1, Math.round(w * scale))}px`;
    canvas.style.height = `${Math.max(1, Math.round(h * scale))}px`;
  }
  new ResizeObserver(fit).observe(stage);

  // The device's pointer, drawn here so that it moves at once.
  function pointerShape(w, h, hx, hy, rgba) {
    if (!w || !h) {
      canvas.style.cursor = "none";
      return;
    }
    const c = document.createElement("canvas");
    c.width = w;
    c.height = h;
    c.getContext("2d").putImageData(new ImageData(new Uint8ClampedArray(rgba), w, h), 0, 0);
    canvas.style.cursor = `url(${c.toDataURL()}) ${Math.min(hx, w - 1)} ${Math.min(hy, h - 1)}, default`;
  }

  function ended(why) {
    if (!cur) return;
    const state = cur;
    log(`ended: ${why}`);
    releaseKeys(state, true);
    state.session = null;
    setTools(null);
    setChip(null);
    canvas.style.cursor = "";
    if (why !== "closed") endedCard(state.device, why);
  }

  // ---------- what the person does ----------
  let outbox = [];
  let flushing = 0;
  function flush() {
    flushing = 0;
    if (!outbox.length || !cur || cur.session == null) {
      outbox = [];
      return;
    }
    const did = outbox;
    outbox = [];
    invoke("remote_input", { session: cur.session, did }).catch(() => {});
  }
  // Moves wait a moment (8 ms), so a fast hand sends some hundred a
  // second at most, each the latest; presses and keys go at once.
  function did(d, now) {
    if (!cur || cur.session == null) return;
    outbox.push(d);
    if (now) {
      clearTimeout(flushing);
      flush();
    } else if (!flushing) flushing = setTimeout(flush, 8);
  }

  function at(ev) {
    const r = canvas.getBoundingClientRect();
    const x = Math.round(((ev.clientX - r.left) * cur.width) / Math.max(1, r.width));
    const y = Math.round(((ev.clientY - r.top) * cur.height) / Math.max(1, r.height));
    return { x: Math.min(Math.max(x, 0), cur.width - 1), y: Math.min(Math.max(y, 0), cur.height - 1) };
  }
  // The page counts left, right, middle; a session left, middle, right.
  const held = (ev) => (ev.buttons & 1 ? LEFT : 0) | (ev.buttons & 4 ? MIDDLE : 0) | (ev.buttons & 2 ? RIGHT : 0);

  function pointer(ev, now) {
    if (!cur || cur.session == null) return;
    const p = at(ev);
    cur.buttons = held(ev);
    // Of moves waiting to go, only the last matters.
    const last = outbox[outbox.length - 1];
    if (!now && last && last.t === "pointer" && last.buttons === cur.buttons) outbox.pop();
    did({ t: "pointer", x: p.x, y: p.y, buttons: cur.buttons }, now);
  }
  canvas.addEventListener("pointerdown", (ev) => {
    ev.preventDefault();
    stage.focus({ preventScroll: true });
    try {
      canvas.setPointerCapture(ev.pointerId);
    } catch (e) {}
    closePop();
    pointer(ev, true);
  });
  canvas.addEventListener("pointermove", (ev) => pointer(ev, false));
  canvas.addEventListener("pointerup", (ev) => pointer(ev, true));
  canvas.addEventListener("pointercancel", (ev) => pointer(ev, true));
  canvas.addEventListener("pointerenter", () => pushClipboard(true));
  canvas.addEventListener("contextmenu", (ev) => ev.preventDefault());

  // The wheel: a notch for every hundred pixels a touchpad or wheel says.
  let wheelY = 0, wheelX = 0;
  canvas.addEventListener("wheel", (ev) => {
    if (!cur || cur.session == null) return;
    ev.preventDefault();
    const unit = ev.deltaMode === 1 ? 33 : ev.deltaMode === 2 ? 400 : 1;
    wheelY += ev.deltaY * unit;
    wheelX += ev.deltaX * unit;
    const up = -Math.trunc(wheelY / 100), right = Math.trunc(wheelX / 100);
    if (!up && !right) return;
    wheelY -= -up * 100;
    wheelX -= right * 100;
    const p = at(ev);
    did({ t: "wheel", x: p.x, y: p.y, up, right }, true);
  }, { passive: false });

  // Keys, while the screen has the focus. Each is remembered until it is
  // let go, so that none stays held on the device when the focus leaves.
  function key(ev, down) {
    if (!cur || cur.session == null || ev.isComposing) return;
    ev.preventDefault();
    ev.stopPropagation();
    const id = ev.code || ev.key;
    if (down) cur.held.set(id, { key: ev.key, code: ev.code });
    else cur.held.delete(id);
    const send = () => did({ t: "key", key: ev.key, code: ev.code, down }, true);
    // Paste: what was copied here goes ahead of the key.
    const paste = down && !ev.repeat && ((ev.ctrlKey && ev.code === "KeyV") || (ev.shiftKey && ev.code === "Insert"));
    if (paste) pushClipboard(true).finally(send);
    else send();
  }
  stage.addEventListener("keydown", (ev) => key(ev, true));
  stage.addEventListener("keyup", (ev) => key(ev, false));
  function releaseKeys(state, quietly) {
    if (!state) return;
    if (!quietly && state.session != null && state === cur) {
      for (const k of state.held.values()) did({ t: "key", key: k.key, code: k.code, down: false }, true);
    }
    state.held.clear();
  }
  stage.addEventListener("blur", () => releaseKeys(cur));
  stage.addEventListener("focus", () => pushClipboard(true));
  window.addEventListener("focus", () => cur && cur.session != null && pushClipboard(true));

  // A row of keys pressed in order and let go in reverse.
  function press(keys) {
    for (const [k, code] of keys) did({ t: "key", key: k, code, down: true }, true);
    for (const [k, code] of [...keys].reverse()) did({ t: "key", key: k, code, down: false }, true);
  }
  const KEYS = [
    ["Ctrl + Alt + Del", [["Control", "ControlLeft"], ["Alt", "AltLeft"], ["Delete", "Delete"]]],
    ["Windows key", [["Meta", "MetaLeft"]]],
    ["Alt + Tab", [["Alt", "AltLeft"], ["Tab", "Tab"]]],
    ["Esc", [["Escape", "Escape"]]],
    ["Print Screen", [["PrintScreen", "PrintScreen"]]],
    ["Alt + F4", [["Alt", "AltLeft"], ["F4", "F4"]]],
  ];

  // ---------- the clipboard ----------
  // What is copied on the device arrives by itself (remote_view.rs sets
  // this computer's clipboard). What is copied here is offered to the
  // device whenever the person comes back to the viewer.
  async function pushClipboard(quietly) {
    if (!cur || cur.session == null) return;
    try {
      const sent = await invoke("remote_clipboard_push", { session: cur.session });
      if (quietly) return;
      toast(sent == null ? "There is no text on this computer's clipboard."
        : `Offered ${sent} characters to ${cur.device.name}. Paste there to use them.`);
    } catch (e) {
      if (!quietly) toast(String(e).replace(/^./, (c) => c.toUpperCase()) + ".");
    }
  }

  // ---------- the tools ----------
  let popFor = null;
  function closePop() {
    pop.classList.add("hidden");
    pop.replaceChildren();
    if (popFor) $(popFor).setAttribute("aria-expanded", "false");
    popFor = null;
  }
  function openPop(tool, cls, children) {
    const again = popFor === tool;
    closePop();
    if (again) return false;
    pop.className = "rc-pop " + cls;
    pop.replaceChildren(...children);
    popFor = tool;
    $(tool).setAttribute("aria-expanded", "true");
    return true;
  }

  $("rv-keys").addEventListener("click", () => {
    const list = KEYS.map(([word, keys]) => {
      const b = button("", word, () => {
        press(keys);
        closePop();
        stage.focus();
      });
      b.setAttribute("role", "menuitem");
      return b;
    });
    if (openPop("rv-keys", "keys", [el("strong", "", "Send keys"), ...list])) pop.setAttribute("role", "menu");
  });

  $("rv-clip").addEventListener("click", () => {
    closePop();
    pushClipboard(false);
    stage.focus();
  });

  const megabytes = (bytes) => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(2)} GB` : `${(bytes / 1e6).toFixed(bytes >= 1e7 ? 0 : 1)} MB`);
  let lastReading = null;
  function numbersLine(r) {
    const parts = [r.delay != null ? `${r.delay} ms delay` : "delay not measured yet"];
    if (lastReading && r.seconds > lastReading.seconds) {
      const rate = ((r.received - lastReading.received) * 8) / (r.seconds - lastReading.seconds) / 1e6;
      parts.push(`${rate.toFixed(rate >= 10 ? 0 : 1)} Mbit/s now`);
    }
    parts.push(`${megabytes(r.received + r.sent)} this session`);
    return parts.join(" · ");
  }

  $("rv-picture").addEventListener("click", () => {
    if (!cur) return;
    const s = cur.settings || {};
    const slider = el("span", "rc-slider");
    const range = el("input");
    range.type = "range";
    range.min = 0;
    range.max = 10;
    range.setAttribute("aria-label", "Sharper to smoother");
    slider.append(el("span", "", "Sharper"), range, el("span", "", "Smoother"));
    const autoLabel = el("label", "switch");
    const auto = el("input");
    auto.type = "checkbox";
    auto.checked = s.picture == null;
    autoLabel.append(auto, " Adjust by itself to the connection");
    const sizeLabel = el("label", "switch");
    const own = el("input");
    own.type = "checkbox";
    own.checked = !!cur.ownSize;
    sizeLabel.append(own, " Show at the device's own size, not fitted to the window");
    const line = el("p", "muted");
    line.id = "rv-numbers";
    const show = (r) => {
      if (r && auto.checked) range.value = r.picture;
      range.disabled = auto.checked;
      if (r) line.textContent = numbersLine(r);
    };
    range.value = s.picture == null ? 0 : s.picture;
    show(lastReading);
    const keep = () => {
      const value = auto.checked ? null : Number(range.value);
      cur.settings = { ...cur.settings, picture: value };
      if (value == null) delete cur.settings.picture;
      did({ t: "picture", value }, true);
      if (hooks.settings) hooks.settings(cur.device.id, cur.settings);
      range.disabled = auto.checked;
    };
    range.addEventListener("change", keep);
    auto.addEventListener("change", keep);
    own.addEventListener("change", () => {
      cur.ownSize = own.checked;
      stage.classList.toggle("own-size", own.checked);
      fit();
    });
    const note = el("p", "muted", cur.info && cur.info.speaks === "vnc"
      ? "Smoother lets the device send areas of many colours as JPEG, when its VNC server can."
      : "");
    if (openPop("rv-picture", "picture", [el("strong", "", "Picture"), slider, autoLabel, sizeLabel, line, note])) {
      pop.removeAttribute("role");
      pop._show = show;
    }
  });

  // The delay in the chip and the numbers in the picture panel, each second.
  async function readNumbers() {
    if (!cur || cur.session == null) return;
    try {
      const r = await invoke("remote_numbers", { session: cur.session });
      if (!cur || cur.session == null) return;
      if (r.delay != null) setChip(r.delay);
      if (popFor === "rv-picture" && pop._show) pop._show(r);
      lastReading = r;
    } catch (e) {}
  }
  setInterval(readNumbers, 1000);

  // How the viewer does, in numbers, in Kagaz's log every five seconds
  // while something moves: pictures a second, how long one takes to
  // paint, how much reached the page.
  setInterval(() => {
    if (!cur || cur.session == null) return;
    const now = { at: performance.now(), frames: cur.frames, paintMs: cur.paintMs, bytes: cur.bytes };
    const was = cur.told;
    cur.told = now;
    if (!was || now.frames === was.frames) return;
    const seconds = (now.at - was.at) / 1000;
    const frames = now.frames - was.frames;
    log(`${(frames / seconds).toFixed(1)} updates/s, ${((now.paintMs - was.paintMs) / frames).toFixed(2)} ms to paint one, `
      + `${((now.bytes - was.bytes) / seconds / 1e6).toFixed(1)} MB/s to the page, delay ${lastReading && lastReading.delay != null ? lastReading.delay + " ms" : "unknown"}`);
  }, 5000);

  $("rv-full").addEventListener("click", () => {
    closePop();
    const root = document.documentElement;
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    else if (root.requestFullscreen) root.requestFullscreen().catch(() => toast("Full screen is not available here."));
    stage.focus();
  });
  document.addEventListener("fullscreenchange", () => {
    const on = !!document.fullscreenElement;
    setIcon($("rv-full").querySelector(".ic"), on ? "unfull" : "full");
    $("rv-full").title = on ? "Leave full screen" : "Full screen";
    $("rv-full").setAttribute("aria-label", $("rv-full").title);
  });

  // A narrow window's tools under the picture do what the bar's do.
  document.querySelectorAll("#rv-touchbar button").forEach((b) => b.addEventListener("click", () => $(b.dataset.tool).click()));
  // A click on the screen closes an open panel.
  stage.addEventListener("pointerdown", (ev) => {
    if (ev.target === stage) closePop();
  });

  return { open, close, isOpen: () => !!cur };
})();
