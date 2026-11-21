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
    const [printers, tvs, tvSaved, purifiers, fans, remote, door, cams, onetouch] = await Promise.all([
      ok(invoke("discover", { timeout: 3 })),
      ok(invoke("tvs_search")),
      ok(invoke("tvs_saved")),
      ok(invoke("purifiers_scan", { seconds: 4 })),
      ok(invoke("fans_scan", { seconds: 5, fresh: false, quiet: true })),
      ok(invoke("remote_nearby")),
      ok(invoke("doorbell_stations").then((s) => invoke("doorbell_discover", { port: s.listen_port }).then((f) => ({ saved: s.stations || [], found: f })))),
      ok(invoke("cameras_status")),
      ok(invoke("onetouch_peek")),
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
      // Not Kagaz on another device (it answers the same way), and not a
      // OneTouch station while the OneTouch account is signed in: those are
      // in Kagaz through the account already.
      const viaAccount = (f) => onetouch && onetouch.logged_in && /^(OT[-_ ]?IP|OTIPVDP|OneTouch)/i.test(f.agent || "");
      for (const f of door.found || []) {
        if (door.saved.some((s) => s.address === f.address)) continue;
        if (/^Kagaz\b/i.test(f.agent || "")) { knownAs(f.address, "Runs Kagaz"); continue; }
        if (viaAccount(f)) { knownAs(f.address, `Your door station (${onetouch.service_name || "OneTouch"})`); continue; }
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
    // A brand signed in to: its cameras are the account's, already here.
    const brands = new Set([...((cams && cams.signed_in) || []), ...((cams && cams.cameras) || []).map((c) => c.brand)]);
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

  // Computers the sweep saw (by their Windows or network name) that did not
  // take a Remote Desktop or VNC connection: listed too, saying so, and
  // their sheet says how to reach them.
  function computerSuggestions(sweep, list) {
    const taken = new Set(list.map((u) => u.ip));
    return sweep
      .filter((h) => h.kind === "computer" && !h.this_device && !taken.has(h.ip) && !known.has(h.ip))
      .map((h) => ({
        key: "computer:" + h.ip, kind: "remote", icon: "computer", ip: h.ip,
        name: hostName(h),
        sub: ["Computer", "Remote Desktop isn't on there", h.ip].join(" · "),
        add: async () => { openHost(h); return null; },
      }));
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
      usable = [...found.list, ...cameraSuggestions(hosts, found.cams), ...computerSuggestions(hosts, found.list)];
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
    if (!h.this_device) {
      li.tabIndex = 0;
      li.setAttribute("role", "button");
      li.setAttribute("aria-haspopup", "dialog");
      const chev = el("span", "nearby-chev");
      chev.innerHTML = KagazIcons.svg("forward");
      li.appendChild(chev);
      li.addEventListener("click", () => openHost(h));
      li.addEventListener("keydown", (ev) => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); openHost(h); } });
    }
    return li;
  }

  // ---------- one device's sheet: what to do with it ----------

  const PAGE_OF = { "Your printer": "print", "Your TV": "tvs", "Your purifier": "purifiers", "Your computer": "remote", "Your door station": "doorbell", "Your camera": "cameras" };
  const PAGE_NAME = { print: "Printers & scanners", tvs: "TVs & displays", purifiers: "Air purifiers", remote: "Computers & phones", doorbell: "Doorbell", cameras: "Security cameras" };
  const WEB = [[80, "http"], [8080, "http"], [443, "https"], [8443, "https"]];

  // What can be done with a device, the likeliest first: what Kagaz does
  // with its kind, then its web page, then asking it what it is.
  function actions(h, closer) {
    const open = new Set(h.open || []);
    const out = [];
    const name = hostName(h);
    const k = known.get(h.ip) || "";
    const page = Object.entries(PAGE_OF).find(([w]) => k.startsWith(w));
    if (page) out.push({ icon: "forward", name: `Open in ${PAGE_NAME[page[1]]}`, what: k, go: () => selectCategory(page[1]) });
    const remote = (speaks, words) => ({ icon: "computer", name: `Connect with ${words}`, what: "See and use its screen here", go: () => { selectCategory("remote"); KagazRemote.openAdd(h.ip, h.name || ""); } });
    if (!page) {
      if (open.has(3389)) out.push(remote("rdp", "Remote Desktop"));
      if (open.has(5900)) out.push(remote("vnc", "VNC"));
      switch (h.kind) {
        case "computer":
          if (!open.has(3389) && !open.has(5900)) {
            out.push({ icon: "info", name: "Remote Desktop isn't on there", what: "On Windows Pro: Settings › System › Remote Desktop, then turn it on. On a Mac: Screen Sharing in System Settings › General › Sharing.", info: true });
            out.push(remote("rdp", "Remote Desktop anyway"));
          }
          break;
        case "printer":
          out.push({ icon: "print", name: "Add this printer", what: "Print and scan from Kagaz", go: () => { selectCategory("print"); rescan(); } });
          break;
        case "tv": case "speaker":
          if ((h.services || []).some((x) => x === "googlecast" || x === "airplay" || x === "upnp-tv")) {
            out.push({ icon: "tv", name: "Add as a TV or display", what: "Show this device's screen on it", go: () => { selectCategory("tvs"); KagazTvs.rescan(); } });
          }
          break;
        case "camera":
          out.push({ icon: "cameras", name: "Watch it in Kagaz", what: h.maker === "Xiaomi" ? "Sign in to your Xiaomi account" : "Sign in to the camera's account (TP-Link, Xiaomi)", go: () => selectCategory("cameras") });
          break;
        case "intercom":
          out.push({ icon: "doorbell", name: "Add as a door station", what: "Rings this device directly, no account", go: () => { selectCategory("doorbell"); openStation({ name: "", address: h.ip, port: 5060, saved: false }); } });
          break;
        case "smart-home": case "console": case "storage":
          out.push({ icon: "info", name: "Kagaz doesn't work with this yet", what: `${kindWord(h)}s like this one aren't among the devices Kagaz controls.`, info: true });
          break;
        case "phone": case "tablet":
          out.push({ icon: "info", name: "Nothing to connect to", what: "Phones and tablets don't take connections from other devices on the network.", info: true });
          break;
      }
    }
    for (const [port, scheme] of WEB) {
      if (!open.has(port)) continue;
      const url = `${scheme}://${h.ip}${port === 80 || port === 443 ? "" : ":" + port}/`;
      out.push({ icon: "globe", name: h.router ? "Open the router's settings" : "Open its web page", what: (closer && closer.title) || url, go: () => invoke("open_web", { url }).catch((e) => snack(String(e))) });
      break;
    }
    if (!closer) {
      const ask = { icon: "search", name: "Ask it what it is", what: "Checks which connections it takes and reads its page", go: () => lookCloser(h) };
      // For a device Kagaz cannot place, the first thing to do.
      if (["unknown", "smart-home", "phone"].includes(h.kind) && !page) out.unshift(ask); else out.push(ask);
    }
    return out;
  }

  async function lookCloser(h) {
    openHost(h, null, true);
    let closer = null;
    try {
      closer = await invoke("nearby_look_closer", { host: h });
    } catch (e) {
      snack("Could not ask it: " + e);
      return;
    }
    const i = hosts.findIndex((x) => x.ip === h.ip);
    if (i >= 0) hosts[i] = closer.host;
    render();
    openHost(closer.host, closer);
  }

  function openHost(h, closer, asking) {
    KagazAsk.ask((dlg, finish) => {
      dlg.classList.add("sheet", "tv-sheet", "nearby-sheet");
      dlg.appendChild(el("h3", "ask-title", hostName(h)));
      const facts = [kindWord(h), h.maker, h.model, h.ip, !PHONE && h.mac && !h.private_mac ? h.mac : ""].filter((x, i, a) => x && a.indexOf(x) === i);
      dlg.appendChild(el("p", "tv-sheet-state muted small", facts.join(" · ")));
      if (closer) {
        const words = closer.services.map(([p, w]) => w).filter((w, i, a) => a.indexOf(w) === i);
        dlg.appendChild(el("p", "small", words.length ? "Takes connections for: " + words.join(", ") + "." : "It took no connection on the usual ports; it may keep to itself."));
        if (closer.server) dlg.appendChild(el("p", "muted small", "Its web server: " + closer.server));
      }
      const list = el("div", "tv-ways");
      if (asking) {
        const r = el("div", "tv-way tv-way-info");
        const words = el("span", "tv-way-words");
        words.append(el("span", "tv-way-name", "Asking it what it is…"), el("span", "tv-way-what", "A few seconds"));
        const bar = el("div", "nearby-progress");
        r.append(icon("search"), words);
        list.append(bar, r);
      } else {
        list.appendChild(el("div", "tv-way-section", "What you can do"));
        for (const a of actions(h, closer)) {
          const r = el(a.info ? "div" : "button", "tv-way" + (a.info ? " tv-way-info" : ""));
          if (!a.info) r.type = "button";
          const words = el("span", "tv-way-words");
          words.append(el("span", "tv-way-name", a.name), el("span", "tv-way-what", a.what));
          r.append(icon(a.icon), words);
          if (!a.info) {
            const chev = el("span", "nearby-chev");
            chev.innerHTML = KagazIcons.svg("forward");
            r.appendChild(chev);
            r.addEventListener("click", () => { finish(null); a.go(); });
          }
          list.appendChild(r);
        }
      }
      dlg.appendChild(list);
      if (!PHONE) {
        const actionsRow = el("div", "actions");
        actionsRow.appendChild(textButton("", "Close", () => finish(null)));
        dlg.appendChild(actionsRow);
      }
    });
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
    // This device once, with every address it has (a phone's Wi-Fi and its
    // hotspot are two networks).
    const mine = hosts.filter((h) => h.this_device);
    const me = mine.length ? [{ ...mine[0], ip: mine.map((h) => h.ip).join(", ") }] : [];
    return [...me, ...hosts.filter((h) => !h.this_device)]
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
        text.append(el("span", "tile-name", "Find devices over Bluetooth too"), el("span", "tile-sub", "Android asks to allow Nearby devices for Kagaz."));
        const later = textButton("link nearby-later", "Not now", () => { write(BT_KEY, true); render(); });
        const allow = textButton("nearby-add", "Allow", (b) => allowBluetooth(b), "Allow Bluetooth to find devices");
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
