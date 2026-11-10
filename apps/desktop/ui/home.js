// Home: first what needs a look (a row of chips, each a count and a noun
// that opens its kind), then the person's favourites, then every device
// set up here by kind, and "Add a device" (a kind, then a brand, then that
// brand's sign-in or search).
//
// Home reads only what Kagaz already knows: it never signs in to a cloud
// account (OneTouch keeps one session per account, so a sign-in here would
// sign the person's other phone out), and it switches nothing: a tile opens
// its device. (Fans' commands are not checked against a fan yet and
// purifiers cannot be switched yet, so a switch on Home would promise what
// the page cannot keep.)
// Uses app.js's globals: $, invoke, selectCategory, devices, rescan,
// printBrand, title, printerBrand, savedPrinters, stateLine, printerKey,
// checkPrinters, select; onetouch.js's otOpenByName, otCallByName.

const KagazHome = (() => {
  let lastScan = 0;
  let addKind = null; // the kind whose brands are shown in Add a device
  let choosing = false; // picking favourites: every tile shows its star

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }
  function button(cls, html, label) {
    const b = el("button", cls);
    b.type = "button";
    if (html) b.innerHTML = html;
    if (label) b.setAttribute("aria-label", label);
    return b;
  }

  // ---------- favourites ----------
  // Kept on this device, as "kind:name" keys, in the order they were starred.
  const FAV_KEY = "kagaz.home.favourites";
  function favs() {
    try {
      const v = JSON.parse(localStorage.getItem(FAV_KEY) || "[]");
      return Array.isArray(v) ? v : [];
    } catch (e) {
      return [];
    }
  }
  function setFavs(list) {
    try {
      localStorage.setItem(FAV_KEY, JSON.stringify(list));
    } catch (e) {}
  }
  const favKey = (kind, name) => `${kind}:${name.trim().toLowerCase()}`;

  // ---------- tiles ----------
  // A tile: the device's icon, name and how it is; the whole tile opens it.
  // A doorbell's tile adds Live and Call, the two things it is opened for.
  // While choosing favourites, a star sits at its end.
  function tile(d) {
    const t = el("div", "home-tile" + (d.tone ? " " + d.tone : ""));
    const main = button("tile-main");
    main.append(Object.assign(el("span", "tile-icon"), { innerHTML: KagazIcons.svg(d.icon) }));
    const text = el("span", "tile-text");
    text.append(el("span", "tile-name", d.name), el("span", "tile-sub", d.sub || ""));
    main.append(text);
    main.addEventListener("click", d.open);
    t.append(main);
    if (d.actions && d.actions.length) {
      const row = el("span", "tile-actions");
      for (const a of d.actions) {
        const b = button("tile-act", KagazIcons.label(a.icon, a.label));
        b.title = a.title || a.label;
        b.addEventListener("click", a.run);
        row.append(b);
      }
      t.append(row);
      t.classList.add("has-actions");
    }
    if (d.key) {
      const on = favs().includes(d.key);
      const star = button("tile-star" + (on ? " on" : ""), KagazIcons.svg("star"), on ? `Remove ${d.name} from favourites` : `Add ${d.name} to favourites`);
      star.setAttribute("aria-pressed", String(on));
      star.addEventListener("click", () => {
        const list = favs().filter((k) => k !== d.key);
        if (!on) list.push(d.key);
        setFavs(list);
        show();
      });
      t.append(star);
    }
    return t;
  }

  // ---------- kinds ----------
  // Kinds start collapsed; one opens when the person taps it (kept while
  // the app runs).
  const expanded = new Set();

  function group(kind) {
    const box = el("section", "home-group");
    const head = el("div", "home-group-head");
    const toggle = button("home-group-toggle");
    const icon = el("span", "kind-icon");
    icon.innerHTML = KagazIcons.svg(kind.icon);
    const words = el("span", "home-group-words");
    words.append(el("span", "home-group-title", kind.title), el("span", "home-status", kind.status || ""));
    const chevron = el("span", "home-chevron");
    chevron.innerHTML = KagazIcons.svg("down");
    toggle.append(icon, words, chevron);
    const go = button("link home-open", "", `Open ${kind.title}`);
    go.textContent = "Open";
    go.addEventListener("click", kind.open);
    head.append(toggle, go);
    const tiles = el("div", "home-tiles");
    tiles.id = `home-tiles-${kind.id}`;
    toggle.setAttribute("aria-controls", tiles.id);
    for (const d of kind.devices) tiles.append(tile(d));
    const open = choosing || expanded.has(kind.id);
    box.classList.toggle("open", open);
    tiles.hidden = !open;
    toggle.setAttribute("aria-expanded", String(open));
    toggle.addEventListener("click", () => {
      if (expanded.has(kind.id)) expanded.delete(kind.id); else expanded.add(kind.id);
      const on = choosing || expanded.has(kind.id);
      box.classList.toggle("open", on);
      tiles.hidden = !on;
      toggle.setAttribute("aria-expanded", String(on));
    });
    box.append(head, tiles);
    return box;
  }

  // ---------- what Kagaz knows, kind by kind ----------
  const DOOR_ICON = { door: "doorbell", indoor: "indoor", guard: "guard" };
  const DOOR_WORDS = { door: "Door station", indoor: "Indoor monitor", guard: "Guard phone" };

  async function doorbellKind() {
    const [calls, ot, stations] = await Promise.all([
      invoke("calls_status").catch(() => null),
      invoke("onetouch_peek").catch(() => null),
      invoke("doorbell_stations").catch(() => null),
    ]);
    const kindOf = { "0": "door", "1": "door", "2": "indoor", "3": "guard" };
    const items = new Map();
    for (const d of (ot && ot.devices) || []) {
      items.set(d.name.trim().toLowerCase(), { name: d.name, kind: kindOf[d.kind] || "door", live: d.can_monitor, cloud: true });
    }
    const lineUp = !!(calls && calls.enabled && calls.line === "up");
    for (const d of (calls && calls.devices) || []) {
      if (d.kind === "apps") continue;
      const key = d.name.trim().toLowerCase();
      const had = items.get(key);
      items.set(key, { name: d.name, kind: d.kind || (had && had.kind) || "door", live: had ? had.live : false, cloud: true, offline: !d.online, callable: true });
    }
    for (const s of (stations && stations.stations) || []) {
      const key = s.name.trim().toLowerCase();
      if (!items.has(key)) items.set(key, { name: s.name, kind: "door", live: false, cloud: false });
    }
    const signedUp = (ot && (ot.logged_in || ot.remembered)) || items.size;
    if (!signedUp) return null;
    const status = lineUp ? "Ringing here" : calls && calls.enabled ? "Not connected yet" : "Not ringing here";
    const open = () => selectCategory("doorbell");
    const devs = [];
    for (const d of items.values()) {
      const sub = [DOOR_WORDS[d.kind] || "Device", d.offline ? "offline" : "", d.cloud ? "" : "this network"].filter(Boolean).join(" · ");
      const actions = [];
      if (d.cloud && d.live && !d.offline) actions.push({ icon: "eye", label: "Live", title: `Watch ${d.name} live`, run: () => { open(); if (typeof otOpenByName === "function") otOpenByName(d.name); } });
      if (d.cloud && d.callable && lineUp && !d.offline) actions.push({ icon: "phone", label: "Call", title: `Call ${d.name}`, run: () => { open(); if (typeof otCallByName === "function") otCallByName(d.name); } });
      devs.push({
        key: favKey("doorbell", d.name), name: d.name, sub, icon: DOOR_ICON[d.kind] || "doorbell", actions,
        open: () => { open(); if (d.cloud && typeof otOpenByName === "function") otOpenByName(d.name); },
      });
    }
    if (!items.size) devs.push({ name: "OneTouch", sub: ot && ot.logged_in ? "Signed in" : "Open to sign in again", icon: "doorbell", open });
    const chip = calls && calls.enabled && !lineUp ? { icon: "doorbell", text: "Doorbell not connected", tone: "warn", open } : null;
    return { id: "doorbell", icon: "doorbell", title: "Doorbell", status, devices: devs, open, chips: chip ? [chip] : [], count: items.size };
  }

  async function camerasKind() {
    const cam = await invoke("cameras_status").catch(() => null);
    if (!cam || !cam.logged_in) return null;
    const brandOf = (c) => KagazBrands.byId("security", c.brand || "tapo") || KagazBrands.byId("security", "tapo");
    const names = [...new Set((cam.cameras || []).map((c) => brandOf(c).name))];
    const open = () => selectCategory("cameras");
    const devs = (cam.cameras || []).map((c) => ({ key: favKey("cameras", c.name), name: c.name, sub: brandOf(c).name, icon: "cameras", open }));
    if (!devs.length) devs.push({ name: "Your cameras", sub: "Open to watch", icon: "cameras", open });
    return { id: "cameras", icon: "cameras", title: "Security cameras", status: names.join(", ") || "Tapo", devices: devs, open, chips: [], count: (cam.cameras || []).length };
  }

  // The printers the person added, each with how it is now.
  function printersKind() {
    if (!savedPrinters || !savedPrinters.length) return null; // not read yet, or none
    const open = () => { printBrand = null; selectCategory("print"); };
    let trouble = 0;
    const devs = savedPrinters.map((d) => {
      const key = printerKey(d);
      const [cls, words] = stateLine(d);
      if (cls === "cap-warn" || cls === "cap-bad") trouble++;
      const b = printerBrand(d);
      return {
        key: favKey("print", title(d)), name: title(d), sub: [b && b.name, words].filter(Boolean).join(" · "), icon: "print",
        tone: cls === "cap-warn" ? "warn" : cls === "cap-bad" ? "bad" : "",
        open: () => {
          open();
          const i = devices.findIndex((x) => printerKey(x) === key);
          if (i >= 0) select(i);
        },
      };
    });
    const chips = trouble ? [{ icon: "print", text: trouble === 1 && savedPrinters.length === 1 ? "Printer needs a look" : `${trouble} printers need a look`, tone: "warn", open }] : [];
    return { id: "print", icon: "print", title: "Printers & scanners", status: "", devices: devs, open, chips, count: devs.length };
  }

  // The fans the person added, by room, as last seen (the Fans page
  // looks for them; Home does not, so it opens at once).
  async function fansKind() {
    const data = await invoke("fans_saved").catch(() => null);
    if (!data || !data.fans.length) return null;
    const open = () => selectCategory("fans");
    const fans = data.fans.slice().sort((a, b) => (a.room || "￿").localeCompare(b.room || "￿") || a.name.localeCompare(b.name));
    const on = fans.filter((f) => f.last_state && f.last_state.power).length;
    const devs = fans.map((f) => ({ key: favKey("fans", f.name), name: f.name, sub: KagazFans.homeWords(f), icon: "fan", tone: f.last_state && f.last_state.power ? "on" : "", open }));
    const chips = on ? [{ icon: "fan", text: `${on} fan${on === 1 ? "" : "s"} on`, open }] : [];
    return { id: "fans", icon: "fan", title: "Ceiling fans", status: `Atomberg · ${on} of ${fans.length} on`, devices: devs, open, chips, count: devs.length };
  }

  // The purifiers the person added, as last seen (the page looks for
  // them; Home does not).
  async function purifiersKind() {
    const data = await invoke("purifiers_saved").catch(() => null);
    if (!data || !data.purifiers.length) return null;
    const open = () => selectCategory("purifiers");
    const devs = data.purifiers.map((p) => ({ key: favKey("purifiers", p.name), name: p.name, sub: KagazPurifiers.homeWords(p), icon: "purifier", open }));
    return { id: "purifiers", icon: "purifier", title: "Air purifiers", status: "Philips", devices: devs, open, chips: [], count: devs.length };
  }

  // The society's gate: who is waiting, when signed in (or looking at the sample).
  function societyKind() {
    if (typeof KagazSociety === "undefined") return null;
    const st = KagazSociety.homeState();
    if (!st) return null;
    const open = () => selectCategory("society");
    const answer = () => { open(); KagazSociety.open("visitors"); };
    const devs = st.pending.map((v) => ({
      name: v.name, sub: `${v.company ? v.company + " " : ""}${v.kind} at ${v.gate} · tap to answer`, icon: "guest", tone: "warn", open: answer,
    }));
    if (!st.pending.length) devs.push({ name: "Nobody at the gate", sub: "Open to allow someone in ahead of time", icon: "society", open });
    const n = st.pending.length;
    const chips = n ? [{ icon: "guest", text: `${n} at the gate`, tone: "warn", open: answer }] : [];
    return { id: "society", icon: "society", title: "Society gate", status: st.status, devices: devs, open, chips, count: 0 };
  }

  // ---------- drawing Home ----------
  let drawing = 0;
  async function show() {
    const mine = ++drawing;
    const kinds = (await Promise.all([doorbellKind(), camerasKind(), printersKind(), fansKind(), purifiersKind(), societyKind()])).filter(Boolean);
    if (mine !== drawing) return; // a newer drawing is on its way

    // Chips: what needs a look, most pressing first.
    const chips = kinds.flatMap((k) => k.chips).sort((a, b) => (b.tone === "warn") - (a.tone === "warn"));
    const row = $("home-chips");
    row.replaceChildren(...chips.map((c) => {
      const b = button("status-chip" + (c.tone ? " " + c.tone : ""), KagazIcons.label(c.icon, c.text));
      b.setAttribute("role", "listitem");
      b.addEventListener("click", c.open);
      return b;
    }));
    row.classList.toggle("hidden", !chips.length);

    // Favourites, in the order they were starred.
    const all = new Map();
    for (const k of kinds) for (const d of k.devices) if (d.key) all.set(d.key, d);
    const fav = favs().map((k) => all.get(k)).filter(Boolean);
    $("home-favs").replaceChildren(...fav.map((d) => tile({ ...d, key: choosing ? d.key : null })));
    $("home-favs-box").classList.toggle("hidden", !fav.length);
    $("home-fav-edit").textContent = choosing ? "Done" : "Edit";
    $("home-fav-choose").textContent = choosing ? "Done" : "Choose favourites";
    $("home-fav-choose").classList.toggle("hidden", !all.size || (!!fav.length && !choosing));
    $("home-fav-hint").classList.toggle("hidden", !choosing);
    $("home").classList.toggle("choosing", choosing);

    // Every kind, its devices inside (stars only while choosing).
    $("home-groups").replaceChildren(...kinds.map((k) => group({ ...k, devices: k.devices.map((d) => ({ ...d, key: choosing ? d.key : null })) })));
    $("home-empty").classList.toggle("hidden", kinds.length > 0);
    $("home-all-title").textContent = kinds.length ? "All devices" : "Your devices";
    kindNotes(kinds);

    // The added printers' states; their addresses are refreshed by a quiet
    // search now and then (a router can hand them new ones).
    checkPrinters();
    if (savedPrinters.length && Date.now() - lastScan > 5 * 60 * 1000 && !$("rescan").disabled) {
      lastScan = Date.now();
      rescan();
    }
  }

  function choose(on) {
    choosing = on;
    show();
  }
  $("home-fav-edit").addEventListener("click", () => choose(!choosing));
  $("home-fav-choose").addEventListener("click", () => choose(!choosing));

  // The navigation's kinds carry a short note ("2 devices", "none added
  // yet"); the sidebar shows it where it has room for it.
  const KIND_EMPTY = { doorbell: "not set up yet", cameras: "not signed in yet", print: "none added yet", fans: "none added yet", purifiers: "none added yet", society: "not signed in yet" };
  function kindNotes(kinds) {
    for (const [cat, empty] of Object.entries(KIND_EMPTY)) {
      const note = document.querySelector(`#categories li[data-cat="${cat}"] .cat-note`);
      if (!note || note.closest(".ringing")) continue;
      const k = kinds.find((x) => x.id === cat);
      note.textContent = !k ? empty : k.count ? `${k.count} device${k.count === 1 ? "" : "s"}` : k.status || "";
    }
  }

  // Something changed (a printer search finished, a purifier added): redraw
  // if Home is on screen.
  function devicesChanged() {
    if (!$("home").classList.contains("hidden")) show();
  }

  // ---------- Add a device ----------

  function showKinds() {
    addKind = null;
    KagazNav.pop("add-brands");
    $("add-title").textContent = "Add a device";
    $("add-sub").textContent = "What kind of device?";
    $("add-back").textContent = "← Home";
    $("add-note").textContent = "";
    $("add-grid").className = "choice-grid kinds";
    // A kind not supported yet opens its planned brands; their page says so.
    KagazBrands.fillKinds($("add-grid"), (k) => showBrands(k));
  }

  function showBrands(kind) {
    addKind = kind;
    $("add-note").textContent = "";
    KagazNav.push("add-brands", showKinds);
    $("add-title").textContent = kind.name;
    $("add-sub").textContent = kind.supported ? "Choose the brand." : "Coming soon. These are the brands planned first.";
    $("add-back").textContent = "← All kinds";
    $("add-grid").className = "choice-grid brands";
    KagazBrands.fill($("add-grid"), kind.id, (b) => pick(kind, b));
    $("main").scrollTop = 0;
  }

  function pick(kind, b) {
    if (!b.supported) {
      $("add-note").textContent = `Coming soon: Kagaz does not work with ${b.name} ${kind.name.toLowerCase()} yet.`;
      return;
    }
    $("add-note").textContent = "";
    if (kind.id === "doorbell" && b.id === "onetouch") {
      selectCategory("doorbell");
      setTimeout(() => {
        if (typeof otStatus !== "undefined" && otStatus && !otStatus.logged_in) $("ot-signin").click();
      }, 600);
    } else if (kind.id === "doorbell") {
      selectCategory("doorbell");
      addLocalStation();
    } else if (kind.id === "security") {
      selectCategory("cameras");
      if (b.id === "xiaomi") {
        invoke("xiaomi_signed_in").then((on) => { if (!on) showXiaomi(true); }).catch(() => {});
      }
    } else if (kind.id === "print") {
      printBrand = b.id === "other" ? null : b.id;
      selectCategory("print");
      rescan();
    } else if (kind.id === "fan") {
      selectCategory("fans");
      KagazFans.rescan();
    } else if (kind.id === "purifier") {
      selectCategory("purifiers");
      KagazPurifiers.rescan();
    } else if (kind.id === "society") {
      selectCategory("society");
      KagazSociety.open(b.id === "parkplus" ? "vehicles" : "visitors");
    }
  }

  function openAdd() {
    choosing = false;
    selectCategory("add");
    showKinds();
  }

  $("home-add").addEventListener("click", openAdd);
  $("add-back").addEventListener("click", () => (addKind ? showKinds() : selectCategory("home")));

  return { show, devicesChanged, openAdd };
})();
