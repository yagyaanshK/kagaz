// The viewer of Computers & phones: the window becomes the device's
// screen. The picture arrives from remote_view.rs as binary packets over
// a channel (updates of changed areas, moves, the pointer's shape, news)
// and is painted on a canvas; each painted update is confirmed, so that
// Kagaz never sends more than the page can show. The mouse, the wheel and
// the keys typed while the screen has the focus go to the device.
// Uses app.js's globals: $, invoke; icons.js; nav.js.
//
// On a computer each session has a window of its own (session.html, with
// session.js in app.js's place): there `hooks.window` is set, leaving the
// viewer closes the window, and the questions speak of the session, not
// of Kagaz. On a phone, and wherever a window cannot be opened, the
// viewer takes over the main window as before.

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

  // `asking`: a computer running Kagaz is asking the person there.
  function connectingCard(d, asking) {
    const list = el("ul", "rc-paths");
    for (const p of d.paths) {
      const li = el("li");
      const state = el("span", "muted");
      state.append(el("span", "rc-spin"), " trying");
      li.append(el("span", "", `${PATH[p.kind] || p.kind}${p.host ? " · " + p.host : ""}`), state);
      list.append(li);
    }
    const status = el("p", "muted", asking ? `Waiting for someone at ${d.name} to allow it. No answer in 30 seconds means no.`
      : "Every way is tried at once; the first to answer is used.");
    status.setAttribute("role", "status");
    showCard(el("h3", "", asking ? `Asking ${d.name}` : `Connecting to ${d.name}`), list, status, buttons(button("", "Cancel", () => close())));
  }
  const PATH = { address: "Local network", name: "Name", tailscale: "Tailscale", gateway: "Its hotspot" };
  // Leaving the viewer goes back to the page it came from, or closes the
  // session's own window.
  const leave = () => (hooks.window ? "Close window" : "Back");

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
        button("", "Edit its addresses", () => (hooks.edit ? hooks.edit() : close())),
        button("accent", "Try again", () => connect()),
      ),
    );
  }

  function failedCard(d, why) {
    showCard(
      el("h3", "", `Could not connect to ${d.name}`),
      el("p", "", why),
      buttons(button("", leave(), () => close()), button("accent", "Try again", () => connect())),
    );
  }

  function endedCard(d, why) {
    showCard(
      el("h3", "", `The session with ${d.name} ended`),
      el("p", "", why.charAt(0).toUpperCase() + why.slice(1) + "."),
      buttons(button("", leave(), () => close()), button("accent", "Connect again", () => connect())),
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

  // A computer running Kagaz, paired with this one: its PIN lets this
  // device in at once where the computer allows that; without it, the
  // person there is asked.
  function pinCard(d) {
    const form = el("form", "rc-form");
    const label = el("label", "", "PIN");
    const input = el("input");
    input.type = "password";
    input.inputMode = "numeric";
    input.autocomplete = "off";
    label.append(input);
    const keepLabel = el("label", "switch wrap");
    const keep = el("input");
    keep.type = "checkbox";
    keep.checked = !!d.remember;
    keepLabel.append(keep, " Remember the PIN in this computer's password store");
    const go = el("button", "accent", "Connect");
    go.type = "submit";
    const ask = button("", "Ask someone there", () => connect({ password: "", remember: null }));
    form.append(label);
    if (hooks.canRemember) form.append(keepLabel);
    form.append(buttons(button("", "Cancel", () => close()), ask, go));
    form.addEventListener("submit", (ev) => {
      ev.preventDefault();
      if (!input.value.trim()) return input.focus();
      d.remember = keep.checked;
      connect({ password: input.value.trim(), remember: hooks.canRemember ? keep.checked : null });
    });
    showCard(el("h3", "", `Connect to ${d.name}`),
      el("p", "muted", `Its PIN, or ask someone at ${d.name} to allow it.`), form);
    input.focus();
  }

  // The computer shows another certificate than the one remembered: the
  // person decides, before anything of the sign-in is sent.
  function certificateCard(d) {
    showCard(
      el("h3", "", `${d.name} shows another certificate`),
      el("p", "", "It differs from the one Kagaz remembered. Windows renews its certificate about twice a year, and a reinstalled computer gets a new one. Another device answering in its place also causes this."),
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
    cur = { device, session: null, owed: 0, width: 0, height: 0, held: new Map(), buttons: 0, frames: 0, paintMs: 0, bytes: 0, told: null, ownSize: false, asked: null, responses: [], lastResponse: null };
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
    closePipe(state);
    if (state.video) videoClose(state.video);
    state.video = null;
    connectingCard(state.device, state.device.paired && asking.password === "");
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
          // An empty PIN is an answer too: the person there is asked.
          password: asking.password == null ? null : asking.password,
          remember: asking.remember == null ? null : asking.remember,
          accept_certificate: !!asking.accept_certificate,
          view: view(),
          // The device's H.264 decoded here, by the graphics chip, rather
          // than sent as pixels: lag comes first.
          video: videoHere(),
        },
        channel,
      });
    } catch (failed) {
      if (cur !== state || attempt !== mine) return;
      const f = typeof failed === "object" && failed ? failed : { kind: "other", why: String(failed) };
      if ((f.kind === "password" || f.kind === "user") && state.device.paired) pinCard(state.device);
      else if (f.kind === "password" || f.kind === "user") passwordCard(state.device, false);
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
    // Linux: the pictures come down a WebSocket on this computer (much
    // quicker there than the channel for a large picture); Kagaz waits a
    // moment for it, then uses the channel.
    if (connected.pipe && typeof WebSocket === "function") {
      try {
        const ws = new WebSocket(connected.pipe);
        ws.binaryType = "arraybuffer";
        ws.onmessage = (m) => {
          if (cur === state && attempt === mine) feed(m.data);
        };
        state.ws = ws;
      } catch (e) {
        log(`the pictures' pipe could not be opened (${e.message || e}); the channel carries them`);
      }
    }
    state.path = connected.path;
    state.protected = connected.protected;
    state.settings = connected.settings || {};
    state.since = performance.now();
    state.last = null; // nothing typed is kept once it has been used
    state.asked = view();
    for (; state.owed > 0; state.owed--) invoke("remote_painted", { session: state.session }).catch(() => {});
    if (state.videoOffOwed) {
      state.videoOffOwed = false;
      invoke("remote_video_off", { session: state.session }).catch(() => {});
    }
    // A computer running Kagaz that is asking its person lets the session
    // open before the answer: "Asking…" stays until its first picture.
    if (state.device.paired && asking.password === "" && !state.frames) state.waitingAnswer = true;
    else hideCard();
    $("rv-banner").classList.toggle("hidden", connected.protected);
    setTools(connected.info);
    setChip(connected.millis);
    stage.focus();
    wantKeys();
    askLocks();
    pushClipboard(true);
    // Files the window was started to send (`kagaz remote send`).
    invoke("remote_startup_files").then((paths) => {
      if (paths && paths.length) sendFiles(paths);
    }).catch(() => {});
    log(`open ${connected.info.speaks} ${connected.width}x${connected.height} over ${low(connected.path)} in ${connected.millis} ms`);
    if (hooks.connected) hooks.connected(state.device.id);
  }

  // Leave the viewer: the session ends, the page it came from returns.
  function closePipe(state) {
    if (!state.ws) return;
    try {
      state.ws.close();
    } catch (e) {}
    state.ws = null;
  }

  function close() {
    if (!cur) return;
    const state = cur;
    cur = null;
    attempt++;
    closePipe(state);
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
    chip.title = (cur.protected ? "How this session travels. Nobody in between can read it."
      : "How this session travels. It is not encrypted on this network.")
      + (cur.lastResponse != null ? ` The last click or key showed on the screen after ${Math.round(cur.lastResponse)} ms.` : "");
  }

  function setTools(info) {
    const on = !!info;
    for (const id of ["rv-keys", "rv-clip", "rv-picture", "rv-full"]) $(id).disabled = !on;
    document.querySelectorAll("#rv-touchbar button").forEach((b) => (b.disabled = !on));
    const not = (what) => (info && info.speaks === "rdp" ? `${what} over Remote Desktop is not built yet`
      : `${what}: not over VNC; the device needs Remote Desktop for it`);
    const set = (id, has, title, without) => {
      $(id).disabled = !(on && has);
      $(id).title = on && !has ? without : title;
    };
    set("rv-file", info && info.files, "Send files to the device", not("Sending files"));
    // The device's sound plays here unless its page says not to; the
    // button turns it off and on for the moment.
    const noSound = info && info.speaks === "rdp" ? "Its sound is turned off for this computer on its page"
      : "VNC has no sound; the device needs Remote Desktop for it";
    set("rv-sound", info && info.sound, "", noSound);
    if (cur) cur.soundOff = false;
    soundButton();
    // No kind of session has monitors to choose from yet: Remote Desktop
    // makes one screen for this window, VNC shows the device's as it is.
    $("rv-screens").classList.toggle("hidden", !(info && info.monitors));
    set("rv-screens", info && info.monitors, "Choose a monitor", "");
    // A computer running Kagaz, which its person goes on using: watch it
    // without touching, or control it.
    const paired = !!(cur && cur.device.paired);
    $("rv-mode").classList.toggle("hidden", !paired);
    if (cur) cur.watching = false;
    modeButtons();
  }

  function modeButtons() {
    const watching = !!(cur && cur.watching);
    $("rv-mode-control").classList.toggle("on", !watching);
    $("rv-mode-control").setAttribute("aria-pressed", watching ? "false" : "true");
    $("rv-mode-watch").classList.toggle("on", watching);
    $("rv-mode-watch").setAttribute("aria-pressed", watching ? "true" : "false");
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
  // A toast says something for a while; with `clicked`, a click on it
  // does that (and it says so by its pointer).
  function toast(text, stays, clicked) {
    const t = $("rv-toast");
    t.textContent = text;
    t.onclick = clicked || null;
    t.classList.toggle("rc-toast-act", !!clicked);
    t.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => t.classList.add("hidden"), stays || 3500);
  }
  const sentence = (e) => String(e).replace(/^./, (c) => c.toUpperCase()).replace(/\.?$/, ".");

  // ---------- the device's H.264, decoded here ----------
  // Each picture is decoded in order (each builds on the ones before) and
  // painted over its areas, the picture's top left at its origin. A
  // decoder that fails, or holds a picture back, hands the session back
  // to pixels: the device's whole screen comes once more, as it is.
  const H = typeof KagazPlayer !== "undefined" ? KagazPlayer.h264 : null;
  // Only where the decoder hands each picture over as soon as it is
  // decoded: Chromium's (Windows, Android). WebKitGTK's keeps one back
  // until the next arrives (measured 2026-11-19), so the last change on
  // screen would wait for another: there, and on WebKit elsewhere, the
  // pixels stay. A decoder found keeping pictures back is remembered.
  const HELD = "kagaz.remote.videoHeld";
  function videoHere() {
    let held = false;
    try {
      held = localStorage.getItem(HELD) === "1";
    } catch (e) {}
    return !!H && !held && typeof VideoDecoder === "function" && typeof EncodedVideoChunk === "function" && /Chrome\//.test(navigator.userAgent);
  }
  function videoOf(state) {
    if (!state.video) state.video = { dec: null, sps: null, pps: null, waiting: [], broken: false, t: 0, first: true };
    return state.video;
  }
  function videoClose(v) {
    if (v.dec && v.dec.state !== "closed") {
      try {
        v.dec.close();
      } catch (e) {}
    }
    v.dec = null;
    for (const w of v.waiting.splice(0)) w(null);
  }
  function videoFail(state, why) {
    const v = videoOf(state);
    if (v.broken) return;
    v.broken = true;
    videoClose(v);
    log(`H.264 is not decoded here (${why}); pixels from now on`);
    if (state.session != null) invoke("remote_video_off", { session: state.session }).catch(() => {});
    else state.videoOffOwed = true;
  }
  // The picture decoded, or null when there is none to paint.
  function videoPicture(state, stream, restart) {
    const v = videoOf(state);
    if (v.broken) return Promise.resolve(null);
    if (restart) videoClose(v);
    const units = H.nals(stream);
    let key = false, fresh = false;
    const slices = [];
    for (const u of units) {
      const t = u[0] & 31;
      if (t === 7) {
        fresh = fresh || !v.sps || v.sps.length !== u.length || v.sps.some((b, i) => b !== u[i]);
        v.sps = u.slice();
      } else if (t === 8) {
        fresh = fresh || !v.pps || v.pps.length !== u.length || v.pps.some((b, i) => b !== u[i]);
        v.pps = u.slice();
      } else if (t >= 1 && t <= 5) {
        slices.push(u);
        if (t === 5) key = true;
      }
    }
    if (!slices.length) return Promise.resolve(null);
    if (key && (fresh || !v.dec)) {
      if (!v.sps || !v.pps) {
        videoFail(state, "a whole picture came without its stream settings");
        return Promise.resolve(null);
      }
      videoClose(v);
      try {
        v.dec = new VideoDecoder({
          output: (frame) => {
            const done = v.waiting.shift();
            if (done) done(frame);
            else frame.close();
          },
          error: (e) => videoFail(state, `the decoder failed: ${(e && e.message) || e}`),
        });
        // Remote Desktop's H.264 is BT.709 at full range (MS-RDPEGFX), not
        // the TV range a decoder takes by default: its whites would glare
        // and its darks sink.
        v.dec.configure({
          codec: H.codec(v.sps),
          description: H.avcC(v.sps, v.pps),
          optimizeForLatency: true,
          colorSpace: { primaries: "bt709", transfer: "bt709", matrix: "bt709", fullRange: true },
        });
      } catch (e) {
        videoFail(state, `the decoder refused it: ${e.message || e}`);
        return Promise.resolve(null);
      }
    }
    // Nothing to build on yet: wait for a whole picture (pixels cover it).
    if (!v.dec) return Promise.resolve(null);
    return new Promise((resolve) => {
      // A decoder that keeps a picture back until the next arrives would
      // put a picture's worth of lag on every one: not here.
      const most = v.first ? 2000 : 300;
      const timer = setTimeout(() => {
        try {
          localStorage.setItem(HELD, "1");
        } catch (e) {}
        videoFail(state, `a picture took over ${most} ms`);
      }, most);
      v.waiting.push((frame) => {
        clearTimeout(timer);
        v.first = false;
        resolve(frame);
      });
      try {
        v.dec.decode(new EncodedVideoChunk({ type: key ? "key" : "delta", timestamp: v.t++ * 1000, data: H.avcc(slices) }));
      } catch (e) {
        videoFail(state, `decoding failed: ${e.message || e}`);
      }
    });
  }

  // ---------- the picture ----------
  // Updates are applied one after another, each whole, even while a
  // picture of the device's H.264 is being decoded.
  let applying = Promise.resolve();
  function feed(m) {
    applying = applying.then(() => apply(m)).catch((e) => log(`an update could not be shown (${e && e.message ? e.message : e})`));
  }
  async function apply(m) {
    const u8 = m instanceof Uint8Array ? m : new Uint8Array(m);
    if (!u8.length || !cur) return;
    const state = cur;
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
        else if (kind === 4) {
          const flags = u8[at - 15];
          const len = dv.getUint32(at, true);
          at += 4;
          const areas = [];
          for (let r = 0; r < w; r++, at += 8) areas.push([dv.getUint16(at, true), dv.getUint16(at + 2, true), dv.getUint16(at + 4, true), dv.getUint16(at + 6, true)]);
          const stream = u8.subarray(at, at + len);
          at += len;
          const frame = await videoPicture(state, stream, (flags & 2) !== 0);
          if (cur !== state) {
            if (frame) frame.close();
            return;
          }
          if (frame) {
            if (flags & 1) {
              ctx.save();
              ctx.beginPath();
              for (const [ax, ay, aw, ah] of areas) ctx.rect(ax, ay, aw, ah);
              ctx.clip();
              ctx.drawImage(frame, x, y);
              ctx.restore();
            }
            frame.close();
          }
        }
      }
      cur.frames++;
      if (cur.waitingAnswer) {
        cur.waitingAnswer = false;
        hideCard();
      }
      cur.paintMs += performance.now() - began;
      // The first change shown after a click or key: its response. One
      // longer than three seconds was not an answer to it.
      if (cur.asked != null) {
        const took = performance.now() - cur.asked;
        cur.asked = null;
        if (took < 3000) {
          cur.responses.push(took);
          cur.lastResponse = took;
        }
      }
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
        // Its size only, never what it says: whether copies arrive.
        log(`copied on the device: ${news.picture ? "a picture" : `${news.characters} characters`}${news.kept ? "" : ", not kept here"}`);
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
    // Watching a computer running Kagaz: this computer's mouse and keys
    // stay here.
    if (!outbox.length || !cur || cur.session == null || cur.watching) {
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
    // A click, a key or the wheel: the time until the screen next changes
    // is the session's response, all of it (the way there and back, the
    // device itself, its picture made, sent and shown). Moves are not
    // counted: one follows another.
    const acting = (d.t === "key" && d.down) || d.t === "wheel" || (d.t === "pointer" && d.buttons & ~cur.buttonsSaid);
    if (d.t === "pointer") cur.buttonsSaid = d.buttons;
    if (acting && cur.asked == null) cur.asked = performance.now();
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
  //
  // The system is asked how they are (remote_locks), at the moments they
  // can have changed: when the session opens, when the person comes back
  // to the screen, and after a lock key is let go. The page's own word is
  // not enough: this webview says "off" for Num Lock and Scroll Lock
  // whatever they are, which turned a computer's number pad the other way
  // round from this keyboard's. Where the system cannot be asked, the
  // page's word is taken, key by key, as before.
  const LOCKS = ["CapsLock", "NumLock", "ScrollLock"];
  let locksAsked = true; // the system answers
  function tellLocks(now) {
    if (!cur || cur.session == null || cur.held.size) return;
    if (cur.locks && now.every((on, i) => on === cur.locks[i])) return;
    cur.locks = now;
    did({ t: "locks", caps: now[0], num: now[1], scroll: now[2] }, true);
    const on = (yes) => (yes ? "on" : "off");
    log(`lock keys told to the device: Caps ${on(now[0])}, Num ${on(now[1])}, Scroll ${on(now[2])}`);
  }
  async function askLocks() {
    if (!locksAsked || !cur || cur.session == null || !cur.info || cur.info.speaks !== "rdp") return;
    const state = cur;
    try {
      const l = await invoke("remote_locks");
      if (cur === state) tellLocks([l.caps, l.num, l.scroll]);
    } catch (e) {
      locksAsked = false;
    }
  }
  function locks(ev) {
    if (locksAsked || !cur || cur.session == null || !cur.info || cur.info.speaks !== "rdp" || !ev.getModifierState) return;
    if (LOCKS.includes(ev.code)) return;
    tellLocks(LOCKS.map((k) => ev.getModifierState(k)));
  }

  // Something dragged on a Windows computer's screen (the left button
  // held a moment and moved): files can come here by its drop zone, which
  // Kagaz's helper there shows. Where it is not running, the person is
  // told once a session, and can start it.
  function draggedThere(p) {
    if (!cur || cur.helperTold || !cur.info || cur.info.speaks !== "rdp" || cur.device.paired) return;
    if (!(cur.buttons & LEFT)) {
      cur.pressedAt = null;
      return;
    }
    if (!cur.pressedAt) {
      cur.pressedAt = { t: performance.now(), x: p.x, y: p.y };
      return;
    }
    const far = Math.abs(p.x - cur.pressedAt.x) + Math.abs(p.y - cur.pressedAt.y);
    if (performance.now() - cur.pressedAt.t < 700 || far < 40) return;
    cur.helperTold = true;
    const state = cur;
    invoke("remote_helper_running", { session: cur.session }).then((running) => {
      if (running || cur !== state) return;
      toast(`To send files from ${cur.device.name} here, Kagaz's helper has to run there. Click to start it (once a session), then drag them onto the strip at the side of its screen.`, 12000, () => {
        invoke("remote_helper_start", { session: state.session }).catch((e) => toast(sentence(e), 6000));
      });
    }).catch(() => {});
  }

  function pointer(ev, now) {
    if (!cur || cur.session == null) return;
    locks(ev);
    const p = at(ev);
    cur.point = p;
    cur.buttons = held(ev);
    draggedThere(p);
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
    // A lock key let go: the device toggled with it; it is told how the
    // key stands here now, in case the two were apart.
    if (!down && LOCKS.includes(ev.code)) setTimeout(askLocks, 80);
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
    askLocks();
  });
  window.addEventListener("focus", () => {
    if (cur && cur.session != null) pushClipboard(true);
    wantKeys();
    askLocks();
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
    ["full-screen", "Full screen only"],
    ["window", "Window active, pointer inside"],
    ["never", "Never: they stay here"],
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
  //
  // Ctrl is pressed once as the pinch starts and held until the fingers
  // lift, the notches following a moment after the press: Windows takes
  // a session's keys and its mouse in by separate ways, so a press, a
  // notch and a release sent in the same instant can reach a program
  // out of order, and the notch then scrolls.
  let pinchSaid = false, pinchDone = 0;
  const PINCH_CTRL = { key: "Control", code: "ControlLeft", pinch: true };
  let pinchReady = 0, pinchUp = 0, pinchWheel = 0, pinchLetGo = 0;
  function wantPinch() {
    const on = !!cur && cur.session != null && !cur.away;
    if (on === pinchSaid) return;
    pinchSaid = on;
    invoke("remote_pinch", { on }).then((found) => {
      if (on) log(found ? "a pinch here zooms on the device" : "this webview's own pinch could not be turned off");
    }).catch(() => {});
  }
  function pinchWheelNow() {
    pinchWheel = 0;
    if (!pinchUp || !cur || cur.session == null) {
      pinchUp = 0;
      return;
    }
    const p = cur.point || { x: Math.round(cur.width / 2), y: Math.round(cur.height / 2) };
    did({ t: "wheel", x: p.x, y: p.y, up: pinchUp, right: 0 }, true);
    pinchUp = 0;
  }
  // The pinch's own Ctrl let go, once its last notch has gone.
  function pinchLift(after) {
    clearTimeout(pinchLetGo);
    pinchLetGo = setTimeout(() => {
      pinchLetGo = 0;
      if (pinchWheel) return pinchLift(after);
      if (cur && cur.held.get("ControlLeft") === PINCH_CTRL) {
        cur.held.delete("ControlLeft");
        did({ t: "key", key: "Control", code: "ControlLeft", down: false }, true);
      }
    }, after);
  }
  function pinch(scale, begin, end) {
    if (begin) pinchDone = 0;
    if (end) return pinchLift(120);
    if (begin || !cur || cur.session == null || cur.away || !(scale > 0)) return;
    const steps = Math.trunc(Math.log(scale) / Math.log(1.125));
    const up = steps - pinchDone;
    if (!up) return;
    if (!pinchDone) log("a pinch, passed on as zoom");
    pinchDone = steps;
    // With Ctrl already held here, the device has it held too.
    if (!cur.held.has("ControlLeft") && !cur.held.has("ControlRight")) {
      cur.held.set("ControlLeft", PINCH_CTRL);
      did({ t: "key", key: "Control", code: "ControlLeft", down: true }, true);
      pinchReady = performance.now() + 60;
    }
    pinchUp += up;
    if (!pinchWheel) pinchWheel = setTimeout(pinchWheelNow, Math.max(0, pinchReady - performance.now()));
    // Should the fingers' lifting never be told.
    pinchLift(1500);
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
      toast(sent.picture ? `Offered this computer's clipboard picture to ${cur.device.name}. Paste there to use it.`
        : sent.characters == null ? "There is no text or picture on this computer's clipboard."
        : `Offered ${sent.characters} characters to ${cur.device.name}. Paste there to use them.`);
    } catch (e) {
      if (!quietly) toast(sentence(e));
    }
  }

  // ---------- files ----------
  // Files go the way chosen for the device (remote_view.rs): through
  // Kagaz's drive there, with Windows asking where to save them (or
  // their folder opening); by its clipboard, to be pasted there; or over
  // SSH. A computer running Kagaz takes them by itself. Without `paths`
  // the person is asked which files. Dropped on its screen, they come
  // with the point they were let go at (`point`).
  async function sendFiles(paths, point) {
    if (!cur || cur.session == null) return;
    closePop();
    const there = cur.device.name;
    try {
      const at = point ? [point.x, point.y] : null;
      const sent = await invoke("remote_send_files", { session: cur.session, paths: paths || null, at });
      const what = sent.count === 1 ? "the file" : `${sent.count} files`;
      if (sent.how === "drop") {
        toast(`Saving ${what} where you let go on ${there}.`, 8000);
        log(`files dropped by the drive, ${sent.count}`);
      } else if (sent.how === "ask") {
        toast(`On ${there}, Windows asks where to save ${what}.`, 8000);
        log(`files sent by the drive, ${sent.count}, asked where`);
      } else if (sent.how === "open") {
        toast(`${what[0].toUpperCase() + what.slice(1)} opened in a folder on ${there} (the Kagaz drive).`, 8000);
        log(`files sent by the drive, ${sent.count}, folder opened`);
      } else if (sent.how === "ssh") {
        toast(`Sent ${what} to ${sent.folder} on ${there} over SSH.`, 8000);
        log(`files sent over SSH, ${sent.count}`);
      }
    } catch (e) {
      toast(sentence(e), 8000);
    }
    stage.focus();
  }
  // Files copied on the computer come here when this window loses the
  // focus: the person may be off to paste them in a folder here.
  window.addEventListener("blur", () => {
    if (!cur || cur.session == null || cur.away || !cur.info || cur.info.speaks !== "rdp" || cur.device.paired) return;
    invoke("remote_fetch_their_files", { session: cur.session, big: false }).catch(() => {});
  });

  // Files dragged here from this computer. To a Windows computer they
  // are put on the Kagaz drive at once, and Kagaz's helper there holds a
  // real Windows drag of them at its pointer (remote_drag_begin); when it
  // does, the button is pressed there, the drag moves with this one, and
  // letting go drops them there as any drop: into a folder, onto the
  // desktop, into a program or a page's upload box. Let go before the
  // helper holds it (it starts once a session), they are saved where
  // that was. Elsewhere (a computer running Kagaz, one whose files are to
  // be pasted) they go the way chosen for the device. Tauri says where in
  // the page's own pixels here (Linux, macOS), in the screen's on Windows.
  function dragAt(position) {
    if (!position || !cur || cur.session == null || cur.away || canvas.classList.contains("hidden")) return null;
    const k = /Windows/.test(navigator.userAgent) ? window.devicePixelRatio || 1 : 1;
    const ev = { clientX: position.x / k, clientY: position.y / k };
    const r = canvas.getBoundingClientRect();
    if (ev.clientX < r.left || ev.clientX >= r.right || ev.clientY < r.top || ev.clientY >= r.bottom) return null;
    return at(ev);
  }
  // The pointer there, with the button held once the drag is.
  function dragMove(p, now) {
    const buttons = cur.drag && cur.drag.pressed ? LEFT : 0;
    cur.point = p;
    cur.buttons = buttons;
    const last = outbox[outbox.length - 1];
    if (!now && last && last.t === "pointer" && last.buttons === buttons) outbox.pop();
    did({ t: "pointer", x: p.x, y: p.y, buttons }, now);
  }
  function dragPress() {
    const d = cur && cur.drag;
    if (!d || !d.ready || d.pressed || !d.over) return;
    d.pressed = true;
    dragMove(d.over, true);
    log("files dragged: held there");
  }
  // The held drag let go of without a drop there: Escape ends it.
  function dragLetGo(d) {
    if (!d.pressed) return;
    did({ t: "key", key: "Escape", code: "Escape", down: true }, true);
    did({ t: "key", key: "Escape", code: "Escape", down: false }, true);
    d.pressed = false;
    if (d.over) dragMove(d.over, true);
  }
  async function dragEnter(payload) {
    if (!cur || cur.session == null || cur.away) return;
    const paths = (payload && payload.paths) || [];
    if (!paths.length || cur.drag) return;
    const d = { paths, id: null, ready: false, pressed: false, over: dragAt(payload.position), state: cur };
    cur.drag = d;
    toast(`Let go where they should go on ${cur.device.name}.`, 4000);
    if (d.over) dragMove(d.over, false);
    try {
      d.id = await invoke("remote_drag_begin", { session: d.state.session, paths });
    } catch (e) {
      log(`files dragged: ${sentence(e)}`);
    }
    // Over before the drive had them (they went the device's way): the
    // helper there is not to hold them.
    if (d.id && (!cur || cur.drag !== d)) {
      invoke("remote_drag_end", { session: d.state.session, id: d.id, pressed: false, at: null }).catch(() => {});
    }
  }
  function dragReady(p) {
    const d = cur && cur.drag;
    if (!d || d.id !== p.id) return;
    if (p.failed) {
      log(`files dragged: the helper there did not start (${p.failed})`);
      return;
    }
    d.ready = true;
    dragPress();
  }
  function dragOver(position) {
    const d = cur && cur.drag;
    if (!d) return;
    d.over = dragAt(position);
    if (!d.over) return;
    if (d.ready && !d.pressed) dragPress();
    else dragMove(d.over, false);
  }
  function dragLeave() {
    const d = cur && cur.drag;
    if (!d) return;
    cur.drag = null;
    dragLetGo(d);
    if (d.id) invoke("remote_drag_end", { session: cur.session, id: d.id, pressed: false, at: null }).catch(() => {});
    log("files dragged away again");
  }
  async function dropped(payload) {
    const d = cur && cur.drag;
    if (!cur || cur.session == null || cur.away) return;
    cur.drag = null;
    const paths = (payload && payload.paths) || (d && d.paths) || [];
    if (!paths.length) return;
    const p = dragAt(payload && payload.position);
    if (!d || !d.id) {
      // Not to be dragged there: the way chosen for the device.
      sendFiles(paths, p);
      return;
    }
    const there = cur.device.name;
    if (d.pressed && p) {
      dragMove(p, true);
      d.pressed = false;
      dragMove(p, true);
      try {
        const end = await invoke("remote_drag_end", { session: cur.session, id: d.id, pressed: true, at: null });
        if (end.effect === 0) toast(`Nothing there took the files on ${there}.`, 6000);
        log(`files dropped there, effect ${end.effect}`);
      } catch (e) {
        toast(sentence(e), 8000);
      }
      return;
    }
    // Let go off its screen, or before the drag was held there.
    dragLetGo(d);
    try {
      const end = await invoke("remote_drag_end", { session: cur.session, id: d.id, pressed: false, at: p ? [p.x, p.y] : null });
      if (end.how === "saving") {
        const what = paths.length === 1 ? "the file" : `${paths.length} files`;
        toast(`Saving ${what} where you let go on ${there}.`, 8000);
        log(`files dropped before the drag was held there, ${paths.length}`);
      } else {
        sendFiles(paths);
      }
    } catch (e) {
      toast(sentence(e), 8000);
    }
  }

  // Large files take a moment to be got ready for the drive.
  function sendReady(p) {
    if (!cur || !(p.total > 20e6)) return;
    toast(p.done >= p.total ? "Ready; opening it there…" : `Getting the files ready: ${Math.floor((p.done / p.total) * 100)}%`, 4000);
  }

  const size = (bytes) => (bytes < 1000 ? `${bytes} bytes` : bytes < 1e6 ? `${Math.round(bytes / 1000)} KB` : megabytes(bytes));
  // How the files offered are getting on (see `Sending` in the core).
  function sending(news) {
    const there = cur.device.name;
    if (news.state === "offered") {
      const one = news.names.length === 1;
      cur.offer = { waiting: true };
      // Said until they go or are withdrawn: nothing happens until then.
      toast(`Waiting for a paste on ${there}: ${one ? news.names[0] : `${news.names.length} files`} (${size(news.bytes)}). `
        + `Open a folder there and press Ctrl + V.`, 10 * 60 * 1000);
      log(`files offered ${news.names.length}, ${news.bytes} bytes`);
    } else if (news.state === "going") {
      if (cur.offer) cur.offer.waiting = false;
      toast(`Sending ${news.name}: ${Math.floor((news.sent / Math.max(news.size, 1)) * 100)}%`, 10000);
    } else if (news.state === "taken") {
      if (cur.offer) cur.offer.waiting = false;
      toast(`${news.name} is on ${there}.`);
      log(`file taken, ${news.size} bytes`);
    } else if (news.state === "step") {
      // The clipboard's own steps, for the log only.
      log(`files: ${news.step}`);
    } else if (news.state === "withdrawn") {
      log(`files withdrawn${cur.offer && cur.offer.waiting ? " before they were pasted" : ""}`);
      // Worth saying only while they were still waiting to be pasted.
      if (cur.offer && cur.offer.waiting) toast("The files are no longer on offer: something else was copied since.", 8000);
      cur.offer = null;
    } else if (news.state === "failed") {
      toast(sentence(news.why), 6000);
      log("files not sent");
    } else if (news.state === "theirsbig") {
      // Copied there, more than is fetched without asking.
      const what = news.count === 1 ? "a file" : `${news.count} files`;
      toast(`${what[0].toUpperCase() + what.slice(1)} copied on ${there} (${size(news.bytes)}). Click to bring them here to paste.`, 12000, () => {
        invoke("remote_fetch_their_files", { session: cur.session, big: true }).catch(() => {});
      });
      log(`files copied there, ${news.count}, ${news.bytes} bytes: asked first`);
    } else if (news.state === "fetching") {
      if (news.total > 20e6 && news.done < news.total) {
        toast(`Bringing the files copied on ${there} here: ${Math.floor((news.done / Math.max(news.total, 1)) * 100)}%`, 4000);
      }
    } else if (news.state === "fetched") {
      const what = news.count === 1 ? "The file" : `The ${news.count} files`;
      toast(`${what} copied on ${there} can be pasted here now (Ctrl + V in a folder).`, 8000);
      log(`files copied there fetched, ${news.count}`);
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

  for (const [id, watching] of [["rv-mode-control", false], ["rv-mode-watch", true]]) {
    $(id).addEventListener("click", () => {
      if (!cur) return;
      cur.watching = watching;
      outbox = [];
      modeButtons();
      invoke("page_log", { line: `remote: ${watching ? "watching" : "in control"}` }).catch(() => {});
      stage.focus();
    });
  }

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
  // The responses measured lately: the middle one and the slowest (and
  // with `take`, they are let go, for the next five seconds' line).
  function response(take) {
    if (!cur || !cur.responses.length) return null;
    const sorted = [...cur.responses].sort((a, b) => a - b);
    const out = { middle: Math.round(sorted[Math.floor(sorted.length / 2)]), worst: Math.round(sorted[sorted.length - 1]), count: sorted.length };
    if (take) cur.responses = [];
    else if (cur.responses.length > 40) cur.responses = cur.responses.slice(-40);
    return out;
  }
  function numbersLine(r) {
    const parts = [r.delay != null ? `${r.delay} ms delay` : "delay not measured yet"];
    if (lastReading && r.seconds > lastReading.seconds) {
      const rate = ((r.received - lastReading.received) * 8) / (r.seconds - lastReading.seconds) / 1e6;
      parts.push(`${rate.toFixed(rate >= 10 ? 0 : 1)} Mbit/s now`);
    }
    const answer = response(false);
    if (answer) parts.push(`${answer.middle} ms from a click to the screen`);
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
      : "Over Remote Desktop the computer suits its picture to the connection and draws its screen at this window's size.");
    // What cannot be chosen here is not offered.
    const parts = chooses ? [slider, autoLabel, sizeLabel] : [];
    // A Windows computer reached over Remote Desktop can draw and encode
    // its screen with its graphics chip (H.264): Windows' own settings,
    // which Kagaz can turn on there (Windows asks for an administrator).
    const windows = cur.info && cur.info.speaks === "rdp" && !cur.device.paired && (!cur.device.system || cur.device.system === "windows");
    if (windows) {
      const chip = el("div", "rv-chip-gpu");
      const drawChip = () => {
        const via = (lastReading && lastReading.via) || "";
        const h264 = /H\.264 \d/.test(via);
        const on = !!(cur.settings && cur.settings.graphics_chip);
        chip.replaceChildren(
          el("strong", "", "Its graphics chip"),
          el("p", "muted small", h264 ? "Its pictures come as H.264, encoded by Windows there."
            : on ? "Turned on there; it takes effect once Windows there is signed out and in again."
              : "Its pictures come without H.264: its processor draws and encodes them, which lags more."),
        );
        const b = el("button", "", on ? "Put back as it was" : "Use it there…");
        b.type = "button";
        b.addEventListener("click", async () => {
          b.disabled = true;
          toast(`On ${cur.device.name}, Windows asks for an administrator: answer it there. Press nothing there until it shows.`, 120000);
          try {
            const said = await invoke("remote_graphics", { session: cur.session, on: !on });
            cur.settings = { ...cur.settings, graphics_chip: !on };
            if (on) delete cur.settings.graphics_chip;
            if (hooks.settings) hooks.settings(cur.device.id, cur.settings);
            toast(said, 15000);
            log(`its graphics chip ${on ? "put back" : "turned on"} there`);
          } catch (e) {
            toast(sentence(e), 10000);
          }
          b.disabled = false;
          drawChip();
        });
        const row = el("div", "rc-buttons start");
        row.append(b);
        chip.append(row);
      };
      drawChip();
      parts.push(chip);
    }
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
    const answer = response(true);
    const answered = answer ? `, a click or key answered in ${answer.middle} ms (slowest ${answer.worst}, of ${answer.count})` : "";
    log(`${(frames / seconds).toFixed(1)} updates/s, ${((now.paintMs - was.paintMs) / frames).toFixed(2)} ms to paint one, `
      + `${((now.bytes - was.bytes) / seconds / 1e6).toFixed(1)} MB/s to the page, delay ${r && r.delay != null ? r.delay + " ms" : "unknown"}${work}${answered}`
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
  // Both are said to this window alone (each session has its own): heard
  // through the window's own ear, which hears what is said to all too.
  const events = window.__TAURI__ && window.__TAURI__.event;
  const mine = window.__TAURI__ && window.__TAURI__.webviewWindow && window.__TAURI__.webviewWindow.getCurrentWebviewWindow
    ? window.__TAURI__.webviewWindow.getCurrentWebviewWindow() : null;
  const hear = (name, fn) => (mine ? mine.listen(name, fn) : events.listen(name, fn));
  if (events && events.listen) {
    hear("remote-pinch", (ev) => pinch(ev.payload.scale, ev.payload.begin, ev.payload.end));
    hear("remote-send-ready", (ev) => sendReady(ev.payload));
    hear("tauri://drag-enter", (ev) => dragEnter(ev.payload));
    hear("tauri://drag-over", (ev) => dragOver(ev.payload && ev.payload.position));
    hear("tauri://drag-leave", () => dragLeave());
    hear("tauri://drag-drop", (ev) => dropped(ev.payload));
    hear("remote-drag-ready", (ev) => dragReady(ev.payload));
    // Files from the computer arrived here (remote_view.rs's `arrived`).
    hear("remote-received", (ev) => {
      const r = ev.payload || {};
      if (!cur) return;
      const what = r.count === 1 ? "A file" : `${r.count} files`;
      if (!r.kept) {
        toast(r.why ? `${what} from ${cur.device.name} could not be saved: ${r.why}.` : `${what} from ${cur.device.name}: not saved.`, 6000);
        log(`files from the device not kept, ${r.count}`);
        return;
      }
      const folder = String(r.folder || "").split("/").filter(Boolean).pop() || "the folder";
      toast(`${what} from ${cur.device.name} in ${folder}. Click to show.`, 10000, () => {
        invoke("show_in_folder", { path: String(r.first || r.folder) }).catch(() => {});
      });
      log(`files from the device kept, ${r.count}`);
    });
    // Kagaz put a line in Windows' Run box there: run by the person, who
    // sees where it is (Kagaz cannot see what has the focus there).
    hear("remote-run-box", () => {
      if (!cur || cur.session == null) return;
      toast(`Kagaz's helper is not running on ${cur.device.name} yet: press Enter in the Run box there (once a session). `
        + `Closed it? Try again in a few seconds and it opens again. (Kagaz share there, with "Take them without the Run box" on, needs no Run box.)`, 20000);
      log("run box: line pasted, waiting for Enter there");
    });
    hear("remote-close-asked", async () => {
      if (cur && cur.session != null) {
        const yes = await KagazAsk.confirm(
          hooks.window ? `End the session with ${cur.device.name}?` : `Close Kagaz and end the session with ${cur.device.name}?`,
          hooks.window ? "End session" : "Close Kagaz", "Everything open on it stays open there.", "Stay");
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

  // The device's settings changed (behind the session window's gear, or
  // on its page): where this computer's keys go follows at once; the
  // rest at the next connection.
  function settings(next) {
    if (!cur) return;
    cur.settings = { ...next };
    wantKeys();
  }

  return { open, close, shown, settings, isOpen: () => !!cur };
})();
