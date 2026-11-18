// The proposed navigation, drawn with made-up devices.
//   frame.html?os=gnome|windows|macos|android|ios
//             &have=some|none|many     what the person has added
//             &at=home|<kind>          the page that is open
//             &phone=steady|own        the bar along a phone's bottom (two proposals)
(() => {
  const q = new URLSearchParams(location.search);
  const root = document.documentElement;
  const os = root.dataset.os || "gnome";
  const $ = (id) => document.getElementById(id);
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };

  // Every kind Kagaz knows, with the name its entry carries. `n`: how many
  // the person has added, for each of the sample set-ups.
  const KINDS = [
    { id: "ac", name: "Air conditioners", title: "Air conditioners", icon: "ac", n: { many: 2 } },
    { id: "cameras", name: "Cameras", title: "Security cameras", icon: "cameras", n: { some: 4, many: 4 } },
    { id: "remote", name: "Computers", title: "Computers & phones", icon: "computer", n: { some: 2, many: 3 } },
    { id: "doorbell", name: "Doorbell", title: "Doorbell", icon: "doorbell", n: { some: 3, many: 3 } },
    { id: "fans", name: "Fans", title: "Ceiling fans", icon: "fan", n: { some: 5, many: 5 } },
    { id: "fridge", name: "Fridges", title: "Fridges", icon: "fridge", n: { many: 1 } },
    { id: "print", name: "Print & scan", title: "Printers & scanners", icon: "print", n: { many: 1 } },
    { id: "purifiers", name: "Purifiers", title: "Air purifiers", icon: "purifier", n: { many: 1 } },
    { id: "vacuum", name: "Robot vacuums", title: "Robot vacuums", icon: "vacuum", n: { many: 1 } },
    { id: "society", name: "Society gate", title: "Society gate", icon: "society", n: { many: 1 } },
    { id: "tv", name: "TVs", title: "TVs", icon: "tv", n: { many: 2 } },
    { id: "washer", name: "Washing machines", title: "Washing machines", icon: "washer", n: { many: 1 } },
  ];
  const have = q.get("have") || "some";
  const at = q.get("at") || "home";
  const phone = q.get("phone") || "steady";
  const byName = (a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  // Only what has been added, by name; and the page that is open, even
  // before its first device is added (the entry shows where the person is).
  const mine = KINDS.filter((k) => (k.n[have] || 0) > 0 || k.id === at).sort(byName);

  // Which navigation the window has room for (as shell.js picks it).
  const w = window.innerWidth, h = window.innerHeight;
  const layout = os === "android" ? (w < 600 && !(h < 500 && w > h) ? "bar" : w >= 1200 ? "sidebar" : "rail")
    : os === "ios" ? (w < 600 ? "bar" : "sidebar")
    : os === "windows" ? (w <= 640 ? "minimal" : w < 1008 ? "rail" : "sidebar")
    : os === "macos" ? "sidebar" : w < 550 ? "bar" : "sidebar";
  root.dataset.layout = layout;
  root.dataset.view = at;

  const entry = (id, name, icon, cls) => {
    const li = el("li", cls || "");
    li.dataset.cat = id;
    li.tabIndex = 0;
    li.setAttribute("role", "button");
    li.title = name;
    const ic = el("span", "cat-icon");
    ic.dataset.icon = icon;
    li.append(ic, el("span", "cat-name", name));
    if (id === at) li.classList.add("active");
    return li;
  };

  // ---------- the side list ----------
  // Home; then the person's own kinds, by name, and Add a device at their
  // end (every kind Kagaz knows is behind it); Tools and Settings stay at
  // the foot. The kinds scroll between the two when they outgrow the window.
  const list = $("categories");
  const scroll = el("li", "cat-scroll");
  scroll.setAttribute("role", "presentation");
  const inner = el("ul", "cat-kinds");
  if (mine.length) inner.append(el("li", "cat-head", "Your devices"));
  for (const k of mine) inner.append(entry(k.id, k.name, k.icon));
  inner.append(entry("add", "Add a device", "plus", "cat-add"));
  scroll.append(inner);
  list.append(entry("home", "Home", "home"), scroll, entry("tools", "Tools", "tools", "cat-foot"), entry("settings", "Settings", "settings"));

  // ---------- a phone's bar ----------
  const tab = (id, name, icon) => {
    const b = el("button");
    b.type = "button";
    b.dataset.cat = id;
    const ic = el("span", "cat-icon");
    ic.dataset.icon = icon;
    b.append(ic, el("span", "cat-name", name));
    if (id === at) b.classList.add("active");
    return b;
  };
  const bar = $("bottom-nav");
  if (phone === "own") {
    // The other proposal: the person's first three kinds in the bar itself.
    bar.append(tab("home", "Home", "home"), ...mine.slice(0, 3).map((k) => tab(k.id, k.name, k.icon)), tab("more", "More", "more"));
  } else {
    bar.append(tab("home", "Home", "home"), tab("tools", "Tools", "tools"), tab("settings", "Settings", "settings"));
  }

  // ---------- the page ----------
  const page = $("home");
  const open = KINDS.find((k) => k.id === at);
  $("bar-title").textContent = open ? open.title : "Home";
  $("bar-add").classList.toggle("hidden", at !== "home");
  if (open) {
    const p = el("div", "empty-state");
    p.append(el("p", "", (open.n[have] || 0) ? `${open.title}: the page as it is today.` : `No ${open.title.toLowerCase()} added yet.`),
      el("p", "muted", (open.n[have] || 0) ? "" : "Its entry shows in the list while this page is open, and stays once the first one is added."));
    page.append(p);
  } else {
    const head = el("div", "section-head");
    head.append(el("h2", "section-title", mine.length ? "All devices" : "Your devices"));
    page.append(head);
    const groups = el("div");
    groups.id = "home-groups";
    for (const k of mine) {
      const box = el("section", "home-group");
      const row = el("div", "home-group-head");
      const toggle = el("button", "home-group-toggle");
      toggle.type = "button";
      const ic = el("span", "kind-icon");
      ic.dataset.icon = k.icon;
      const words = el("span", "home-group-words");
      const n = k.n[have] || 0;
      words.append(el("span", "home-group-title", k.title), el("span", "home-status", k.id === "society" ? "Nobody at the gate" : `${n} added`));
      const chev = el("span", "home-chevron");
      chev.dataset.icon = "down";
      toggle.append(ic, words, chev);
      const go = el("button", "link home-open", "Open");
      go.type = "button";
      row.append(toggle, go);
      box.append(row);
      groups.append(box);
    }
    page.append(groups);
    if (!mine.length) {
      const empty = el("div", "empty-state");
      empty.append(el("p", "", "Nothing set up yet."), el("p", "muted", "Add a doorbell, security cameras, a printer, a fan or a computer to see it here and in the list."));
      page.append(empty);
    }
    const add = el("button", "add-device");
    add.type = "button";
    const plus = el("span", "ic");
    plus.dataset.icon = "plus";
    add.append(plus, el("span", "", "Add a device"));
    page.append(add);
  }
  // The icons are filled in as the app does it.
  if (typeof KagazIcons !== "undefined") KagazIcons.fill(document);
})();
