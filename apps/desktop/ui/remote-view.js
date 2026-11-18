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
        button("accent", "Try again", () => connect()),
      ),
    );
  }

  function failedCard(d, why) {
    showCard(
      el("h3", "", `Could not connect to ${d.name}`),
      el("p", "", why),
      buttons(button("", "Back", () => close()), button("accent", "Try again", () => connect())),
    );
  }

  function endedCard(d, why) {
    showCard(
      el("h3", "", `The session with ${d.name} ended`),
      el("p", "", why.charAt(0).toUpperCase() + why.slice(1) + "."),
      buttons(button("", "Back", () => close()), button("accent", "Connect again", () => connect())),
    );
  }

  // A session with this device goes over Remote Desktop where it speaks
  // that and this build has it, else over VNC (as the core chooses).
  const overRdp = (d) => !!hooks.canRdp && d.ways.some((w) => w.speaks === "rdp");

  // `wrong`: false the first time, "typed" or "remembered" after a
  // password that the device did not accept.
  function passwordCard(d, wrong) {
    const vnc = !overRdp(d);
    const form = el("form", "rc-form");
    // Remote Desktop signs in to an account: who, and the password.
    const userLabel = el("label", "", "User name");
    const user = el("input");
    user.type = "text";
    user.autocomplete = "username";
    user.autocapitalize = "none";
    user.spellcheck = false;
    user.value = (cur && cur.last && cur.last.user) || d.user || "";
    userLabel.append(user);
    if (!vnc) form.append(userLabel);
    const label = el("label", "", "Password");
    // The app's own Show button joins every password field (reveal.js).
    const input = el("input");
    input.type = "password";
    input.autocomplete = "current-password";
    input.setAttribute("aria-describedby", "rv-pass-hint");
    label.append(input);
    const hint = el("small", "muted", vnc ? "A VNC password: only its first eight letters count."
      : "The account on that computer, as you sign in to it there.");
    hint.id = "rv-pass-hint";
    // Kept in the system's password store, never in Kagaz's own files.
    const keepLabel = el("label", "switch wrap");
    const keep = el("input");
    keep.type = "checkbox";
    keep.checked = !!d.remember;
    keepLabel.append(keep, " Remember the password in this computer's password store");
    const go = el("button", "accent", "Connect");
    go.type = "submit";
    form.append(label, hint);
    if (hooks.canRemember) form.append(keepLabel);
    form.append(buttons(button("", "Cancel", () => close()), go));
    form.addEventListener("submit", (ev) => {
      ev.preventDefault();
      if (!vnc && !user.value.trim()) return user.focus();
      if (!input.value) return input.focus();
      d.remember = keep.checked;
      connect({ user: vnc ? null : user.value.trim(), password: input.value, remember: hooks.canRemember ? keep.checked : null });
    });
    const say = el("p", wrong ? "rc-wrong" : "muted", wrong === "remembered" ? "The device did not accept the remembered password."
      : wrong ? (vnc ? "The device did not accept that password." : "The computer did not accept that user name and password.")
        : vnc ? `${d.name} asks for a password.` : `${d.name} asks who signs in.`);
    say.setAttribute("role", "status");
    showCard(el("h3", "", `Sign in to ${d.name}`), say, form);
    (!vnc && !user.value ? user : input).focus();
  }

  // The computer shows another certificate than the one remembered: the
  // person decides, before anything of the sign-in is sent.
  function certificateCard(d) {
    showCard(
      el("h3", "", `${d.name} shows another certificate`),
      el("p", "", "It is not the one Kagaz remembered for this computer. Windows renews its certificate about twice a year, and a reinstalled computer has a new one. It also happens when another device answers in its place."),
      el("p", "muted", "If you did not expect this, cancel and check the computer and its address."),
      buttons(
        button("", "Cancel", () => close()),
        button("accent", "Connect and remember the new one", () => connect({ ...(cur.last || {}), accept_certificate: true })),
      ),
    );
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
    connect();
  }

  // The viewer as the device should draw for it: the stage in the
  // screen's own pixels, and how many of them make a point, in percent.
  function view() {
    const dpr = window.devicePixelRatio || 1;
    return [Math.round(stage.clientWidth * dpr), Math.round(stage.clientHeight * dpr), Math.round(dpr * 100)];
  }

  // `asking`: what the person typed, if the device asked: `user`,
  // `password`, `remember` (keep the password once accepted), and
  // `accept_certificate` after they took a changed certificate.
  async function connect(asking) {
    if (!cur) return;
    asking = asking || {};
    cur.last = asking;
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
      connected = await invoke("remote_connect", {
        id: state.device.id,
        asking: {
          user: asking.user || null,
          password: asking.password || null,
          remember: asking.remember == null ? null : asking.remember,
          accept_certificate: !!asking.accept_certificate,
          view: view(),
        },
        channel,
      });
    } catch (failed) {
      if (cur !== state || attempt !== mine) return;
      const f = typeof failed === "object" && failed ? failed : { kind: "other", why: String(failed) };
      if (f.kind === "password" || f.kind === "user") passwordCard(state.device, false);
      else if (f.kind === "certificate") certificateCard(state.device);
      else if (f.kind === "wrong-password") passwordCard(state.device, f.remembered ? "remembered" : "typed");
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
    state.last = null; // nothing typed is kept once it has been used
    state.asked = view();
    for (; state.owed > 0; state.owed--) invoke("remote_painted", { session: state.session }).catch(() => {});
    hideCard();
    $("rv-banner").classList.toggle("hidden", connected.protected);
    setTools(connected.info);
    setChip(connected.millis);
    stage.focus();
    wantKeys();
    pushClipboard(true);
    // Files the window was started to send (`kagaz remote send`).
    invoke("remote_startup_files").then((paths) => {
      if (paths && paths.length) sendFiles(paths);
    }).catch(() => {});
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
    fullScreen(false);
    wantKeys();
    delete document.documentElement.dataset.viewer;
    $("bar-remote").classList.add("hidden");
    canvas.classList.add("hidden");
    if (hooks.closed) hooks.closed();
  }

  // Another page of Kagaz came on screen (a call at the door, the person
  // went to look at something) or this one is back. The session goes on
  // meanwhile; its bar, its keys and its hold on the whole window step
  // aside, and return with the person.
  function shown(on) {
    if (!cur || on === !cur.away) return;
    cur.away = !on;
    if (on) {
      document.documentElement.dataset.viewer = "on";
      $("bar-remote").classList.remove("hidden");
      stageResized();
      stage.focus();
      // Its sound again, unless the person had turned it off.
      if (cur.info && cur.info.sound && !cur.soundOff) did({ t: "sound", on: true }, true);
      log("back on screen");
      wantKeys();
    } else {
      releaseKeys(cur);
      closePop();
      // Its sound waits too: a call at the door is not talked over.
      if (cur.info && cur.info.sound && !cur.soundOff) did({ t: "sound", on: false }, true);
      fullScreen(false);
      delete document.documentElement.dataset.viewer;
      $("bar-remote").classList.add("hidden");
      log("another page on screen; the session goes on");
      wantKeys();
    }
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
    const not = (what) => (info && info.speaks === "rdp" ? `${what} over Remote Desktop is not built yet`
      : `${what}: VNC carries none of this; the device needs Remote Desktop for it`);
    const set = (id, has, title, without) => {
      $(id).disabled = !(on && has);
      $(id).title = on && !has ? without : title;
    };
    set("rv-file", info && info.files, "Send files: they go to the device's clipboard, to be pasted there", not("Sending files"));
    // The device's sound plays here unless its page says not to; the
    // button turns it off and on for the moment.
    const noSound = info && info.speaks === "rdp" ? "Its sound is turned off for this computer on its page"
      : "VNC carries no sound; the device needs Remote Desktop for it";
    set("rv-sound", info && info.sound, "", noSound);
    if (cur) cur.soundOff = false;
    soundButton();
    // No kind of session has monitors to choose from yet: Remote Desktop
    // makes one screen for this window, VNC shows the device's as it is.
    $("rv-screens").classList.toggle("hidden", !(info && info.monitors));
    set("rv-screens", info && info.monitors, "Choose a monitor", "");
  }

  // The sound button says what a press does now.
  function soundButton() {
    const b = $("rv-sound");
    const off = !!(cur && cur.soundOff);
    setIcon(b.querySelector(".ic"), off ? "mute" : "speaker");
    if (!b.disabled) b.title = off ? "Play the device's sound here again" : "Turn the device's sound off here for now";
    b.setAttribute("aria-label", b.title || "The device's sound");
    b.setAttribute("aria-pressed", off ? "true" : "false");
  }

  let toastTimer = 0;
  function toast(text, stays) {
    const t = $("rv-toast");
    t.textContent = text;
    t.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => t.classList.add("hidden"), stays || 3500);
  }
  const sentence = (e) => String(e).replace(/^./, (c) => c.toUpperCase()).replace(/\.?$/, ".");

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
      else if (news.type === "pointer-default") canvas.style.cursor = "default";
      else if (news.type === "clipboard") {
        const what = news.picture ? "a picture is" : `${news.characters} characters are`;
        toast(news.kept ? `Copied on ${cur.device.name}: ${what} on this computer's clipboard now.`
          : `Something was copied on ${cur.device.name}; the clipboard is not carried on this system yet.`);
      } else if (news.type === "sending") sending(news);
    } else if (u8[0] === 3) {
      const w = dv.getUint16(4, true), h = dv.getUint16(6, true);
      const hx = dv.getUint16(8, true), hy = dv.getUint16(10, true);
      pointerShape(w, h, hx, hy, new Uint8ClampedArray(u8.buffer, u8.byteOffset + 12, w * h * 4));
    }
  }

  function resize(w, h) {
    log(`its screen is ${w}x${h}; the viewer has room for ${view().slice(0, 2).join("x")}`);
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
  // The window changed size: fit the picture now, and a moment after the
  // last change ask a device that can to draw its screen at the new size.
  let resizing = 0;
  function stageResized() {
    fit();
    clearTimeout(resizing);
    resizing = setTimeout(() => {
      if (!cur || cur.session == null || !cur.info || !cur.info.resizes) return;
      const now = view();
      if (cur.asked && now[0] === cur.asked[0] && now[1] === cur.asked[1]) return;
      if (now[0] < 200 || now[1] < 200) return;
      fullScreenNow();
      cur.asked = now;
      log(`asked for ${now[0]}x${now[1]}`);
      did({ t: "resize", width: now[0], height: now[1] }, true);
    }, 350);
  }
  new ResizeObserver(stageResized).observe(stage);
  window.addEventListener("resize", stageResized);

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
    wantKeys();
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
  // The side buttons (back, forward) go to the device too, where its
  // kind of session carries them.
  const held = (ev) => (ev.buttons & 1 ? LEFT : 0) | (ev.buttons & 4 ? MIDDLE : 0) | (ev.buttons & 2 ? RIGHT : 0)
    | (ev.buttons & 8 ? 8 : 0) | (ev.buttons & 16 ? 16 : 0);

  // Caps Lock, Num Lock and Scroll Lock as they are on this keyboard:
  // told to the device when they differ from what it was last told (they
  // can change while the person is elsewhere), so that capitals here are
  // capitals there. Not in the middle of a held key, nor from a lock
  // key's own press, whose state is in between.
  const LOCKS = ["CapsLock", "NumLock", "ScrollLock"];
  function locks(ev) {
    if (!cur || cur.session == null || !cur.info || cur.info.speaks !== "rdp" || !ev.getModifierState) return;
    if (cur.held.size || LOCKS.includes(ev.code)) return;
    const now = LOCKS.map((k) => ev.getModifierState(k));
    if (cur.locks && now.every((on, i) => on === cur.locks[i])) return;
    cur.locks = now;
    did({ t: "locks", caps: now[0], num: now[1], scroll: now[2] }, true);
  }

  function pointer(ev, now) {
    if (!cur || cur.session == null) return;
    locks(ev);
    const p = at(ev);
    cur.point = p;
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
  // Over the device's screen the mouse's back button is the device's:
  // it does not step back in Kagaz (nav.js), which would end the session.
  for (const kind of ["mousedown", "mouseup", "auxclick"]) {
    canvas.addEventListener(kind, (ev) => {
      if (ev.button < 3) return;
      ev.preventDefault();
      ev.stopPropagation();
    });
  }

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
    // Ctrl + Esc leaves full screen: the one press Kagaz keeps for
    // itself there, where every other key is the device's. (In a window
    // it is the device's too.)
    if (ev.code === "Escape" && (cur.keptEsc || (down && full && ev.ctrlKey && !ev.altKey && !ev.shiftKey && !ev.metaKey))) {
      cur.keptEsc = down;
      if (down && !ev.repeat) fullScreen(false);
      return;
    }
    if (down && !ev.repeat) locks(ev);
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
  stage.addEventListener("blur", () => {
    releaseKeys(cur);
    wantKeys();
  });
  stage.addEventListener("focus", () => {
    pushClipboard(true);
    wantKeys();
  });
  window.addEventListener("focus", () => {
    if (cur && cur.session != null) pushClipboard(true);
    wantKeys();
  });
  window.addEventListener("blur", () => {
    // Keys held when the window lost the focus are let go there by
    // whatever took it; the device is told the same.
    releaseKeys(cur);
    wantKeys();
  });

  // ---------- this computer's own keys ----------
  // Print Screen, the Windows key, Alt + Tab and the system's other
  // shortcuts never reach a page: this computer acts on them itself. The
  // window can ask for them (remote_keys), and then they go to the
  // device, and only there: a press is never acted on by both. When it
  // asks is the person's choice for the device (settings.system_keys):
  // in full screen only (nothing of this computer shows then), whenever
  // Kagaz is in front with the pointer on it, or never. Never while a
  // card or question of Kagaz's own has the keys.
  const KEY_MODES = [
    ["full-screen", "In full screen only"],
    ["window", "Whenever the pointer is on Kagaz"],
    ["never", "Never: they stay with this computer"],
  ];
  const keyMode = () => (cur && cur.settings && cur.settings.system_keys) || "full-screen";
  let pointerIn = document.documentElement.matches(":hover");
  let keysSaid = "", keysTurn = Promise.resolve(), keysLater = null, keysLogged = 0;
  function wantKeys() {
    wantPinch();
    const mode = keyMode();
    const here = !!cur && cur.session != null && !cur.away && document.activeElement === stage && card.classList.contains("hidden");
    const on = here && (mode === "window" ? pointerIn : mode === "full-screen" && full);
    const focused = document.hasFocus();
    const say = `${on}:${focused}`;
    clearTimeout(keysLater);
    if (say === keysSaid) return;
    const tell = () => {
      keysSaid = say;
      keysTurn = keysTurn.then(() => invoke("remote_keys", { on, focused })).then((held) => {
        if (keysLogged++ < 8) log(`this computer's own keys: ${held ? "to the device" : "here"}`);
      }).catch(() => {});
    };
    // Taken at once; given back a moment later, when it is known what
    // else changed with it (the pointer leaves and the focus goes in the
    // same instant when the system puts a question of its own on top).
    if (on) tell();
    else keysLater = setTimeout(tell, 150);
  }
  // ---------- a pinch ----------
  // Two fingers spread or closed on the touchpad zoom what is under the
  // pointer on the device, not Kagaz's own page: while a device's screen
  // shows, the window keeps the webview from enlarging the page
  // (remote_pinch) and tells how far the fingers have spread. That goes
  // on as Ctrl + wheel, a notch for every eighth wider or narrower: what
  // Windows itself makes of a pinch on its own touchpad, and what
  // programs everywhere take for zoom.
  let pinchSaid = false, pinchDone = 0;
  function wantPinch() {
    const on = !!cur && cur.session != null && !cur.away;
    if (on === pinchSaid) return;
    pinchSaid = on;
    invoke("remote_pinch", { on }).then((found) => {
      if (on) log(found ? "a pinch here zooms on the device" : "this webview's own pinch could not be turned off");
    }).catch(() => {});
  }
  function pinch(scale, begin) {
    if (begin) pinchDone = 0;
    if (begin || !cur || cur.session == null || cur.away || !(scale > 0)) return;
    const steps = Math.trunc(Math.log(scale) / Math.log(1.125));
    const up = steps - pinchDone;
    if (!up) return;
    if (!pinchDone) log("a pinch, passed on as zoom");
    pinchDone = steps;
    const p = cur.point || { x: Math.round(cur.width / 2), y: Math.round(cur.height / 2) };
    // With Ctrl already held here, the device has it held too.
    const ctrl = cur.held.has("ControlLeft") || cur.held.has("ControlRight");
    if (!ctrl) did({ t: "key", key: "Control", code: "ControlLeft", down: true }, false);
    did({ t: "wheel", x: p.x, y: p.y, up, right: 0 }, ctrl);
    if (!ctrl) did({ t: "key", key: "Control", code: "ControlLeft", down: false }, true);
  }

  document.documentElement.addEventListener("mouseenter", () => {
    pointerIn = true;
    wantKeys();
  });
  document.documentElement.addEventListener("mouseleave", () => {
    pointerIn = false;
    wantKeys();
  });

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
      const sent = await invoke("remote_clipboard_push", { session: cur.session, asked: !quietly });
      if (quietly) return;
      toast(sent.picture ? `Offered the picture on this computer's clipboard to ${cur.device.name}. Paste there to use it.`
        : sent.characters == null ? "There is no text or picture on this computer's clipboard."
        : `Offered ${sent.characters} characters to ${cur.device.name}. Paste there to use them.`);
    } catch (e) {
      if (!quietly) toast(sentence(e));
    }
  }

  // ---------- files ----------
  // Files go by the device's clipboard: Kagaz says which files there
  // are, and the device reads them when they are pasted there. Without
  // `paths` the person is asked which files.
  async function sendFiles(paths) {
    if (!cur || cur.session == null) return;
    closePop();
    try {
      await invoke("remote_send_files", { session: cur.session, paths: paths || null });
    } catch (e) {
      toast(sentence(e));
    }
    stage.focus();
  }

  const size = (bytes) => (bytes < 1000 ? `${bytes} bytes` : bytes < 1e6 ? `${Math.round(bytes / 1000)} KB` : megabytes(bytes));
  // How the files offered are getting on (see `Sending` in the core).
  function sending(news) {
    const there = cur.device.name;
    if (news.state === "offered") {
      const one = news.names.length === 1;
      cur.offer = { waiting: true };
      toast(`${one ? news.names[0] : `${news.names.length} files`} (${size(news.bytes)}) ${one ? "is" : "are"} on ${there}'s clipboard. `
        + `Paste ${one ? "it" : "them"} there: open a folder and press Ctrl + V.`, 15000);
      log(`files offered ${news.names.length}, ${news.bytes} bytes`);
    } else if (news.state === "going") {
      if (cur.offer) cur.offer.waiting = false;
      toast(`Sending ${news.name}: ${Math.floor((news.sent / Math.max(news.size, 1)) * 100)}%`, 10000);
    } else if (news.state === "taken") {
      if (cur.offer) cur.offer.waiting = false;
      toast(`${news.name} is on ${there}.`);
      log(`file taken, ${news.size} bytes`);
    } else if (news.state === "withdrawn") {
      // Worth saying only while they were still waiting to be pasted.
      if (cur.offer && cur.offer.waiting) toast("The files are no longer on offer: something else was copied since.");
      cur.offer = null;
    } else if (news.state === "failed") {
      toast(sentence(news.why), 6000);
      log("files not sent");
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
    // And where this computer's own keys go, for this device.
    const where = el("label", "rc-keys-where");
    const choose = el("select");
    for (const [v, words] of KEY_MODES) {
      const o = el("option", "", words);
      o.value = v;
      o.selected = keyMode() === v;
      choose.append(o);
    }
    choose.addEventListener("change", () => {
      cur.settings = { ...cur.settings, system_keys: choose.value };
      if (choose.value === "full-screen") delete cur.settings.system_keys;
      if (hooks.settings) hooks.settings(cur.device.id, cur.settings);
      wantKeys();
    });
    where.append(el("span", "", `Print Screen, the Windows key, Alt + Tab and this computer's other keys go to ${cur.device.name}`), choose);
    if (openPop("rv-keys", "keys", [el("strong", "", "Send keys"), ...list, where])) pop.setAttribute("role", "menu");
  });

  $("rv-clip").addEventListener("click", () => {
    closePop();
    pushClipboard(false);
    stage.focus();
  });

  $("rv-file").addEventListener("click", () => sendFiles());

  $("rv-sound").addEventListener("click", () => {
    if (!cur || cur.session == null) return;
    closePop();
    cur.soundOff = !cur.soundOff;
    did({ t: "sound", on: !cur.soundOff }, true);
    soundButton();
    toast(cur.soundOff ? `${cur.device.name}'s sound is off here for now.` : `${cur.device.name}'s sound plays here again.`);
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
    const chooses = !cur.info || cur.info.picture_setting;
    const note = el("p", "muted", chooses
      ? "Smoother lets the device send areas of many colours as JPEG, when its VNC server can."
      : "Over Remote Desktop the computer suits its picture to the connection by itself, and draws its screen at this window's size.");
    // What cannot be chosen here is not offered.
    const parts = chooses ? [slider, autoLabel, sizeLabel] : [];
    if (openPop("rv-picture", "picture", [el("strong", "", "Picture"), ...parts, line, note])) {
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
    const now = { at: performance.now(), frames: cur.frames, paintMs: cur.paintMs, bytes: cur.bytes, work: lastReading ? lastReading.work : null, sound: lastReading ? lastReading.sound : null };
    const was = cur.told;
    cur.told = now;
    if (!was || now.frames === was.frames) return;
    const seconds = (now.at - was.at) / 1000;
    const frames = now.frames - was.frames;
    // And from the session itself: how much of each second went into
    // what the device sent (decoding its picture), and how that came.
    const r = lastReading;
    const work = r && was.work != null && r.work >= was.work ? `, ${Math.round((r.work - was.work) / seconds)} ms of work a second` : "";
    log(`${(frames / seconds).toFixed(1)} updates/s, ${((now.paintMs - was.paintMs) / frames).toFixed(2)} ms to paint one, `
      + `${((now.bytes - was.bytes) / seconds / 1e6).toFixed(1)} MB/s to the page, delay ${r && r.delay != null ? r.delay + " ms" : "unknown"}${work}`
      + (r && was.sound != null && r.sound > was.sound ? `, sound ${Math.round((r.sound - was.sound) / seconds / 1000)} kB/s` : "")
      + (r && r.via ? `; ${r.via}` : "")
      + (r && r.sound_via ? `; sound by the ${r.sound_via}` : ""));
  }, 5000);

  // Full screen is the window's own, not the page's: a page's full
  // screen ends with Esc, and Esc in a session belongs to the device.
  // Where the window cannot (a browser in the tests), the page's serves.
  let full = false;
  function fullIcon() {
    setIcon($("rv-full").querySelector(".ic"), full ? "unfull" : "full");
    $("rv-full").title = full ? "Leave full screen (Ctrl + Esc)" : "Full screen";
    $("rv-full").setAttribute("aria-label", $("rv-full").title);
  }
  async function fullScreen(on) {
    if (on === full) return;
    try {
      full = await invoke("remote_fullscreen", { on });
    } catch (e) {
      const root = document.documentElement;
      try {
        if (on && root.requestFullscreen) await root.requestFullscreen();
        else if (!on && document.fullscreenElement) await document.exitFullscreen();
        full = on;
      } catch (e2) {
        if (on) toast("Full screen is not available here.");
      }
    }
    fullIcon();
    wantKeys();
    if (on && full && cur && cur.session != null) {
      toast(`Ctrl + Esc leaves full screen.${keyMode() === "never" ? "" : ` Print Screen, the Windows key and Alt + Tab go to ${cur.device.name} meanwhile.`}`, 5000);
    }
  }
  // The system may take the window out of full screen by itself.
  async function fullScreenNow() {
    try {
      full = await invoke("remote_fullscreen", { on: null });
    } catch (e) {
      full = !!document.fullscreenElement;
    }
    fullIcon();
    wantKeys();
  }
  $("rv-full").addEventListener("click", () => {
    closePop();
    fullScreen(!full);
    stage.focus();
  });
  document.addEventListener("fullscreenchange", fullScreenNow);

  // The window is being closed with a session open (main.rs holds it
  // back): that ends the session, so the person is asked.
  const events = window.__TAURI__ && window.__TAURI__.event;
  if (events && events.listen) {
    events.listen("remote-pinch", (ev) => pinch(ev.payload.scale, ev.payload.begin));
    events.listen("remote-close-asked", async () => {
      if (cur && cur.session != null) {
        const yes = await KagazAsk.confirm(`Close Kagaz and end the session with ${cur.device.name}?`, "Close Kagaz",
          "Everything open on it stays open there.", "Stay");
        if (!yes) {
          if (!cur.away) stage.focus();
          return;
        }
      }
      invoke("remote_close_window").catch(() => {});
    });
  }

  // A narrow window's tools under the picture do what the bar's do.
  document.querySelectorAll("#rv-touchbar button").forEach((b) => b.addEventListener("click", () => $(b.dataset.tool).click()));
  // A click on the screen closes an open panel.
  stage.addEventListener("pointerdown", (ev) => {
    if (ev.target === stage) closePop();
  });

  return { open, close, shown, isOpen: () => !!cur };
})();
