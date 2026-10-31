// Ceiling fans (Atomberg): the fans the person added, by room, each with
// how it is now and its controls (on and off, speed, light, sleep, timer);
// Rearrange to make rooms, name fans and move them; and the fans within
// reach that are not added yet, under Found nearby. A remembered fan is
// shown even when it is out of reach, as a remembered printer is.
// Uses app.js's globals: $, invoke, selectCategory; ask.js's KagazAsk;
// nav.js's KagazNav; brands.js's KagazBrands.

const KagazFans = (() => {
  let data = { rooms: [], fans: [] }; // the saved list
  let reachable = new Set(); // ids that answered in the last scan
  let found = []; // nearby, not added
  let scanning = false;
  let lastScan = 0;
  let arranging = false;
  const collapsed = new Set(); // rooms folded while the app runs
  const busy = new Set(); // ids with a command on its way
  const menus = new Set(); // ids whose menu (Rename, Room, Forget) is open
  const TIMER = [0, 1, 2, 3, 6];

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }
  function button(cls, text, onClick, label) {
    const b = el("button", cls, text);
    b.type = "button";
    if (label) b.setAttribute("aria-label", label);
    b.addEventListener("click", onClick);
    return b;
  }

  // An on/off switch with its name: it shows how the fan is and flips it.
  function toggle(text, on, label, onChange) {
    const wrap = el("label", "fan-switch", text + " ");
    const input = el("input", "toggle");
    input.type = "checkbox";
    input.setAttribute("role", "switch");
    input.checked = on;
    input.setAttribute("aria-label", label);
    input.addEventListener("change", () => onChange(input.checked));
    wrap.appendChild(input);
    return wrap;
  }

  function profile(f) {
    const s = (f.series || "").toUpperCase();
    return {
      light: !s.startsWith("K"),
      brightness: s.startsWith("I") || ["M1", "S1", "S2"].includes(s),
      color: s.startsWith("I"),
    };
  }

  // The fan's state in a few words: the same words the CLI uses.
  // The Aris's three light colours, in the remote's words.
  const COLOURS = { warm: "warm", cool: "white", daylight: "natural white" };

  function words(state, colourFan = false) {
    if (!state) return "";
    if (!state.power) return "Off";
    const w = [state.boost ? "Boost" : `Speed ${state.speed}`];
    if (state.sleep) w.push("sleep");
    if (state.led) w.push("light on");
    if (state.led && colourFan) w[w.length - 1] = `light ${COLOURS[state.light_mode] || "on"}, ${state.brightness}%`;
    if (state.timer_hours > 0) w.push(`${state.timer_hours} h left`);
    return w.join(", ");
  }

  // A span of seconds the way people say it (the same words as the CLI).
  function span(seconds) {
    const d = Math.floor(seconds / 86400), h = Math.floor(seconds / 3600) % 24;
    const m = Math.floor(seconds / 60) % 60, sec = seconds % 60;
    const parts = [];
    if (d) parts.push(`${d} day${d === 1 ? "" : "s"}`);
    if (h) parts.push(`${h} h`);
    if (m) parts.push(`${m} min`);
    if (sec || !parts.length) parts.push(`${sec} s`);
    return parts.length === 1 ? parts[0] : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
  }

  // The fan's own timer: the fan reports only whole hours left, so Kagaz
  // keeps the moment it ends (timer_until, seconds since the epoch;
  // estimated when the timer was set with a remote).
  function timerLeft(f) {
    const s = f.last_state;
    if (!f.timer_until || !s || !s.power) return null;
    return f.timer_until - Date.now() / 1000;
  }
  function clock(seconds) {
    const t = Math.max(0, Math.ceil(seconds));
    const h = Math.floor(t / 3600), m = Math.floor(t / 60) % 60, sec = t % 60;
    const two = (n) => String(n).padStart(2, "0");
    return h ? `${h}:${two(m)}:${two(sec)}` : `${m}:${two(sec)}`;
  }
  function countdownText(f) {
    const left = timerLeft(f);
    if (left === null) return "";
    if (left > 0) return `Turns off in ${f.timer_estimated ? "about " : ""}${clock(left)}`;
    return "Timer done; checking the fan…";
  }
  // Asked once per timer that ended, so a fan out of reach is not asked
  // again and again.
  const timerChecked = new Set();
  function tick() {
    if ($("fans").classList.contains("hidden")) return;
    for (const el of document.querySelectorAll(".fan-countdown")) {
      const f = data.fans.find((x) => x.id === el.dataset.id);
      if (!f) continue;
      el.textContent = countdownText(f);
      const left = timerLeft(f);
      // A little after the moment, so the fan has switched itself off.
      const key = `${f.id}@${f.timer_until}`;
      if (left !== null && left < -15 && !timerChecked.has(key) && !busy.has(f.id)) {
        timerChecked.add(key);
        readOne(f.id);
      }
    }
  }
  setInterval(tick, 1000);

  // How a fan stands, from what is known and whether it answered.
  function standing(f, here) {
    const s = f.last_state;
    if (busy.has(f.id)) return f.last_state ? "Sending…" : "Asking the fan…";
    if (scanning && !lastScan) return "Checking…";
    if (s && here) return words(s, profile(f).color);
    if (s && !here) return s.power ? "Out of reach" : "Off, out of reach";
    return here ? "Within reach" : "Out of reach";
  }

  // What Home says about a fan, with no scan: what was seen last.
  function homeWords(f) {
    const w = f.last_state ? words(f.last_state) : "Not reached yet";
    const left = timerLeft(f);
    const t = left !== null && left > 0 ? `turns off in ${f.timer_estimated ? "about " : ""}${Math.ceil(left / 60)} min` : "";
    return [f.room, w, t].filter(Boolean).join(" · ");
  }

  // ---------- commands ----------

  async function send(f, command, card) {
    if (busy.has(f.id)) return;
    busy.add(f.id);
    card.classList.add("busy");
    card.querySelectorAll(".fan-controls button, .fan-controls select").forEach((c) => (c.disabled = true));
    card.querySelector(".fan-standing").textContent = "Sending…";
    try {
      const out = await invoke("fan_send", { id: f.id, command });
      data = out.fans;
      reachable.add(f.id);
    } catch (e) {
      reachable.delete(f.id);
      $("fans-note").textContent = `${f.name}: ${e}`;
    } finally {
      busy.delete(f.id);
      render();
    }
  }

  // ---------- the fan card ----------

  const FAN_ICON = KagazBrands.ICON.fan;

  function card(f) {
    const here = reachable.has(f.id);
    const s = f.last_state;
    const on = !!(s && s.power);
    const c = el("article", "fan-card" + (here ? "" : " unreachable") + (on ? " on" : ""));
    c.dataset.id = f.id;
    c.setAttribute("aria-label", f.name);
    const head = el("div", "fan-head");
    const icon = el("span", "fan-icon");
    icon.innerHTML = FAN_ICON;
    const text = el("div", "fan-text");
    const countdown = el("span", "small fan-countdown", countdownText(f));
    countdown.dataset.id = f.id;
    countdown.setAttribute("role", "timer");
    text.append(el("strong", "fan-name", f.name), el("span", "muted small fan-standing", standing(f, here)), countdown);
    head.append(icon, text);
    if (!arranging) {
      const toggle = button("icon fan-more-btn", "⋯", () => {
        const more = c.querySelector(".fan-more");
        more.hidden = !more.hidden;
        if (more.hidden) menus.delete(f.id); else menus.add(f.id);
        toggle.setAttribute("aria-expanded", String(!more.hidden));
      }, `More for ${f.name}`);
      toggle.setAttribute("aria-expanded", String(menus.has(f.id)));
      head.appendChild(toggle);
    }
    c.appendChild(head);

    if (arranging) {
      // Name and room are set here; the controls rest.
      const row = el("div", "fan-arrange");
      row.appendChild(button("", "Rename", () => rename(f)));
      row.appendChild(roomSelect(f));
      row.appendChild(button("danger-text", "Forget", () => forget(f)));
      c.appendChild(row);
      return c;
    }

    const more = el("div", "fan-more");
    more.hidden = !menus.has(f.id);
    if (s && (s.run_seconds != null || s.off_seconds != null)) {
      const times = el("p", "fan-times muted small");
      const lines = [];
      if (s.run_seconds != null) lines.push(`Has run for ${span(s.run_seconds)} to date.`);
      if (s.off_seconds != null) lines.push(`Has been switched off for ${span(s.off_seconds)} to date.`);
      times.textContent = lines.join(" ");
      times.title = "The fan's own count, as of the last time Kagaz read it";
      more.appendChild(times);
    }
    more.appendChild(button("", "Rename", () => rename(f)));
    more.appendChild(roomSelect(f));
    more.appendChild(button("danger-text", "Forget", () => forget(f)));
    c.appendChild(more);

    const controls = el("div", "fan-controls");
    const power = button("fan-power" + (on ? " on" : ""), on ? "Turn off" : "Turn on", () => send(f, { kind: "power", value: !on }, c));
    power.setAttribute("aria-pressed", String(on));
    controls.appendChild(power);
    const speeds = el("div", "fan-speeds");
    speeds.setAttribute("role", "group");
    speeds.setAttribute("aria-label", `Speed of ${f.name}`);
    // Speeds 1 to 5 and Boost, as on the remote. The fan reports Boost as
    // speed 6 with its Boost flag (checked on an Aris).
    const boosting = on && !!s.boost;
    for (let n = 1; n <= 5; n++) {
      const here = on && !boosting && s.speed === n;
      const b = button("chip" + (here ? " active" : ""), String(n), () => send(f, { kind: "speed", value: n }, c), `Speed ${n}`);
      b.setAttribute("aria-pressed", String(here));
      speeds.appendChild(b);
    }
    // Boost on and off, as the remote's button: checked on an Aris (Boost
    // off goes back to speed 5).
    const boost = button("chip" + (boosting ? " active" : ""), "Boost", () => send(f, { kind: "boost", value: !boosting }, c), `Boost ${f.name}`);
    boost.setAttribute("aria-pressed", String(boosting));
    speeds.appendChild(boost);
    controls.appendChild(speeds);
    const extras = el("div", "fan-extras");
    const p = profile(f);
    if (p.light) extras.appendChild(toggle("Light", !!(s && s.led), `Light of ${f.name}`, (v) => send(f, { kind: "led", value: v }, c)));
    // The light's colour and brightness, on fans that have them: read and
    // set as checked on an Aris.
    if (p.light && p.color) {
      const pick = el("label", "fan-timer", "Colour ");
      const sel = el("select");
      sel.setAttribute("aria-label", `Light colour of ${f.name}`);
      for (const [mode, name] of [["warm", "Warm"], ["cool", "White"], ["daylight", "Natural white"]]) {
        const o = el("option", "", name);
        o.value = mode;
        sel.appendChild(o);
      }
      sel.value = (s && s.light_mode) || "daylight";
      sel.addEventListener("change", () => send(f, { kind: "light_mode", value: sel.value }, c));
      pick.appendChild(sel);
      extras.appendChild(pick);
    }
    if (p.light && p.brightness) {
      const dim = el("label", "fan-timer fan-dim", "Brightness ");
      const range = el("input");
      range.type = "range";
      range.min = "10";
      range.max = "100";
      range.step = "1"; // any whole value: the fan shows steps of about 2 (checked on an Aris)
      range.value = String((s && s.brightness) || 100);
      range.setAttribute("aria-label", `Brightness of ${f.name}`);
      const shown = el("span", "muted small", `${range.value}%`);
      range.addEventListener("input", () => (shown.textContent = `${range.value}%`));
      // Sent when let go, not while dragging: one connection, not ten.
      range.addEventListener("change", () => send(f, { kind: "brightness", value: Number(range.value) }, c));
      dim.append(range, shown);
      extras.appendChild(dim);
    }
    const sb = toggle("Sleep", !!(s && s.sleep), `Sleep mode of ${f.name}`, (v) => send(f, { kind: "sleep", value: v }, c));
    sb.title = "Sleep mode: the fan's own feature (on some remotes, such as the Efficio's; the Aris's remote has no button for it); what it does through the night is not yet known";
    extras.appendChild(sb);
    const timer = el("label", "fan-timer", "Turn off after ");
    const sel = el("select");
    sel.setAttribute("aria-label", `Timer of ${f.name}`);
    for (let i = 0; i < TIMER.length; i++) {
      const o = el("option", "", i === 0 ? "no timer" : `${TIMER[i]} h`);
      o.value = String(i);
      sel.appendChild(o);
    }
    // The fan tells hours left, not which choice was made: pick the nearest.
    const left = s ? s.timer_hours : 0;
    const nearest = TIMER.reduce((best, h, i) => (h >= left && (best < 0 || h < TIMER[best]) ? i : best), -1);
    sel.value = String(left ? (nearest < 0 ? TIMER.length - 1 : nearest) : 0);
    sel.addEventListener("change", () => send(f, { kind: "timer", value: Number(sel.value) }, c));
    timer.appendChild(sel);
    extras.appendChild(timer);
    controls.appendChild(extras);
    if (!here) {
      controls.querySelectorAll("button, select").forEach((x) => (x.disabled = true));
      controls.title = "Out of reach: Look again when you are near it";
    }
    c.appendChild(controls);
    return c;
  }

  function roomSelect(f) {
    const label = el("label", "fan-room-pick", "Room ");
    const sel = el("select");
    sel.setAttribute("aria-label", `Room of ${f.name}`);
    const none = el("option", "", "No room");
    none.value = "";
    sel.appendChild(none);
    for (const r of data.rooms) {
      const o = el("option", "", r);
      o.value = r;
      sel.appendChild(o);
    }
    const add = el("option", "", "New room…");
    add.value = "\u0000new";
    sel.appendChild(add);
    sel.value = f.room || "";
    sel.addEventListener("change", async () => {
      let room = sel.value;
      if (room === "\u0000new") {
        room = await KagazAsk.name("Name the room", "", "Add room");
        if (!room) { sel.value = f.room || ""; return; }
      }
      await apply(() => invoke("fan_set_room", { id: f.id, room }));
    });
    label.appendChild(sel);
    return label;
  }

  async function rename(f) {
    const name = await KagazAsk.name("Name the fan", f.name, "Rename");
    if (name && name !== f.name) await apply(() => invoke("fan_rename", { id: f.id, name }));
  }

  async function forget(f) {
    if (await KagazAsk.confirm(`Forget ${f.name}?`, "Forget fan", "It stays as it is; it is just not listed here any more. It shows under Found nearby when it is within reach.")) {
      menus.delete(f.id);
      await apply(() => invoke("fan_forget", { id: f.id }));
    }
  }

  // Ask one fan how it is, without changing anything.
  async function readOne(id) {
    if (busy.has(id)) return;
    busy.add(id);
    render();
    try {
      const out = await invoke("fan_send", { id, command: { kind: "read" } });
      data = out.fans;
      reachable.add(id);
    } catch (e) {
      reachable.delete(id);
    } finally {
      busy.delete(id);
      render();
    }
  }

  // Run a change to the list and show the result; errors go to the note.
  async function apply(change) {
    try {
      data = await change();
      $("fans-note").textContent = "";
    } catch (e) {
      $("fans-note").textContent = String(e);
    }
    render();
  }

  // ---------- rooms ----------

  function roomBox(name, fansIn) {
    const title = name || "No room";
    const box = el("section", "home-group fan-room" + (collapsed.has(title) ? "" : " open"));
    const head = el("div", "home-group-head");
    const toggle = el("button", "home-group-toggle");
    toggle.type = "button";
    const icon = el("span", "kind-icon");
    icon.innerHTML = KagazBrands.ICON.room;
    const wordsBox = el("span", "home-group-words");
    const onCount = fansIn.filter((f) => reachable.has(f.id) && f.last_state && f.last_state.power).length;
    const status = fansIn.length
      ? `${fansIn.length} fan${fansIn.length === 1 ? "" : "s"}${onCount ? `, ${onCount} on` : ""}`
      : "No fans here yet";
    wordsBox.append(el("span", "home-group-title", title), el("span", "muted small home-status", status));
    const chevron = el("span", "home-chevron");
    chevron.setAttribute("aria-hidden", "true");
    toggle.append(icon, wordsBox, chevron);
    const tiles = el("div", "fan-cards");
    tiles.id = `fan-room-${title.replace(/[^a-z0-9]+/gi, "-").toLowerCase()}`;
    toggle.setAttribute("aria-controls", tiles.id);
    toggle.setAttribute("aria-expanded", String(!collapsed.has(title)));
    toggle.addEventListener("click", () => {
      if (collapsed.has(title)) collapsed.delete(title); else collapsed.add(title);
      render();
    });
    head.appendChild(toggle);
    if (arranging && name) {
      head.appendChild(button("link", "Rename", async () => {
        const n = await KagazAsk.name("Name the room", name, "Rename");
        if (n && n !== name) await apply(() => invoke("room_rename", { old: name, new: n }));
      }, `Rename room ${name}`));
      head.appendChild(button("link", "Remove", async () => {
        if (await KagazAsk.confirm(`Remove the room ${name}?`, "Remove room", "Its fans stay, in no room.")) {
          await apply(() => invoke("room_remove", { name }));
        }
      }, `Remove room ${name}`));
    }
    box.appendChild(head);
    for (const f of fansIn) tiles.appendChild(card(f));
    tiles.hidden = collapsed.has(title);
    box.appendChild(tiles);
    return box;
  }

  function render() {
    const root = $("fan-rooms");
    const fresh = document.createDocumentFragment();
    const byName = (a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    for (const r of data.rooms) {
      const fansIn = data.fans.filter((f) => f.room === r).sort(byName);
      if (fansIn.length || arranging) fresh.appendChild(roomBox(r, fansIn));
    }
    const loose = data.fans.filter((f) => !f.room || !data.rooms.includes(f.room)).sort(byName);
    if (loose.length) fresh.appendChild(roomBox("", loose));
    if (arranging) {
      fresh.appendChild(button("fan-add-room", "+ Add room", async () => {
        const n = await KagazAsk.name("Name the room", "", "Add room");
        if (n) await apply(() => invoke("room_add", { name: n }));
      }));
    }
    root.replaceChildren(fresh);
    $("fans-empty").classList.toggle("hidden", data.fans.length > 0 || arranging);
    $("fans-arrange").textContent = arranging ? "Done" : "Rearrange";
    $("fans-arrange").classList.toggle("hidden", !data.fans.length && !arranging);
    $("fans-arrange").setAttribute("aria-pressed", String(arranging));
    renderFound();
  }

  function renderFound() {
    const ul = $("fans-found");
    ul.innerHTML = "";
    for (const x of found) {
      const li = el("li");
      const product = (KagazBrands.fanProduct || (() => "Atomberg fan"))(x.series);
      const tail = x.id.slice(-4);
      li.innerHTML = `<div class="name"></div><div class="where"></div>`;
      li.querySelector(".name").textContent = `${product} ${tail}`;
      li.querySelector(".where").textContent = [
        "Atomberg",
        x.series,
        x.ble_address ? "Bluetooth" : "",
        x.ip ? `Wi-Fi ${x.ip}` : "",
      ].filter(Boolean).join(" · ");
      li.appendChild(button("add-printer", "Add", () => add(x, `${product} ${tail}`), `Add ${product} ${tail} to your fans`));
      ul.appendChild(li);
    }
    const note = $("fans-found-note");
    if (scanning) note.textContent = "Looking for fans over Bluetooth and on the Wi-Fi network…";
    else if (!lastScan) note.textContent = "";
    else if (found.length) note.textContent = `${found.length} found. Add one to keep it in your fans.`;
    else note.textContent = data.fans.length ? "No other fans within reach." : "No fans within reach. Stand near the fan with Bluetooth on, then Look again.";
    note.classList.toggle("working", scanning);
    $("fans-found-head").classList.toggle("hidden", !found.length && !lastScan && !scanning);
  }

  // Add a found fan: its name and room, in one step.
  function add(x, suggested) {
    const dlg = document.createElement("dialog");
    dlg.className = "ask";
    const form = el("form");
    form.method = "dialog";
    const title = el("h3", "ask-title", "Add this fan");
    const nameLabel = el("label", "ask-title", "Name");
    const name = el("input");
    name.type = "text";
    name.value = suggested;
    name.required = true;
    name.autocomplete = "off";
    name.setAttribute("aria-label", "Name");
    nameLabel.appendChild(name);
    const roomLabel = el("label", "ask-title", "Room");
    const room = el("select");
    const none = el("option", "", "No room");
    none.value = "";
    room.appendChild(none);
    for (const r of data.rooms) {
      const o = el("option", "", r);
      o.value = r;
      room.appendChild(o);
    }
    const fresh = el("option", "", "New room…");
    fresh.value = "\u0000new";
    room.appendChild(fresh);
    const newRoom = el("input");
    newRoom.type = "text";
    newRoom.placeholder = "Room's name";
    newRoom.setAttribute("aria-label", "New room's name");
    newRoom.hidden = true;
    room.addEventListener("change", () => {
      newRoom.hidden = room.value !== "\u0000new";
      if (!newRoom.hidden) newRoom.focus();
    });
    roomLabel.append(room, newRoom);
    const actions = el("div", "actions");
    const keep = button("", "Not now", () => dlg.close());
    const ok = el("button", "accent", "Add fan");
    ok.type = "submit";
    actions.append(keep, ok);
    form.append(title, nameLabel, roomLabel, actions);
    let answer = null;
    form.addEventListener("submit", (ev) => {
      ev.preventDefault();
      const r = room.value === "\u0000new" ? newRoom.value.trim() : room.value;
      answer = { name: name.value.trim(), room: r };
      dlg.close();
    });
    dlg.appendChild(form);
    dlg.addEventListener("close", async () => {
      dlg.remove();
      KagazNav.pop("ask");
      if (!answer || !answer.name) return;
      found = found.filter((y) => y.id !== x.id);
      await apply(() => invoke("fan_add", { found: x, name: answer.name, room: answer.room }));
      reachable.add(x.id);
      // Ask the new fan how it is, so its card starts true (on, speed).
      await readOne(x.id);
    });
    document.body.appendChild(dlg);
    KagazNav.push("ask", () => dlg.close());
    dlg.showModal();
    name.select();
  }

  // ---------- scanning ----------

  // `fresh`: the person asked (Look again), so every fan within reach is
  // read; otherwise only fans whose last reading is old.
  async function rescan(fresh = false) {
    if (scanning) return;
    scanning = true;
    $("fans-rescan").disabled = true;
    $("fans-note").textContent = "";
    render();
    try {
      const out = await invoke("fans_scan", { seconds: 6, fresh });
      data = await invoke("fans_saved").catch(() => out.fans);
      reachable = new Set(out.reachable);
      found = out.found;
      lastScan = Date.now();
      if (out.bluetooth) $("fans-note").textContent = `${out.bluetooth}. Only fans on the Wi-Fi network can be found.`;
    } catch (e) {
      $("fans-note").textContent = String(e);
    } finally {
      scanning = false;
      $("fans-rescan").disabled = false;
      render();
    }
  }

  async function show() {
    try {
      data = await invoke("fans_saved");
    } catch (e) {
      $("fans-note").textContent = String(e);
    }
    render();
    // A timer that ended while this page was closed: ask those fans how
    // they are now, one at a time.
    for (const f of data.fans.filter((x) => (timerLeft(x) ?? 1) < 0)) {
      timerChecked.add(`${f.id}@${f.timer_until}`);
      await readOne(f.id);
    }
    // The saved fans show at once; a look around runs alongside (again
    // only after a couple of minutes).
    if (Date.now() - lastScan > 2 * 60 * 1000) rescan();
  }

  $("fans-rescan").addEventListener("click", () => rescan(true));
  $("fans-arrange").addEventListener("click", () => {
    arranging = !arranging;
    if (arranging) KagazNav.push("fans-arrange", () => { arranging = false; render(); });
    else KagazNav.pop("fans-arrange");
    render();
  });

  return { show, rescan, homeWords, words };
})();
