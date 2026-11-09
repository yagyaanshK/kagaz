// Kagaz home shell mockup: the Home content built once, then the OS skin decides
// the chrome (navigation, bars, tiles, sheets). No framework; plain DOM.
(() => {
  const q = new URLSearchParams(location.search);
  const os = ["android", "ios", "windows", "gnome", "macos"].includes(q.get("os")) ? q.get("os") : "android";
  const state = q.get("state") || "normal";          // empty | urgent | normal
  const detail = q.get("detail") || "";              // "" | fan
  const theme = q.get("theme") || "light";
  const phone = os === "android" || os === "ios";
  document.documentElement.dataset.os = os;
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.pointer = phone ? "coarse" : "fine";

  // ---- sample content (invented) ----
  const I = {
    home: '<svg viewBox="0 0 24 24"><path d="M3 11l9-8 9 8v10a1 1 0 0 1-1 1h-5v-7h-6v7H4a1 1 0 0 1-1-1z"/></svg>',
    bell: '<svg viewBox="0 0 24 24"><path d="M6 8a6 6 0 0 1 12 0v5l2 3H4l2-3zM10 19a2 2 0 0 0 4 0"/></svg>',
    cam: '<svg viewBox="0 0 24 24"><path d="M3 7h12a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H3zM17 10l4-2v8l-4-2"/></svg>',
    print: '<svg viewBox="0 0 24 24"><path d="M6 9V3h12v6M6 18H4a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2M6 14h12v7H6z"/></svg>',
    fan: '<svg viewBox="0 0 24 24"><path d="M12 12m-2 0a2 2 0 1 0 4 0a2 2 0 1 0-4 0M12 10V3c3 0 5 2 5 4s-2 3-5 3M14 13l6 3c-1.5 2.6-4 3.5-5.7 2.5S13 15.5 14 13M10 13l-6 3c1.5 2.6 4 3.5 5.7 2.5S11 15.5 10 13"/></svg>',
    air: '<svg viewBox="0 0 24 24"><path d="M3 8h11a3 3 0 1 0-3-3M3 12h15a3 3 0 1 1-3 3M3 16h8a2 2 0 1 1-2 2"/></svg>',
    gate: '<svg viewBox="0 0 24 24"><path d="M3 21V9l9-6 9 6v12M9 21v-8h6v8"/></svg>',
    tools: '<svg viewBox="0 0 24 24"><path d="M14.7 6.3a4 4 0 0 0 5 5l-9 9a2 2 0 0 1-3-3l9-9zM4 4l4 4"/></svg>',
    gear: '<svg viewBox="0 0 24 24"><path d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>',
    plus: '<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>',
    menu: '<svg viewBox="0 0 24 24"><path d="M4 7h16M4 12h16M4 17h16"/></svg>',
    chev: '<svg viewBox="0 0 24 24"><path d="M9 6l6 6-6 6"/></svg>',
    down: '<svg viewBox="0 0 24 24"><path d="M6 9l6 6 6-6"/></svg>',
    search: '<svg viewBox="0 0 24 24"><path d="M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM20 20l-4-4"/></svg>',
    power: '<svg viewBox="0 0 24 24"><path d="M12 3v9M6.3 6.3a8 8 0 1 0 11.4 0"/></svg>',
    moon: '<svg viewBox="0 0 24 24"><path d="M21 13A8 8 0 1 1 11 3a6 6 0 0 0 10 10z"/></svg>',
    bolt: '<svg viewBox="0 0 24 24"><path d="M13 2L4 14h7l-1 8 9-12h-7z"/></svg>',
    bulb: '<svg viewBox="0 0 24 24"><path d="M9 18h6M10 21h4M12 3a6 6 0 0 0-4 10.5c.7.7 1 1.4 1 2.5h6c0-1.1.3-1.8 1-2.5A6 6 0 0 0 12 3z"/></svg>',
    close: '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>',
    back: '<svg viewBox="0 0 24 24"><path d="M15 6l-6 6 6 6"/></svg>',
    timer: '<svg viewBox="0 0 24 24"><path d="M12 8v5l3 2M12 21a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM9 2h6"/></svg>',
  };

  // Status chips: a count and a noun; each opens its kind's filtered view (H9).
  const chips = state === "empty" ? [] : [
    { icon: I.bell, text: "1 missed ring", kind: "doorbell", tone: "warn" },
    { icon: I.fan, text: "2 fans on", kind: "fans" },
    { icon: I.air, text: "PM2.5 good · 18", kind: "purifiers", tone: "ok" },
    { icon: I.print, text: "Printer: low toner", kind: "print", tone: "warn" },
  ];
  const favourites = state === "empty" ? [] : [
    { icon: I.bell, name: "Front gate", sub: "Doorbell · Live", kind: "doorbell", wide: false, actions: ["Live", "Call"] },
    { icon: I.fan, name: "Bedroom fan", sub: "On · Speed 3", kind: "fans", on: true },
    { icon: I.air, name: "Living room purifier", sub: "Auto · PM2.5 18", kind: "purifiers", on: true },
    { icon: I.cam, name: "Balcony camera", sub: "Live", kind: "cameras", camera: true },
  ];
  const kinds = [
    { id: "doorbell", icon: I.bell, name: "Doorbell", note: state === "empty" ? "not set up yet" : "Ringing here · 1 device", count: 1, open: true,
      devices: [{ icon: I.bell, name: "Front gate", sub: "Door station · online", actions: ["Live", "Call"] }, { icon: I.bell, name: "Lobby monitor", sub: "Indoor monitor · online" }] },
    { id: "cameras", icon: I.cam, name: "Security cameras", note: state === "empty" ? "not signed in yet" : "Tapo · 3 cameras", count: 3,
      devices: [{ icon: I.cam, name: "Balcony camera", sub: "Live", camera: true }, { icon: I.cam, name: "Main road", sub: "Live", camera: true }, { icon: I.cam, name: "Kitchen", sub: "Offline" }] },
    { id: "print", icon: I.print, name: "Printers & scanners", note: state === "empty" ? "none added yet" : "Brother · low toner", count: 1,
      devices: [{ icon: I.print, name: "Brother DCP", sub: "Ready · toner 8%", actions: ["Print", "Scan"] }] },
    { id: "fans", icon: I.fan, name: "Ceiling fans", note: state === "empty" ? "none added yet" : "Atomberg · 2 of 3 on", count: 3,
      devices: [{ icon: I.fan, name: "Bedroom fan", sub: "On · Speed 3", on: true }, { icon: I.fan, name: "Hall fan", sub: "On · Speed 2", on: true }, { icon: I.fan, name: "Study fan", sub: "Off" }] },
    { id: "purifiers", icon: I.air, name: "Air purifiers", note: state === "empty" ? "none added yet" : "Philips · PM2.5 good", count: 1,
      devices: [{ icon: I.air, name: "Living room purifier", sub: "Auto · PM2.5 18", on: true }] },
    { id: "society", icon: I.gate, name: "Society gate", note: state === "empty" ? "not signed in yet" : "MyGate · no visitors waiting", count: 0 },
  ];

  const el = (tag, cls, html) => { const e = document.createElement(tag); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };
  const btn = (cls, html, label) => { const b = el("button", cls, html); b.type = "button"; if (label) b.setAttribute("aria-label", label); return b; };

  // ---- Home content (the same on every OS) ----
  function homeContent() {
    const main = el("main", "home");
    if (state === "urgent") {
      const u = el("div", "urgent", `<span class="urgent-icon">${I.bell}</span><span class="urgent-text"><strong>Front gate is ringing</strong><span>Someone is at the door</span></span>`);
      u.append(btn("urgent-btn primary", "Answer"), btn("urgent-btn", "Open door"));
      main.append(u);
    }
    if (chips.length) {
      const row = el("div", "chips");
      for (const c of chips) row.append(btn(`chip ${c.tone || ""}`, `${c.icon}<span>${c.text}</span>`));
      main.append(row);
    }
    if (favourites.length) {
      main.append(el("h2", "section-title", "Favourites"));
      const grid = el("div", "tiles");
      for (const f of favourites) grid.append(tile(f));
      main.append(grid);
    }
    main.append(el("h2", "section-title", state === "empty" ? "Your devices" : "All devices"));
    for (const k of kinds) main.append(kindGroup(k));
    const add = btn("add-device", `${I.plus}<span>Add a device</span>`);
    main.append(add);
    if (state === "empty") main.append(el("p", "hint", "Nothing is set up yet. Add a doorbell, a camera, a printer, a fan, a purifier or your society gate."));
    return main;
  }
  // A tile: the icon toggles (for a switchable device), the body opens detail (H14).
  function tile(d) {
    const t = el("div", `tile ${d.on ? "on" : ""} ${d.camera ? "camera" : ""} ${d.actions ? "has-actions" : ""}`);
    if (d.camera) {
      const pic = el("div", "tile-pic", `<span class="live">LIVE</span>`);
      t.append(pic);
    }
    const icon = btn("tile-icon", d.icon, d.on != null ? `Turn ${d.name} ${d.on ? "off" : "on"}` : d.name);
    const body = btn("tile-body", `<strong>${d.name}</strong><span>${d.sub}</span>`);
    t.append(icon, body);
    if (d.actions) { const a = el("div", "tile-actions"); for (const x of d.actions) a.append(btn("tile-act", x)); t.append(a); }
    return t;
  }
  function kindGroup(k) {
    const g = el("section", `kind ${k.open ? "open" : ""}`);
    const head = btn("kind-head", `<span class="kind-icon">${k.icon}</span><span class="kind-text"><strong>${k.name}</strong><span>${k.note}</span></span><span class="kind-chev">${I.down}</span>`);
    head.setAttribute("aria-expanded", String(!!k.open));
    g.append(head);
    if (k.open && k.devices) { const grid = el("div", "tiles"); for (const d of k.devices) grid.append(tile(d)); g.append(grid); }
    return g;
  }

  // ---- fan detail: one hero control, power apart, modes, timer (F-section) ----
  function fanDetail() {
    const d = el("div", "detail");
    const head = el("div", "detail-head");
    head.append(btn("detail-close", os === "ios" ? "Done" : I.close, "Close"), el("strong", "detail-title", "Bedroom fan"), btn("detail-more", I.gear, "Settings"));
    d.append(head);
    const hero = el("div", "hero");
    hero.innerHTML = `<div class="hero-label">Speed</div><div class="speed"><span class="step">1</span><span class="step">2</span><span class="step on">3</span><span class="step">4</span><span class="step">5</span><span class="step">6</span></div><div class="hero-value">3 of 6 · On</div>`;
    d.append(hero);
    const row = el("div", "detail-row");
    row.append(btn("mode on", `${I.power}<span>Power</span>`), btn("mode", `${I.bolt}<span>Boost</span>`), btn("mode", `${I.moon}<span>Sleep</span>`), btn("mode", `${I.timer}<span>Timer</span>`));
    d.append(row);
    const list = el("div", "detail-list");
    list.append(el("div", "detail-item", `<span>${I.bulb}</span><span class="grow">Light</span><span class="switch on" role="switch" aria-checked="true"></span>`),
      el("div", "detail-item", `<span>${I.timer}</span><span class="grow">Timer</span><span class="muted">Off</span>${I.chev}`),
      el("div", "detail-item", `<span>${I.fan}</span><span class="grow">Run time today</span><span class="muted">4 h 20 min</span>`));
    d.append(list);
    return d;
  }

  // ---- OS shells ----
  const NAV = [
    { id: "home", icon: I.home, label: "Home", current: true },
    { id: "doorbell", icon: I.bell, label: "Doorbell" },
    { id: "cameras", icon: I.cam, label: "Cameras" },
    { id: "print", icon: I.print, label: "Print & scan" },
    { id: "fans", icon: I.fan, label: "Fans" },
    { id: "purifiers", icon: I.air, label: "Purifiers" },
    { id: "society", icon: I.gate, label: "Society gate" },
    { id: "tools", icon: I.tools, label: "Tools" },
  ];
  const PHONE_TABS = [NAV[0], NAV[7], { id: "settings", icon: I.gear, label: "Settings" }];

  function navList(cls, items, withSettings) {
    const ul = el("nav", cls);
    for (const n of items) {
      const a = el("a", `nav-item ${n.current ? "current" : ""}`, `<span class="nav-icon">${n.icon}</span><span class="nav-label">${n.label}</span>`);
      a.href = "#"; if (n.current) a.setAttribute("aria-current", "page");
      ul.append(a);
    }
    if (withSettings) { const s = el("a", "nav-item nav-settings", `<span class="nav-icon">${I.gear}</span><span class="nav-label">Settings</span>`); s.href = "#"; ul.append(s); }
    return ul;
  }

  function build() {
    const app = document.getElementById("app");
    app.replaceChildren();
    const w = window.innerWidth;
    const content = homeContent();
    if (os === "android") {
      // < 600 dp: top app bar + navigation bar; ≥ 600: navigation rail.
      const wide = w >= 600;
      app.classList.toggle("wide", wide);
      const bar = el("header", "topbar", `<span class="topbar-title">Home</span>`);
      bar.append(btn("icon-btn", I.search, "Search"), btn("icon-btn", I.gear, "Settings"));
      if (wide) {
        const rail = navList("rail", NAV, false);
        app.append(rail, el("div", "page", "").appendChild(bar).parentElement);
        app.lastElementChild.append(content);
      } else {
        app.append(bar, content, navList("navbar", PHONE_TABS, false));
      }
    } else if (os === "ios") {
      const bar = el("header", "topbar", `<span class="topbar-title large">Home</span>`);
      bar.append(btn("icon-btn tint", I.plus, "Add"));
      app.append(bar, content, navList("tabbar", PHONE_TABS, false));
    } else if (os === "windows") {
      // ≤640 hamburger only; 641–1007 icon rail; ≥1008 expanded pane.
      const mode = w >= 1008 ? "expanded" : w > 640 ? "compact" : "minimal";
      app.dataset.navmode = mode;
      const title = el("div", "titlebar", `<span class="title-text">Kagaz</span><span class="title-controls"><span>—</span><span>☐</span><span>✕</span></span>`);
      const body = el("div", "body");
      const pane = el("div", "pane");
      pane.append(btn("icon-btn pane-toggle", I.menu, "Navigation"), navList("pane-list", NAV, false), navList("pane-foot", [{ id: "settings", icon: I.gear, label: "Settings" }], false));
      const page = el("div", "page");
      page.append(el("h1", "page-title", "Home"), content);
      body.append(pane, page);
      app.append(title, body);
    } else if (os === "gnome") {
      // Sidebar split view; collapses to a stack with a bottom switcher under 550 sp / 400 sp.
      const collapsed = w < 550;
      app.classList.toggle("collapsed", collapsed);
      const side = el("div", "sidebar");
      side.append(el("div", "headerbar side", `<span class="hb-title">Kagaz</span>`), navList("sidebar-list", NAV, false));
      const page = el("div", "page");
      const hb = el("div", "headerbar");
      if (collapsed) hb.append(btn("flat-btn", I.menu, "Sidebar"));
      hb.append(el("span", "hb-title", "Home"), btn("flat-btn", I.menu, "Main menu"));
      page.append(hb, content);
      app.append(side, page);
      if (collapsed) app.append(navList("bottom-switcher", [NAV[0], NAV[1], NAV[2], NAV[7]], false));
    } else if (os === "macos") {
      const side = el("div", "sidebar");
      side.append(el("div", "traffic", `<span class="tl r"></span><span class="tl y"></span><span class="tl g"></span>`), navList("sidebar-list", NAV, false));
      const page = el("div", "page");
      const tb = el("div", "toolbar", `<span class="tb-title">Home</span>`);
      tb.append(btn("tb-btn", I.search, "Search"), btn("tb-btn", I.plus, "Add"));
      page.append(tb, content);
      app.append(side, page);
    }
    if (detail === "fan") {
      // Simple device: a bottom sheet under 870 px wide or 500 px tall, else a dialog (H15).
      const sheet = w < 870 || window.innerHeight < 500;
      const scrim = el("div", "scrim");
      const panel = el("div", sheet ? "sheet" : "dialog");
      if (sheet && os !== "ios") panel.append(el("div", "handle"));
      panel.append(fanDetail());
      app.append(scrim, panel);
    }
  }
  build();
  window.addEventListener("resize", build);
})();
