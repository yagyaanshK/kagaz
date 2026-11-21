// Found on your network (nearby.js): Home's card of devices on this
// network that Kagaz can use and are not added yet, each with Add, and a
// page with everything else that answered (phones, the router, computers),
// so the person sees what Kagaz can do here without knowing where to look.
//
// One look when the app opens (Home's first showing) and again on the
// bar's refresh or Look again; nothing runs in the background. Each tool's
// own search runs at once (printers, TVs, purifiers, fans, computers that
// show their screen, door stations), beside a sweep of every address
// (nearby_sweep). Bluetooth is used only when it is already allowed and on:
// the look never asks anything by itself; a row in the card asks once.
// On an iPhone and a Mac the system asks before an app may look on the
// local network, so there the first look waits for a tap.

const KagazNearby = (() => {
  const OS = document.documentElement.dataset.os || "gnome";
  const PHONE = OS === "android" || OS === "ios";
  const ASKS_FIRST = OS === "ios" || OS === "macos";
  const HIDDEN_KEY = "kz-nearby-hidden";
  const BT_KEY = "kz-nearby-bt-asked";
  const OK_KEY = "kz-nearby-ok";

  const read = (k, d) => { try { const v = JSON.parse(localStorage.getItem(k)); return v ?? d; } catch (_) { return d; } };
  const write = (k, v) => { try { localStorage.setItem(k, JSON.stringify(v)); } catch (_) {} };

  let usable = [];   // devices Kagaz can add: { key, kind, icon, name, sub, ip, add }
  let hosts = [];    // every address that answered (nearby_sweep)
  let known = new Map(); // address → what it is in Kagaz already ("Your printer")
  let searching = false;
  let searched = false;
  let lastRun = 0;
  let trouble = "";
  let btReady = true;
  let hidden = new Set(read(HIDDEN_KEY, []));

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function icon(name) {
    const s = el("span", "tile-icon");
    s.innerHTML = KagazIcons.svg(name);
    return s;
  }
  function textButton(cls, text, onClick, label) {
    const b = el("button", cls, text);
    b.type = "button";
    if (label) b.setAttribute("aria-label", label);
    b.addEventListener("click", (ev) => { ev.stopPropagation(); onClick(b); });
    return b;
  }
  const ipNum = (ip) => (ip || "").split(".").reduce((n, p) => n * 256 + (+p || 0), 0);

  // ---------- what each kind is called ----------

  const KIND = {
    this: [PHONE ? "This phone" : "This computer", PHONE ? "mobile" : "computer"],
    router: ["Router", "device"],
    phone: ["Phone", "mobile"],
    tablet: ["Tablet", "mobile"],
    computer: ["Computer", "computer"],
    tv: ["TV or streaming box", "tv"],
    speaker: ["Speaker", "speaker"],
    printer: ["Printer", "print"],
    camera: ["Camera", "cameras"],
    intercom: ["Door station", "doorbell"],
    console: ["Game console", "device"],
    storage: ["Network storage", "device"],
    "smart-home": ["Smart home device", "device"],
    unknown: ["Device", "device"],
  };
  const kindWord = (h) => (KIND[h.kind] || KIND.unknown)[0];
  const kindIcon = (h) => (KIND[h.kind] || KIND.unknown)[1];
  // Its own name, else what it is by its maker ("Samsung phone").
  function hostName(h) {
    if (h.this_device) return kindWord(h);
    if (h.name) return h.name;
    if (h.router) return h.maker ? `${h.maker} router` : "Router";
    const word = kindWord(h);
    if (h.maker) return `${h.maker} ${h.kind === "unknown" ? "device" : word.toLowerCase()}`;
    return word;
  }
  function hostLine(h) {
    const parts = [];
    if (!h.this_device && (h.name || h.maker)) parts.push(kindWord(h));
    if (h.name && h.maker) parts.push(h.maker);
    if (h.model && !(h.name || "").includes(h.model)) parts.push(h.model);
    if (h.private_mac && !h.this_device) parts.push("private address");
    parts.push(h.ip);
    const k = known.get(h.ip);
    if (k) parts.unshift(k);
    return parts.join(" · ");
  }

  // ---------- looking ----------

  // Each tool's own search, all at once: what it found and has not added.
  async function lookForUsable() {
    const out = [];
    const ok = (p) => p.catch(() => null);
    const [printers, tvs, tvSaved, purifiers, fans, remote, door, cams] = await Promise.all([
      ok(invoke("discover", { timeout: 3 })),
      ok(invoke("tvs_search")),
      ok(invoke("tvs_saved")),
      ok(invoke("purifiers_scan", { seconds: 4 })),
      ok(invoke("fans_scan", { seconds: 5, fresh: false, quiet: true })),
      ok(invoke("remote_nearby")),
      ok(invoke("doorbell_stations").then((s) => invoke("doorbell_discover", { port: s.listen_port }).then((f) => ({ saved: s.stations || [], found: f })))),
      ok(invoke("cameras_status")),
    ]);

    known = new Map();
    const knownAs = (addr, words) => { if (addr && !known.has(addr)) known.set(addr, words); };

    // Printers and scanners, as their page lists them.
    if (typeof savedPrinters !== "undefined") for (const p of savedPrinters) for (const a of p.addresses || []) knownAs(a, "Your printer");
    const seenPrinter = new Set();
    for (const d of printers || []) {
      if (typeof isSaved === "function" && isSaved(d)) continue;
      const key = "printer:" + printerKey(d);
      if (seenPrinter.has(key)) continue;
      seenPrinter.add(key);
      const ip = (d.addresses || [])[0] || "";
      const scans = typeof has === "function" && has(d, "Escl", "Escls", "WsdScan", "SaneNet");
      const prints = typeof has === "function" && has(d, "Ipp", "Ipps", "WsdPrint", "IppUsb", "PdlDataStream", "Lpd", "UsbPrinter");
      out.push({
        key, kind: "print", icon: "print", ip,
        name: title(d),
        sub: [prints && scans ? "Printer and scanner" : scans ? "Scanner" : "Printer", d.usb ? "USB" : ip].filter(Boolean).join(" · "),
        add: async () => {
          await addPrinter(d);
          return typeof isSaved === "function" && isSaved(d) ? "In your printers" : null;
        },
      });
    }

    // TVs (Cast, Miracast, AirPlay), not the ones added nor Kagaz itself.
    for (const t of tvSaved || []) knownAs(t.address, "Your TV");
    for (const f of tvs || []) {
      if (f.added || (tvSaved || []).some((s) => s.name === f.name) || f.model === "Kagaz") continue;
      out.push({
        key: "tv:" + f.name, kind: "tvs", icon: "tv", ip: f.address || "",
        name: f.name,
        sub: ["TV", f.model, f.address].filter(Boolean).join(" · "),
        add: async () => {
          await invoke("tv_add", { name: f.name, model: f.model, address: f.address });
          return "In your TVs";
        },
      });
    }

    // Air purifiers.
    if (purifiers) {
      for (const p of (purifiers.purifiers && purifiers.purifiers.purifiers) || []) knownAs(p.address, "Your purifier");
      for (const f of purifiers.found || []) {
        out.push({
          key: "purifier:" + f.id, kind: "purifiers", icon: "purifier", ip: f.address,
          name: f.name,
          sub: ["Air purifier", f.model, f.address].filter(Boolean).join(" · "),
          add: async () => {
            await invoke("purifier_add", { found: f, name: null });
            return "In your purifiers";
          },
        });
      }
    }

    // Fans: over Wi-Fi always, over Bluetooth when already allowed.
    if (fans) {
      for (const x of fans.found || []) {
        const product = (KagazBrands.fanProduct || (() => "Atomberg fan"))(x.series);
        const name = `${product} ${x.id.slice(-4)}`;
        out.push({
          key: "fan:" + x.id, kind: "fans", icon: "fan", ip: x.ip || "",
          name,
          sub: ["Ceiling fan", x.ble_address ? "Bluetooth" : "", x.ip || ""].filter(Boolean).join(" · "),
          add: async () => {
            selectCategory("fans");
            if (KagazFans.addFound) KagazFans.addFound(x, name);
            return null;
          },
        });
      }
    }

    // Computers and phones that show their screen (Remote Desktop, VNC).
    if (remote) {
      for (const n of remote.network || []) {
        out.push({
          key: "remote:" + n.address, kind: "remote", icon: n.way && n.way.speaks === "vnc" ? "mobile" : "computer", ip: n.address,
          name: n.name || `A ${n.words} device`,
          sub: [n.words, n.address].filter(Boolean).join(" · "),
          add: async () => {
            selectCategory("remote");
            KagazRemote.openAdd(n.address, n.name || "");
            return null;
          },
        });
      }
    }
    if (typeof KagazRemote !== "undefined" && KagazRemote.saved) {
      for (const d of KagazRemote.saved() || []) for (const p of d.paths || []) knownAs(p.host, "Your computer");
    }

    // Door stations that ring this device directly (Direct IP, SIP).
    if (door) {
      for (const s of door.saved) knownAs(s.address, "Your door station");
      for (const f of door.found || []) {
        if (door.saved.some((s) => s.address === f.address)) continue;
        out.push({
          key: "door:" + f.address, kind: "doorbell", icon: "doorbell", ip: f.address,
          name: f.agent ? f.agent.split(/[\s/]/)[0] + " door station" : "Door station",
          sub: ["Door station", "rings here directly", f.address].join(" · "),
          add: async () => {
            selectCategory("doorbell");
            openStation({ name: "", address: f.address, port: f.port, saved: false });
            $("door-station-name").focus();
            return null;
          },
        });
      }
    }

    // Cameras added already, by where they play from.
    for (const c of (cams && cams.cameras) || []) knownAs(c.local_address, "Your camera");
    return { list: out, cams };
  }

  // A camera from the sweep that Kagaz's brands cover (Tapo, Xiaomi) and
  // that is not one of the cameras already signed in.
  function cameraSuggestions(sweep, cams) {
    const out = [];
    const brands = new Set(((cams && cams.cameras) || []).map((c) => c.brand));
    for (const h of sweep) {
      if (h.kind !== "camera" || known.has(h.ip)) continue;
      const tapo = h.maker === "TP-Link" || /tapo/i.test(h.name || "");
      const xiaomi = h.maker === "Xiaomi" || /chuangmi|mijia|xiaomi/i.test(h.name || "");
      const brand = tapo ? "tapo" : xiaomi ? "xiaomi" : null;
      if (!brand || brands.has(brand)) continue;
      const word = tapo ? "TP-Link Tapo" : "Xiaomi";
      out.push({
        key: "camera:" + h.ip, kind: "cameras", icon: "cameras", ip: h.ip,
        name: h.model ? `${word} ${h.model}` : `${word} camera`,
        sub: ["Camera", `with a ${tapo ? "TP-Link" : "Xiaomi"} account`, h.ip].join(" · "),
        add: async () => {
          selectCategory("cameras");
          if (brand === "xiaomi" && typeof showXiaomi === "function") {
            invoke("xiaomi_signed_in").then((on) => { if (!on) showXiaomi(true); }).catch(() => {});
          }
          return null;
        },
      });
    }
    return out;
  }

  async function search(opts = {}) {
    if (searching) return;
    if (ASKS_FIRST && !read(OK_KEY, false) && !opts.asked) { render(); return; }
    if (opts.asked) write(OK_KEY, true);
    searching = true;
    trouble = "";
    render();
    try {
      const [found, sweep, bt] = await Promise.all([
        lookForUsable(),
        invoke("nearby_sweep", { seconds: 4 }).catch((e) => { trouble = String(e); return []; }),
        invoke("fans_bluetooth_ready").catch(() => true),
      ]);
      hosts = sweep || [];
      btReady = bt;
      usable = [...found.list, ...cameraSuggestions(hosts, found.cams)];
      usable.sort((a, b) => a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name));
    } catch (e) {
      trouble = String(e);
    }
    searching = false;
    searched = true;
    lastRun = Date.now();
    render();
  }

  // Bluetooth asked for once, when the person taps Allow: the fans' own
  // search, which may ask Android for the permission and to turn it on.
  async function allowBluetooth(b) {
    write(BT_KEY, true);
    b.disabled = true;
    try {
      const out = await invoke("fans_scan", { seconds: 6, fresh: false, quiet: false });
      btReady = await invoke("fans_bluetooth_ready").catch(() => false);
      for (const x of out.found || []) {
        const key = "fan:" + x.id;
        if (usable.some((u) => u.key === key)) continue;
        const product = (KagazBrands.fanProduct || (() => "Atomberg fan"))(x.series);
        const name = `${product} ${x.id.slice(-4)}`;
        usable.push({
          key, kind: "fans", icon: "fan", ip: x.ip || "", name,
          sub: ["Ceiling fan", x.ble_address ? "Bluetooth" : "", x.ip || ""].filter(Boolean).join(" · "),
          add: async () => { selectCategory("fans"); if (KagazFans.addFound) KagazFans.addFound(x, name); return null; },
        });
      }
    } catch (_) {}
    render();
  }

  // ---------- the rows ----------

  // One device Kagaz can add: icon, name and a line, Add, and a menu with
  // Not mine.
  function usableRow(u, inPage) {
    const li = el("li", "rc-row nearby-row");
    const text = el("span", "tile-text");
    text.append(el("span", "tile-name", u.name), el("span", "tile-sub", u.sub));
    const add = textButton("nearby-add", "Add", async (b) => {
      b.disabled = true;
      try {
        // Added here (a word back), or its own form opened (null): the
        // row stays until the device is added there.
        const done = await u.add();
        if (done) {
          usable = usable.filter((x) => x.key !== u.key);
          snack(`${u.name}: ${done.toLowerCase()}.`);
          render();
          if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
        } else {
          b.disabled = false;
        }
      } catch (e) {
        snack("Could not add it: " + e);
        b.disabled = false;
      }
    }, `Add ${u.name}`);
    const isHidden = hidden.has(u.key);
    const more = textButton("icon-btn nearby-more", null, (b) => menu(b, [
      isHidden
        ? { text: "Show on Home", go: () => unhide(u) }
        : { text: "Not mine", note: "Hide it from Home", go: () => hide(u) },
    ]), `More for ${u.name}`);
    more.innerHTML = `<span class="ic" data-icon="more">${KagazIcons.svg("more")}</span>`;
    more.setAttribute("aria-haspopup", "menu");
    li.append(icon(u.icon), text, add, more);
    if (inPage && isHidden) li.classList.add("is-hidden");
    return li;
  }

  function hostRow(h) {
    const li = el("li", "rc-row nearby-row nearby-host");
    const text = el("span", "tile-text");
    text.append(el("span", "tile-name", hostName(h)), el("span", "tile-sub", hostLine(h)));
    li.append(icon(kindIcon(h)), text);
    if (!PHONE && h.mac && !h.private_mac) li.title = h.mac;
    return li;
  }

  function hide(u) {
    hidden.add(u.key);
    write(HIDDEN_KEY, [...hidden]);
    render();
    snack(`${u.name} is hidden from Home.`, () => unhide(u));
  }
  function unhide(u) {
    hidden.delete(u.key);
    write(HIDDEN_KEY, [...hidden]);
    render();
  }

  // ---------- a small menu, and the snackbar ----------

  let openMenu = null;
  function closeMenu() {
    if (!openMenu) return;
    openMenu.remove();
    openMenu = null;
    KagazNav.pop("nearby-menu");
    document.removeEventListener("pointerdown", outside, true);
  }
  function outside(ev) {
    if (openMenu && !openMenu.contains(ev.target)) closeMenu();
  }
  function menu(anchor, items) {
    closeMenu();
    const m = el("div", "kz-menu");
    m.setAttribute("role", "menu");
    for (const it of items) {
      const b = el("button", "kz-menu-item");
      b.type = "button";
      b.setAttribute("role", "menuitem");
      b.appendChild(el("span", "kz-menu-text", it.text));
      if (it.note) b.appendChild(el("span", "kz-menu-note", it.note));
      b.addEventListener("click", () => { closeMenu(); it.go(); });
      m.appendChild(b);
    }
    m.addEventListener("keydown", (ev) => { if (ev.key === "Escape") { closeMenu(); anchor.focus(); } });
    document.body.appendChild(m);
    const r = anchor.getBoundingClientRect();
    const w = m.offsetWidth, h = m.offsetHeight;
    const left = Math.max(8, Math.min(r.right - w, window.innerWidth - w - 8));
    const top = r.bottom + 4 + h > window.innerHeight - 8 ? r.top - h - 4 : r.bottom + 4;
    m.style.left = left + "px";
    m.style.top = Math.max(8, top) + "px";
    openMenu = m;
    KagazNav.push("nearby-menu", closeMenu);
    setTimeout(() => document.addEventListener("pointerdown", outside, true), 0);
    m.querySelector("button").focus();
  }

  let snackTimer = null;
  function snack(text, undo) {
    const s = $("nearby-snack");
    s.replaceChildren(el("span", "", text));
    if (undo) s.appendChild(textButton("link", "Undo", () => { undo(); s.classList.add("hidden"); }));
    s.classList.remove("hidden");
    clearTimeout(snackTimer);
    snackTimer = setTimeout(() => s.classList.add("hidden"), undo ? 6000 : 4000);
  }

  // ---------- drawing ----------

  // Others: everything that answered that is not one of the usable ones;
  // this device and the router first, then by kind and address.
  function others() {
    const taken = new Set(usable.map((u) => u.ip).filter(Boolean));
    const rank = (h) => (h.this_device ? 0 : h.router ? 1 : 2);
    return hosts
      .filter((h) => !taken.has(h.ip))
      .sort((a, b) => rank(a) - rank(b) || kindWord(a).localeCompare(kindWord(b)) || ipNum(a.ip) - ipNum(b.ip));
  }

  function render() {
    renderCard();
    if (!$("nearby").classList.contains("hidden")) renderPage();
  }

  function renderCard() {
    const card = $("home-nearby");
    const list = $("home-nearby-list");
    const shown = usable.filter((u) => !hidden.has(u.key));
    const rest = others().length;
    const waiting = ASKS_FIRST && !read(OK_KEY, false) && !searched && !searching;
    $("home-nearby-progress").classList.toggle("hidden", !searching);
    card.setAttribute("aria-busy", searching ? "true" : "false");
    const rows = [];
    if (waiting) {
      $("home-nearby-title").textContent = "See what's on your network";
      $("home-nearby-sub").textContent = `Kagaz can look on this Wi-Fi for printers, TVs, fans, door stations and more. ${OS === "ios" ? "Your iPhone" : "Your Mac"} asks you first.`;
      const li = el("li", "nearby-cta");
      li.appendChild(textButton("accent nearby-look", "Look now", () => search({ asked: true })));
      rows.push(li);
    } else {
      $("home-nearby-title").textContent = "Found on your network";
      $("home-nearby-sub").textContent = searching && !searched
        ? "Looking for devices Kagaz can use…"
        : shown.length
          ? "Kagaz can work with these. Add one to use it here."
          : trouble ? "Could not look on this network: " + trouble : "Nothing new for Kagaz here.";
      rows.push(...shown.map((u) => usableRow(u, false)));
      if (OS === "android" && !btReady && !read(BT_KEY, false) && searched) {
        const li = el("li", "rc-row nearby-row nearby-ask");
        const text = el("span", "tile-text");
        text.append(el("span", "tile-name", "Find fans over Bluetooth too"), el("span", "tile-sub", "Android asks to allow Nearby devices for Kagaz."));
        const later = textButton("link nearby-later", "Not now", () => { write(BT_KEY, true); render(); });
        const allow = textButton("nearby-add", "Allow", (b) => allowBluetooth(b), "Allow Bluetooth to find fans");
        li.append(icon("fan"), text, later, allow);
        rows.push(li);
      }
      if (searched && (rest || usable.length > shown.length)) {
        const li = el("li", "rc-row nearby-row nearby-all");
        li.tabIndex = 0;
        li.setAttribute("role", "button");
        const text = el("span", "tile-text");
        const n = rest + (usable.length - shown.length);
        text.append(el("span", "tile-name", "Everything on your network"), el("span", "tile-sub", `${n} other device${n === 1 ? "" : "s"}${usable.length > shown.length ? `, ${usable.length - shown.length} hidden` : ""}`));
        const chev = el("span", "nearby-chev");
        chev.innerHTML = KagazIcons.svg("forward");
        li.append(icon("device"), text, chev);
        li.addEventListener("click", openPage);
        rows.push(li);
      }
    }
    list.replaceChildren(...rows);
    // Nothing yet and nothing coming: no card at all.
    card.classList.toggle("hidden", !waiting && !searching && !searched);
  }

  function renderPage() {
    const shown = usable.filter((u) => !hidden.has(u.key));
    const hid = usable.filter((u) => hidden.has(u.key));
    const rest = others();
    $("nearby-usable").replaceChildren(...shown.map((u) => usableRow(u, true)));
    $("nearby-usable-head").classList.toggle("hidden", !shown.length);
    $("nearby-hidden").replaceChildren(...hid.map((u) => usableRow(u, true)));
    $("nearby-hidden-head").classList.toggle("hidden", !hid.length);
    $("nearby-others").replaceChildren(...rest.map(hostRow));
    $("nearby-others-head").classList.toggle("hidden", !rest.length);
    $("nearby-progress").classList.toggle("hidden", !searching);
    const ago = lastRun ? Math.round((Date.now() - lastRun) / 60000) : null;
    $("nearby-note").textContent = searching
      ? "Looking on this network…"
      : !searched ? "" : `${hosts.length} device${hosts.length === 1 ? "" : "s"} answered on this network${ago ? `, ${ago} min ago` : ""}.`
        + (OS === "android" ? " Android keeps other devices' hardware addresses from apps, so a maker shows only when a device names itself." : "");
  }

  function openPage() {
    selectCategory("nearby");
    renderPage();
  }

  // Home on screen: the first time in this run of the app, a look.
  function homeShown() {
    renderCard();
    if (!searched && !searching) search();
  }

  $("home").addEventListener("refresh", () => search({ asked: ASKS_FIRST }));
  $("nearby").addEventListener("refresh", () => search({ asked: ASKS_FIRST }));
  $("home-nearby-list").addEventListener("keydown", (ev) => {
    const li = ev.target.closest && ev.target.closest(".nearby-all");
    if (li && (ev.key === "Enter" || ev.key === " ")) { ev.preventDefault(); openPage(); }
  });
  window.addEventListener("resize", closeMenu);
  $("main").addEventListener("scroll", closeMenu, { passive: true });

  return { homeShown, search, render };
})();
