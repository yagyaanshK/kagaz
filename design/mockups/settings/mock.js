// Settings, put in order; drawn with made-up devices.
//   set.html?os=gnome|windows|macos|android|ios
//           &screen=index|kind|device|search
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
  const icon = (name, cls) => {
    const s = el("span", cls || "ic");
    s.dataset.icon = name;
    return s;
  };
  const screen = q.get("screen") || "index";
  const w = window.innerWidth;
  const phone = (os === "android" || os === "ios") ? w < 600 : w < 550;
  root.dataset.layout = phone ? "bar" : "sidebar";

  // ---------- the frame ----------
  const entry = (id, label, ic, cls) => {
    const li = el("li", cls || "");
    li.append(icon(ic, "cat-icon"), el("span", "cat-name", label));
    if (id === "settings") li.classList.add("active");
    return li;
  };
  const scroll = el("li", "cat-scroll");
  const kinds = el("ul", "cat-kinds");
  kinds.append(el("li", "cat-head", "Your devices"), entry("cameras", "Cameras", "cameras"), entry("remote", "Computers", "computer"),
    entry("doorbell", "Doorbell", "doorbell"), entry("fans", "Fans", "fan"), entry("add", "Add a device", "plus", "cat-add"));
  scroll.append(kinds);
  $("categories").append(entry("home", "Home", "home"), scroll, entry("tools", "Tools", "tools", "cat-foot"), entry("settings", "Settings", "settings"));
  for (const [id, label, ic] of [["home", "Home", "home"], ["tools", "Tools", "tools"], ["settings", "Settings", "settings"]]) {
    const b = el("button");
    b.type = "button";
    b.append(icon(ic, "cat-icon"), el("span", "cat-name", label));
    if (id === "settings") b.classList.add("active");
    $("bottom-nav").append(b);
  }

  // ---------- parts of a settings page ----------
  const body = $("settings-body");
  const section = (title, note) => {
    const box = el("section", "set-section");
    box.append(el("h3", "", title));
    if (note) box.append(el("p", "muted small", note));
    body.append(box);
    return box;
  };
  // A row that sets something: its words, and one control.
  const row = (label, note, control, own) => {
    const r = el("div", "set-row");
    const l = el("label", "set-label");
    const words = el("span", "", label);
    if (own) words.append(el("span", "set-own-mark", "Its own"));
    l.append(words);
    if (note) l.append(el("span", "muted small", note));
    const c = el("span", "set-control");
    c.append(control);
    if (own) {
      const reset = el("button", "link", "Same as all");
      reset.type = "button";
      c.append(reset);
    }
    r.append(l, c);
    return r;
  };
  // A row that leads somewhere: its words, what is inside, an arrow.
  const door = (ic, label, note) => {
    const r = el("div", "set-row set-door");
    r.tabIndex = 0;
    r.setAttribute("role", "button");
    const l = el("span", "set-label");
    l.append(el("span", "", label));
    if (note) l.append(el("span", "muted small", note));
    r.append(icon(ic, "set-door-icon ic"), l, icon("forward", "set-door-go ic"));
    return r;
  };
  const select = (options, chosen) => {
    const s = el("select");
    options.forEach((o, i) => {
      const op = el("option", "", o);
      op.selected = i === (chosen || 0);
      s.append(op);
    });
    return s;
  };
  const onoff = (on) => {
    const l = el("label", "switch");
    const i = el("input");
    i.type = "checkbox";
    i.checked = on;
    l.append(i);
    return l;
  };
  const search = (typed) => {
    const box = el("div", "set-search");
    const i = el("input");
    i.type = "search";
    i.placeholder = "Find a setting";
    if (typed) i.value = typed;
    box.append(icon("search"), i);
    body.append(box);
  };
  const back = (word) => {
    $("bar-back").classList.remove("hidden");
    $("bar-back").querySelector(".bar-back-word").textContent = word;
  };

  const KEYS = ["In full screen only", "Whenever the pointer is on Kagaz", "Never: they stay with this computer"];

  if (screen === "index") {
    // One door to every setting: Kagaz's own, then one row for each kind
    // of device the person has, by name.
    $("bar-title").textContent = "Settings";
    search();
    const app = section("Kagaz");
    app.append(door("family", "Accounts", "OneTouch, Tapo, Xiaomi: signed in"),
      row("Do not disturb", "No ring and no notification until turned off", onoff(false)),
      door("file", "Where things are saved", "Screenshots, recordings, scans"),
      door("check", "About Kagaz", "Version, licence, third-party licences"));
    const mine = section("Your devices", "What each kind does for all of its devices, and each device's own.");
    mine.append(door("cameras", "Cameras", "Sound, quality, where recordings go"),
      door("computer", "Computers & phones", "Session windows, keyboard, sound · 3 devices"),
      door("doorbell", "Doorbell", "Ringtones, who rings here · 3 devices"),
      door("fan", "Fans", "5 fans"));
  } else if (screen === "kind") {
    // A kind's settings: what holds for all of its devices first, then a
    // door to each device. The same page its gear opens.
    $("bar-title").textContent = "Computers & phones";
    back("Settings");
    const win = section("Session windows");
    win.append(row("When Kagaz's main window closes", "", select(["End every session, after asking", "Keep the session windows open"])),
      row("Show a call at the door in a session's window", "A strip with Answer and Decline, also in full screen", onoff(true)));
    const all = section("For every computer and phone", "A device follows these unless it is given its own.");
    all.append(row("This computer's own keys go to the device", "Print Screen, the Windows key, Alt + Tab", select(KEYS)),
      row("Play its sound here", "", onoff(true)),
      row("Fit its screen to the window", "", onoff(true)));
    const each = section("Each device");
    each.append(door("laptop", "Study laptop", "2 settings of its own"), door("computer", "Workshop PC", "Same as all"), door("mobile", "Old phone", "Same as all"));
  } else if (screen === "device") {
    // A device's settings: only it. What it takes from its kind says so;
    // what is its own is marked and can be given back.
    $("bar-title").textContent = "Study laptop";
    back("Computers & phones");
    const k = section("Keyboard and mouse");
    k.append(row("This computer's own keys go to it", "Print Screen, the Windows key, Alt + Tab", select(KEYS, 1), true),
      row("Keep left click on the left", "For a device whose mouse buttons are swapped", onoff(true)));
    const s = section("Screen and sound");
    s.append(row("Size", "", select(["Same as all: fit this window, sharp", "Fit this window, sharp", "A fixed size, scaled to the window"])),
      row("Play its sound here", "", select(["Same as all: yes", "Yes", "No"])));
    const more = el("details", "set-more");
    more.append(el("summary", "", "More: for when something is wrong"));
    s.append(more);
    const i = section("Sign-in and reaching it");
    i.append(row("Sign in as", "", (() => { const t = el("input"); t.type = "text"; t.value = "asha"; return t; })()),
      row("Remember the password", "In this computer's password store", onoff(false)),
      door("reload", "Ways to reach it", "2 addresses · the first to answer is used"));
  } else {
    // Finding one: every level answers, and each result says where it lives.
    $("bar-title").textContent = "Settings";
    search("ring");
    const found = section("4 found");
    found.append(door("bell", "Do not disturb", "Kagaz"),
      door("doorbell", "Ringtone", "Doorbell › Every doorbell"),
      door("doorbell", "Ring me here", "Doorbell › OneTouch"),
      door("computer", "Show a call at the door in a session's window", "Computers & phones › Session windows"));
  }
  if (typeof KagazIcons !== "undefined") KagazIcons.fill(document);
})();
