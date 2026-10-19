// Settings. The app's own (the accounts Kagaz is signed in to, and about)
// are on the Settings page; each kind of device keeps its settings on its
// own page, behind its gear: the doorbell's here are its ring sounds, for
// every doorbell first and then per kind (OneTouch, stations on this
// network; other brands add their own section).
//
// KagazRing plays the ring: the tone chosen for the caller (a device's
// own, else its kind's), on a loop, until stopped.
// Uses app.js's globals: $, invoke, selectCategory.

const KagazRing = (() => {
  let listing = null; // { tones, settings } from Rust
  const buffers = new Map(); // tone id -> AudioBuffer
  let playing = null; // { source, gain }

  async function tones() {
    if (!listing) listing = await invoke("ringtones_list");
    return listing;
  }
  function forget() {
    listing = null;
  }

  async function buffer(id) {
    if (buffers.has(id)) return buffers.get(id);
    const bytes = await invoke("ringtone_audio", { id });
    const data = bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes).buffer;
    const decoded = await KagazPlayer.audioContext().decodeAudioData(data.slice(0));
    buffers.set(id, decoded);
    return decoded;
  }

  function toneFor(settings, peer, kind) {
    const own = settings.devices[(peer || "").trim().toLowerCase()];
    return own || settings.events[kind] || settings.events.all || "soft-chime";
  }

  async function play(id, loop, volume) {
    stop();
    const ctx = KagazPlayer.audioContext();
    const gain = ctx.createGain();
    gain.gain.value = volume;
    gain.connect(ctx.destination);
    const source = ctx.createBufferSource();
    source.buffer = await buffer(id);
    source.loop = loop;
    source.connect(gain);
    source.start();
    playing = { source, gain };
    source.onended = () => { if (playing && playing.source === source) ended(); };
  }

  // Told once when the tone playing now stops (a Listen button turns back).
  let onStop = [];
  function whenStopped(fn) {
    onStop.push(fn);
  }
  function ended() {
    playing = null;
    const fns = onStop;
    onStop = [];
    for (const fn of fns) fn();
  }

  // Ring for `peer` (a device's or person's name) of `kind`.
  async function start(peer, kind) {
    try {
      const { settings } = await tones();
      await play(toneFor(settings, peer, kind), true, settings.volume);
    } catch (e) {
      // No sound file could be played: a plain two-note chime instead.
      beep();
    }
  }

  function beep() {
    const ctx = KagazPlayer.audioContext();
    const t = ctx.currentTime;
    for (const [freq, at, len] of [[784, 0, 0.5], [622, 0.35, 0.8]]) {
      const o = ctx.createOscillator();
      const g = ctx.createGain();
      o.frequency.value = freq;
      g.gain.setValueAtTime(0.0001, t + at);
      g.gain.exponentialRampToValueAtTime(0.4, t + at + 0.02);
      g.gain.exponentialRampToValueAtTime(0.0001, t + at + len);
      o.connect(g).connect(ctx.destination);
      o.start(t + at);
      o.stop(t + at + len + 0.05);
    }
  }

  function stop() {
    if (!playing) return;
    try { playing.source.stop(); } catch (_) {}
    try { playing.gain.disconnect(); } catch (_) {}
    ended();
  }

  // Hear a tone once (Settings).
  async function preview(id, volume) {
    try { await play(id, false, volume); } catch (e) { beep(); }
  }

  return { start, stop, preview, tones, forget, toneFor, whenStopped, isPlaying: () => !!playing };
})();

// Helpers shared by the settings pages.
const KagazSet = (() => {
  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }
  function section(title, note) {
    const box = el("section", "set-section");
    box.appendChild(el("h3", "", title));
    if (note) box.appendChild(el("p", "muted small", note));
    return box;
  }
  // A row: a label (and its note) on the left, controls on the right. The
  // label belongs to the row's first field, so tapping its words works too.
  let ids = 0;
  function row(label, note, ...controls) {
    const r = el("div", "set-row");
    const l = el("label", "set-label");
    l.appendChild(el("span", "", label));
    if (note) l.appendChild(el("span", "muted small", note));
    const c = el("span", "set-control");
    c.append(...controls);
    const field = c.querySelector("input, select");
    if (field) {
      if (!field.id) field.id = `set-field-${++ids}`;
      l.htmlFor = field.id;
    }
    r.append(l, c);
    return r;
  }
  // Save a change; when it fails, put the control back and say so in its row.
  async function attempt(r, work, undo) {
    let err = r.querySelector(".set-error");
    try {
      await work();
      if (err) err.remove();
    } catch (e) {
      if (undo) undo();
      if (!err) {
        err = el("span", "small cap-warn set-error");
        err.setAttribute("role", "alert");
        r.querySelector(".set-label").appendChild(err);
      }
      err.textContent = `Not saved: ${e}`;
    }
  }
  return { el, section, row, attempt };
})();

// The doorbell's settings page (its gear).
const KagazDoorbellSettings = (() => {
  const { el, section, row, attempt } = KagazSet;
  let listing = null;

  function toneSelect(value, same) {
    const sel = document.createElement("select");
    if (same) {
      const o = el("option", "", same);
      o.value = "";
      sel.appendChild(o);
    }
    for (const t of listing.tones) {
      const o = el("option", "", t.own ? `${t.name} (yours)` : t.name);
      o.value = t.id;
      sel.appendChild(o);
    }
    sel.value = value || "";
    return sel;
  }

  function playButton(getId) {
    const b = el("button", "icon play-tone", "▶");
    b.type = "button";
    b.title = "Listen";
    b.setAttribute("aria-label", "Listen");
    const shown = (on) => {
      b.textContent = on ? "■" : "▶";
      b.title = on ? "Stop" : "Listen";
      b.setAttribute("aria-label", b.title);
    };
    b.addEventListener("click", async () => {
      if (KagazRing.isPlaying()) { KagazRing.stop(); return; }
      const id = getId();
      if (!id) return;
      await KagazRing.preview(id, listing.settings.volume);
      if (KagazRing.isPlaying()) {
        shown(true);
        KagazRing.whenStopped(() => shown(false));
      }
    });
    return b;
  }

  async function save() {
    const s = listing.settings;
    listing = await invoke("ringtones_set", { events: s.events, devices: s.devices, volume: s.volume });
    KagazRing.forget();
  }

  // A tone row for one kind (`key` in events), falling back to every doorbell's.
  function kindTone(key, label, note) {
    const s = listing.settings;
    const sel = toneSelect(s.events[key] || "", key === "all" ? null : "Same as every doorbell");
    if (key === "all") sel.value = s.events.all || "soft-chime";
    sel.setAttribute("aria-label", `${label} tone`);
    const r = row(label, note, sel, playButton(() => sel.value || s.events.all || "soft-chime"));
    let was = sel.value;
    sel.addEventListener("change", () => {
      const before = s.events[key];
      if (sel.value) s.events[key] = sel.value; else delete s.events[key];
      attempt(r, async () => { await save(); was = sel.value; }, () => {
        if (before) s.events[key] = before; else delete s.events[key];
        sel.value = was;
      });
    });
    return r;
  }

  // One device's own tone.
  function deviceTone(name, kindKey) {
    const s = listing.settings;
    const key = name.trim().toLowerCase();
    const sel = toneSelect(s.devices[key] || "", "Same as its kind");
    sel.setAttribute("aria-label", `${name} tone`);
    const r = row(name, "", sel, playButton(() => sel.value || s.events[kindKey] || s.events.all || "soft-chime"));
    let was = sel.value;
    sel.addEventListener("change", () => {
      const before = s.devices[key];
      if (sel.value) s.devices[key] = sel.value; else delete s.devices[key];
      attempt(r, async () => { await save(); was = sel.value; }, () => {
        if (before) s.devices[key] = before; else delete s.devices[key];
        sel.value = was;
      });
    });
    return r;
  }

  // How a ring shows on this device: the notification with the ring sound,
  // or the notification only (never a sound alone); and, on a phone,
  // vibration with either.
  function ringStyle() {
    const s = listing.settings;
    const phone = /Android|iPhone|iPad/i.test(navigator.userAgent);
    const box = document.createDocumentFragment();
    const style = document.createElement("select");
    for (const [value, text] of [["ring", "Notification and ring sound"], ["silent", "Silent notification"]]) {
      const o = el("option", "", text);
      o.value = value;
      style.appendChild(o);
    }
    style.value = s.silent ? "silent" : "ring";
    style.setAttribute("aria-label", "When someone rings");
    const note = () => (style.value === "silent"
      ? "On this device. The notification only; tap it to open the call. The tones below play again if you switch back."
      : phone ? "On this device. A volume key quiets a ring." : "On this device.");
    const styleRow = row("When someone rings", note(), style);
    box.appendChild(styleRow);
    const vibrate = document.createElement("input");
    vibrate.type = "checkbox";
    vibrate.className = "toggle";
    vibrate.checked = s.vibrate !== false;
    let vibrateRow = null;
    const save = (r, undo) => attempt(r, async () => {
      listing = await invoke("ring_style_set", { silent: style.value === "silent", vibrate: vibrate.checked });
      const n = styleRow.querySelector(".set-label .muted");
      if (n) n.textContent = note();
    }, undo);
    let wasStyle = style.value;
    style.addEventListener("change", () => save(styleRow, () => { style.value = wasStyle; }).then(() => { wasStyle = style.value; }));
    vibrate.addEventListener("change", () => save(vibrateRow, () => { vibrate.checked = !vibrate.checked; }));
    if (phone) {
      vibrate.setAttribute("aria-label", "Vibrate");
      vibrateRow = row("Vibrate", "While it rings, in either case.", vibrate);
      box.appendChild(vibrateRow);
    }
    return box;
  }

  function drawAll(box) {
    const s = listing.settings;
    const sec = section("Every doorbell", "The ring for any doorbell, unless its own section below says otherwise.");
    sec.appendChild(ringStyle());
    sec.appendChild(kindTone("all", "Ringtone", "Calm tones made by Kagaz, or your own."));
    const range = document.createElement("input");
    range.type = "range";
    range.min = "0.1";
    range.max = "1";
    range.step = "0.05";
    range.value = String(s.volume);
    range.setAttribute("aria-label", "Ring volume");
    const volumeRow = row("Ring volume", "", range);
    let wasVolume = s.volume;
    range.addEventListener("change", () => {
      s.volume = Number(range.value);
      attempt(volumeRow, async () => { await save(); wasVolume = s.volume; }, () => {
        s.volume = wasVolume;
        range.value = String(wasVolume);
      });
    });
    sec.appendChild(volumeRow);
    // Own tones.
    for (const t of listing.tones.filter((t) => t.own)) {
      const rm = el("button", "", "Remove");
      rm.type = "button";
      rm.setAttribute("aria-label", `Remove ${t.name}`);
      const r = row(t.name, "Your tone", playButton(() => t.id), rm);
      rm.addEventListener("click", async () => {
        if (!(await KagazAsk.confirm(`Remove the tone ${t.name}?`, "Remove tone", "Rings that use it go back to every doorbell's tone."))) return;
        attempt(r, async () => {
          listing = await invoke("ringtone_remove", { id: t.id });
          KagazRing.forget();
          draw();
        });
      });
      sec.appendChild(r);
    }
    const pick = document.createElement("input");
    pick.type = "file";
    pick.accept = "audio/*,.mp3,.m4a,.aac,.ogg,.opus,.wav,.flac";
    pick.className = "hidden";
    const add = el("button", "", "+ Add your own tone");
    add.type = "button";
    const note = el("p", "muted small");
    add.addEventListener("click", () => pick.click());
    pick.addEventListener("change", async () => {
      const file = pick.files && pick.files[0];
      pick.value = "";
      if (!file) return;
      if (file.size > 15 * 1024 * 1024) { note.textContent = "Choose an audio file under 15 MB."; return; }
      note.textContent = "Adding…";
      try {
        const data = await new Promise((resolve, reject) => {
          const r = new FileReader();
          r.onload = () => resolve(String(r.result).split(",")[1] || "");
          r.onerror = () => reject(r.error);
          r.readAsDataURL(file);
        });
        listing = await invoke("ringtone_add", { name: file.name.replace(/\.[^.]+$/, ""), fileName: file.name, data });
        KagazRing.forget();
        draw();
      } catch (e) {
        note.textContent = String(e);
      }
    });
    const own = el("div", "set-own");
    own.append(add, pick, note);
    sec.appendChild(own);
    box.appendChild(sec);
  }

  async function drawOneTouch(box) {
    const [calls, ot] = await Promise.all([invoke("calls_status").catch(() => null), invoke("onetouch_peek").catch(() => null)]);
    if (!ot || !(ot.logged_in || ot.remembered || (calls && calls.devices && calls.devices.length))) return;
    const sec = section("OneTouch", ot.logged_in ? `Signed in as ${ot.username}.` : "Your OneTouch account.");
    const sw = document.createElement("input");
    sw.type = "checkbox";
    sw.className = "toggle";
    sw.checked = !!(calls && calls.enabled);
    sw.setAttribute("aria-label", "Ring me here");
    let ringRow = null;
    sw.addEventListener("change", () => attempt(ringRow, async () => {
      await invoke("calls_enable", { enabled: sw.checked });
      if ($("calls-enabled")) $("calls-enabled").checked = sw.checked;
    }, () => { sw.checked = !sw.checked; }));
    sec.appendChild(ringRow = row("Ring me here", /Android/i.test(navigator.userAgent)
      ? "Ring on this phone for calls to your OneTouch account, also after you close Kagaz (a small “Ready for doorbell calls” notification shows it is listening). If it stops after a while, let Kagaz run in the background in Android's battery settings."
      : "Ring on this computer for calls to your OneTouch account.", sw));
    sec.appendChild(kindTone("doorbell", "Door stations and monitors", "A ring from a door station, an indoor monitor or a guard phone."));
    sec.appendChild(kindTone("family", "Family", "A call from someone in your home with their own login."));
    // A tone per device.
    const names = [];
    for (const d of (calls && calls.devices) || []) if (d.kind !== "apps") names.push([d.name, "doorbell"]);
    if (typeof otPeople !== "undefined") for (const p of otPeople) names.push([p.name, "family"]);
    if (names.length) {
      const per = el("details", "set-more");
      per.appendChild(el("summary", "", "A tone for one device or person"));
      for (const [name, kind] of names) per.appendChild(deviceTone(name, kind));
      sec.appendChild(per);
    }
    box.appendChild(sec);
  }

  async function drawLocal() {
    const slot = $("door-settings-local-tone");
    slot.innerHTML = "";
    slot.appendChild(kindTone("local", "Ringtone", ""));
    try {
      const st = await invoke("doorbell_stations");
      if ((st.stations || []).length) {
        const per = el("details", "set-more");
        per.appendChild(el("summary", "", "A tone for one station"));
        for (const s of st.stations) per.appendChild(deviceTone(s.name, "local"));
        slot.appendChild(per);
      }
    } catch (_) {}
  }

  async function draw() {
    const all = $("door-settings-all");
    const one = $("door-settings-onetouch");
    all.innerHTML = "";
    one.innerHTML = "";
    drawAll(all);
    await drawOneTouch(one);
    await drawLocal();
  }

  async function show() {
    $("door-settings-all").innerHTML = '<p class="muted">Loading…</p>';
    try {
      listing = await invoke("ringtones_list");
    } catch (e) {
      $("door-settings-all").textContent = String(e);
      return;
    }
    await draw();
  }

  return { show };
})();

// The app's Settings page: what applies to the whole app.
const KagazSettings = (() => {
  const { el, section, row } = KagazSet;

  async function drawAccounts(body) {
    const box = section("Accounts", "Kagaz signs in to each service the way its own app does. Passwords stay in this device's password store.");
    const go = (label, cat) => {
      const b = el("button", "", label);
      b.type = "button";
      b.addEventListener("click", () => selectCategory(cat));
      return b;
    };
    const loading = el("p", "muted", "Loading…");
    body.appendChild(loading);
    try {
      const ot = await invoke("onetouch_peek");
      box.appendChild(row("OneTouch (doorbell)", ot.logged_in ? `Signed in as ${ot.username}` : ot.remembered ? `Saved sign-in: ${ot.remembered.username}` : "Not signed in", go("Open Doorbell", "doorbell")));
    } catch (_) {}
    try {
      const cam = await invoke("cameras_status");
      box.appendChild(row("Tapo (security cameras)", cam.logged_in ? "Signed in" : "Not signed in", go("Open Security cameras", "cameras")));
    } catch (_) {}
    loading.remove();
    body.appendChild(box);
  }

  function drawDevices(body) {
    const box = section("Device settings", "Each kind of device keeps its settings on its own page, behind its ⚙ button.");
    const go = (label, cat) => {
      const b = el("button", "", "Open doorbell settings");
      b.type = "button";
      b.addEventListener("click", () => {
        selectCategory(cat);
        if (cat === "doorbell") doorSettings(true);
      });
      return row(label, "", b);
    };
    box.append(go("Doorbell: ringtones, ringing here, stations on this network", "doorbell"));
    body.appendChild(box);
  }

  function drawAbout(body) {
    const box = section("About Kagaz");
    box.appendChild(el("p", "muted small", "Free and open-source (GPL-3.0). Brand names and logos belong to their owners and are shown only to say which devices Kagaz works with."));
    body.appendChild(box);
  }

  async function show() {
    const body = $("settings-body");
    body.innerHTML = "";
    await drawAccounts(body);
    drawDevices(body);
    drawAbout(body);
  }

  return { show };
})();
