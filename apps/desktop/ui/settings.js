// Settings for the whole app: ring sounds, ringing on this device, the
// accounts Kagaz is signed in to, and about. A kind of device's own
// settings stay on its page (its gear button).
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
    return own || settings.events[kind] || "soft-chime";
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
    source.onended = () => { if (playing && playing.source === source) playing = null; };
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
    playing = null;
  }

  // Hear a tone once (Settings).
  async function preview(id, volume) {
    try { await play(id, false, volume); } catch (e) { beep(); }
  }

  return { start, stop, preview, tones, forget, toneFor, isPlaying: () => !!playing };
})();

const KagazSettings = (() => {
  const KINDS = [
    ["doorbell", "Door stations and monitors", "A ring from a door station, an indoor monitor or a guard phone on your OneTouch account."],
    ["family", "Family", "A call from someone in your home with their own OneTouch login."],
    ["local", "Door stations on this network", "A station ringing this device directly (Local intercom)."],
  ];
  let listing = null;
  let devices = []; // [{ name, kind }] to set a tone for

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

  function toneSelect(value, withSame) {
    const sel = document.createElement("select");
    if (withSame) {
      const o = el("option", "", "Same as its kind");
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
    b.addEventListener("click", () => {
      if (KagazRing.isPlaying()) { KagazRing.stop(); return; }
      const id = getId();
      if (id) KagazRing.preview(id, listing.settings.volume);
    });
    return b;
  }

  async function save() {
    const s = listing.settings;
    listing = await invoke("ringtones_set", { events: s.events, devices: s.devices, volume: s.volume });
    KagazRing.forget();
  }

  // The devices that can ring: the account's call devices, family, stations.
  async function ringers() {
    const out = [];
    const seen = new Set();
    const add = (name, kind) => {
      const key = (name || "").trim().toLowerCase();
      if (!key || seen.has(key)) return;
      seen.add(key);
      out.push({ name, kind });
    };
    try {
      const calls = await invoke("calls_status");
      for (const d of calls.devices || []) if (d.kind !== "apps") add(d.name, d.kind === "family" ? "family" : "doorbell");
    } catch (_) {}
    if (typeof otPeople !== "undefined") for (const p of otPeople) add(p.name, "family");
    try {
      const st = await invoke("doorbell_stations");
      for (const s of st.stations || []) add(s.name, "local");
    } catch (_) {}
    return out;
  }

  function drawTones(body) {
    const s = listing.settings;
    const box = section("Ringtones", "Calm tones made by Kagaz, or your own songs. A device can have its own tone; otherwise it rings with its kind's.");
    for (const [kind, name, note] of KINDS) {
      const row = el("div", "set-row");
      const label = el("label", "set-label");
      label.appendChild(el("span", "", name));
      label.appendChild(el("span", "muted small", note));
      const sel = toneSelect(s.events[kind] || "soft-chime", false);
      sel.setAttribute("aria-label", `${name} tone`);
      sel.addEventListener("change", async () => { s.events[kind] = sel.value; await save(); });
      row.append(label, el("span", "set-control"));
      row.lastChild.append(sel, playButton(() => sel.value));
      box.appendChild(row);
    }
    const vol = el("div", "set-row");
    const vl = el("label", "set-label");
    vl.appendChild(el("span", "", "Ring volume"));
    const range = document.createElement("input");
    range.type = "range";
    range.min = "0.1";
    range.max = "1";
    range.step = "0.05";
    range.value = String(s.volume);
    range.setAttribute("aria-label", "Ring volume");
    range.addEventListener("change", async () => { s.volume = Number(range.value); await save(); });
    vol.append(vl, el("span", "set-control"));
    vol.lastChild.appendChild(range);
    box.appendChild(vol);

    if (devices.length) {
      const per = el("details", "set-more");
      per.appendChild(el("summary", "", "A tone for one device"));
      for (const d of devices) {
        const row = el("div", "set-row");
        const label = el("label", "set-label");
        label.appendChild(el("span", "", d.name));
        label.appendChild(el("span", "muted small", KINDS.find((k) => k[0] === d.kind)?.[1] || ""));
        const key = d.name.trim().toLowerCase();
        const sel = toneSelect(s.devices[key] || "", true);
        sel.setAttribute("aria-label", `${d.name} tone`);
        sel.addEventListener("change", async () => {
          if (sel.value) s.devices[key] = sel.value; else delete s.devices[key];
          await save();
        });
        row.append(label, el("span", "set-control"));
        row.lastChild.append(sel, playButton(() => sel.value || s.events[d.kind] || "soft-chime"));
        per.appendChild(row);
      }
      box.appendChild(per);
    }

    // Own tones.
    const own = el("div", "set-own");
    const mine = listing.tones.filter((t) => t.own);
    for (const t of mine) {
      const row = el("div", "set-row");
      row.appendChild(el("span", "set-label", t.name));
      const ctl = el("span", "set-control");
      ctl.appendChild(playButton(() => t.id));
      const rm = el("button", "", "Remove");
      rm.type = "button";
      rm.addEventListener("click", async () => { listing = await invoke("ringtone_remove", { id: t.id }); KagazRing.forget(); draw(); });
      ctl.appendChild(rm);
      row.appendChild(ctl);
      own.appendChild(row);
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
    own.append(add, pick, note);
    box.appendChild(own);
    body.appendChild(box);
  }

  async function drawRinging(body) {
    const box = section("Ringing on this device");
    let calls = null;
    try { calls = await invoke("calls_status"); } catch (_) {}
    const row = el("div", "set-row");
    const label = el("label", "set-label");
    label.appendChild(el("span", "", "Ring me here"));
    label.appendChild(el("span", "muted small", "Ring on this device when someone calls your OneTouch account. On a phone Kagaz keeps listening after you close it, with a small “Ready for doorbell calls” notification."));
    const sw = document.createElement("input");
    sw.type = "checkbox";
    sw.className = "toggle";
    sw.checked = !!(calls && calls.enabled);
    sw.setAttribute("aria-label", "Ring me here");
    sw.addEventListener("change", () => invoke("calls_enable", { enabled: sw.checked }));
    row.append(label, el("span", "set-control"));
    row.lastChild.appendChild(sw);
    box.appendChild(row);
    if (/Android/i.test(navigator.userAgent)) {
      box.appendChild(el("p", "muted small", "Some phones close apps to save battery. If Kagaz stops ringing after a while, allow it to run in the background in Android's battery settings for Kagaz."));
    }
    body.appendChild(box);
  }

  async function drawAccounts(body) {
    const box = section("Accounts", "Kagaz signs in to each service the way its own app does. Passwords stay in this device's password store.");
    const row = (name, state, action, go) => {
      const r = el("div", "set-row");
      const label = el("label", "set-label");
      label.appendChild(el("span", "", name));
      label.appendChild(el("span", "muted small", state));
      const b = el("button", "", action);
      b.type = "button";
      b.addEventListener("click", go);
      r.append(label, el("span", "set-control"));
      r.lastChild.appendChild(b);
      box.appendChild(r);
    };
    try {
      const ot = await invoke("onetouch_peek");
      row("OneTouch (doorbell)", ot.logged_in ? `Signed in as ${ot.username}` : ot.remembered ? `Saved sign-in: ${ot.remembered.username}` : "Not signed in", "Open", () => selectCategory("doorbell"));
    } catch (_) {}
    try {
      const cam = await invoke("cameras_status");
      row("Tapo (security cameras)", cam.logged_in ? "Signed in" : "Not signed in", "Open", () => selectCategory("cameras"));
    } catch (_) {}
    body.appendChild(box);
  }

  function drawAbout(body) {
    const box = section("About Kagaz");
    box.appendChild(el("p", "muted small", "Free and open-source (GPL-3.0). Brand names and logos belong to their owners and are shown only to say which devices Kagaz works with."));
    body.appendChild(box);
  }

  async function draw() {
    const body = $("settings-body");
    body.innerHTML = "";
    drawTones(body);
    await drawRinging(body);
    await drawAccounts(body);
    drawAbout(body);
  }

  async function show() {
    $("settings-body").innerHTML = '<p class="muted">Loading…</p>';
    try {
      listing = await invoke("ringtones_list");
      devices = await ringers();
    } catch (e) {
      $("settings-body").textContent = String(e);
      return;
    }
    await draw();
  }

  return { show };
})();
