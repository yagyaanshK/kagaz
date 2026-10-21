// Home: the devices set up on this device, grouped by kind, and "Add a
// device" (a kind, then a brand, then that brand's sign-in or search).
// Home reads only what Kagaz already knows: it never signs in to a cloud
// account (OneTouch keeps one session per account, so a sign-in here would
// sign the person's other phone out).
// Uses app.js's globals: $, invoke, selectCategory, devices, rescan,
// printBrand, title, brandOf; onetouch.js's otOpenByName.

const KagazHome = (() => {
  let lastScan = 0;
  let addKind = null; // the kind whose brands are shown in Add a device

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  // Kinds start collapsed; one opens when the person taps it (kept while
  // the app runs).
  const expanded = new Set();

  function group(kindId, title, status, open) {
    const box = el("section", "home-group");
    const head = el("div", "home-group-head");
    const toggle = el("button", "home-group-toggle");
    toggle.type = "button";
    const icon = el("span", "kind-icon");
    icon.innerHTML = KagazBrands.ICON[kindId];
    const words = el("span", "home-group-words");
    const h = el("span", "home-group-title", title);
    const st = el("span", "muted small home-status", status || "");
    words.append(h, st);
    const chevron = el("span", "home-chevron");
    chevron.setAttribute("aria-hidden", "true");
    toggle.append(icon, words, chevron);
    const go = el("button", "link", "Open \u203A");
    go.type = "button";
    go.setAttribute("aria-label", `Open ${title}`);
    go.addEventListener("click", open);
    head.append(toggle, go);
    const tiles = el("div", "home-tiles");
    tiles.id = `home-tiles-${kindId}`;
    toggle.setAttribute("aria-controls", tiles.id);
    const show = (on) => {
      box.classList.toggle("open", on);
      tiles.hidden = !on;
      toggle.setAttribute("aria-expanded", String(on));
    };
    show(expanded.has(kindId));
    toggle.addEventListener("click", () => {
      const on = !expanded.has(kindId);
      if (on) expanded.add(kindId); else expanded.delete(kindId);
      show(on);
    });
    box.append(head, tiles);
    // The count joins the status once the tiles are in.
    const count = () => {
      const n = tiles.childElementCount;
      st.textContent = [status, `${n} device${n === 1 ? "" : "s"}`].filter(Boolean).join(" \u00B7 ");
    };
    return { box, tiles, count };
  }

  function tile(name, sub, iconHtml, onClick, logo) {
    const t = el("button", "home-tile");
    t.type = "button";
    const icon = el("span", "tile-icon");
    if (logo) {
      const img = document.createElement("img");
      img.src = logo;
      img.alt = "";
      icon.appendChild(img);
      icon.classList.add("logo");
    } else {
      icon.innerHTML = iconHtml;
    }
    const text = el("span", "tile-text");
    text.append(el("span", "tile-name", name), el("span", "tile-sub muted small", sub || ""));
    t.append(icon, text);
    t.addEventListener("click", onClick);
    return t;
  }

  const DOOR_ICON = {
    door: '<svg viewBox="0 0 24 24"><path d="M6 3h12v18H6zM10 8h4M12 13a1.5 1.5 0 1 0 0 .01M9 18h6"/></svg>',
    indoor: '<svg viewBox="0 0 24 24"><path d="M3 5h18v11H3zM8 20h8M12 16v4"/></svg>',
    guard: '<svg viewBox="0 0 24 24"><path d="M12 3l8 3v6c0 4.5-3.5 8-8 9-4.5-1-8-4.5-8-9V6z"/></svg>',
  };
  const DOOR_WORDS = { door: "Door station", indoor: "Indoor monitor", guard: "Guard phone" };

  async function doorbellGroup(root) {
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
    for (const d of (calls && calls.devices) || []) {
      if (d.kind === "apps") continue;
      const key = d.name.trim().toLowerCase();
      const had = items.get(key);
      items.set(key, { name: d.name, kind: d.kind || (had && had.kind) || "door", live: had ? had.live : false, cloud: true, offline: !d.online });
    }
    for (const s of (stations && stations.stations) || []) {
      const key = s.name.trim().toLowerCase();
      if (!items.has(key)) items.set(key, { name: s.name, kind: "door", live: false, cloud: false });
    }
    const signedUp = (ot && (ot.logged_in || ot.remembered)) || items.size;
    if (!signedUp) return false;
    const status = calls && calls.enabled && calls.line === "up" ? "Ringing here" : calls && calls.enabled ? "Not connected yet" : "Not ringing here";
    const g = group("doorbell", "Doorbell", status, () => selectCategory("doorbell"));
    for (const d of items.values()) {
      const sub = [DOOR_WORDS[d.kind] || "Device", d.offline ? "offline" : "", d.live ? "live view" : "", d.cloud ? "" : "this network"].filter(Boolean).join(" · ");
      g.tiles.appendChild(tile(d.name, sub, DOOR_ICON[d.kind] || DOOR_ICON.door, () => {
        selectCategory("doorbell");
        if (d.cloud && typeof otOpenByName === "function") otOpenByName(d.name);
      }));
    }
    if (!items.size) {
      g.tiles.appendChild(tile("OneTouch", ot && ot.logged_in ? "Signed in" : "Open to sign in again", DOOR_ICON.door, () => selectCategory("doorbell")));
    }
    g.count();
    root.appendChild(g.box);
    return true;
  }

  async function camerasGroup(root) {
    const cam = await invoke("cameras_status").catch(() => null);
    if (!cam || !cam.logged_in) return false;
    const g = group("security", "Security cameras", "Tapo", () => selectCategory("cameras"));
    const tapo = KagazBrands.byId("security", "tapo");
    if (cam.cameras && cam.cameras.length) {
      for (const c of cam.cameras) g.tiles.appendChild(tile(c.name, "Tapo", "", () => selectCategory("cameras"), tapo.logo));
    } else {
      g.tiles.appendChild(tile("Tapo cameras", "Open to watch", "", () => selectCategory("cameras"), tapo.logo));
    }
    if (cam.cameras && cam.cameras.length) g.count();
    root.appendChild(g.box);
    return true;
  }

  // The printers the person added, each with how it is now.
  function printersGroup(root) {
    if (!savedPrinters.length) return false;
    const g = group("print", "Printers & scanners", "", () => { printBrand = null; selectCategory("print"); });
    for (const d of savedPrinters) {
      const b = brandOf(d);
      const key = printerKey(d);
      g.tiles.appendChild(tile(title(d), stateLine(d)[1], KagazBrands.ICON.print, () => {
        printBrand = null;
        selectCategory("print");
        const i = devices.findIndex((x) => printerKey(x) === key);
        if (i >= 0) select(i);
      }, b && b.logo));
    }
    g.count();
    root.appendChild(g.box);
    return true;
  }

  async function show() {
    const root = $("home-groups");
    const fresh = document.createElement("div");
    const shown = [await doorbellGroup(fresh), await camerasGroup(fresh), printersGroup(fresh)];
    root.replaceChildren(...fresh.childNodes);
    $("home-empty").classList.toggle("hidden", shown.some(Boolean));
    // The added printers' states; their addresses are refreshed by a quiet
    // search now and then (a router can hand them new ones).
    checkPrinters();
    if (savedPrinters.length && Date.now() - lastScan > 5 * 60 * 1000 && !$("rescan").disabled) {
      lastScan = Date.now();
      rescan();
    }
  }

  // The printer search finished: redraw if Home is on screen.
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
      setTimeout(() => $("door-list").scrollIntoView({ block: "start", behavior: "smooth" }), 300);
    } else if (kind.id === "security") {
      selectCategory("cameras");
    } else if (kind.id === "print") {
      printBrand = b.id === "other" ? null : b.id;
      selectCategory("print");
      rescan();
    }
  }

  function openAdd() {
    selectCategory("add");
    showKinds();
  }

  $("home-add").addEventListener("click", openAdd);
  $("add-back").addEventListener("click", () => (addKind ? showKinds() : selectCategory("home")));

  return { show, devicesChanged, openAdd };
})();
