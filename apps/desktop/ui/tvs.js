// TVs & displays: the TVs the person added and the ones found nearby, and
// every way this device can show its screen on one: Kagaz's own sender
// (Miracast, over the network, or Wi-Fi Direct from Linux), a second
// screen (Linux), and the system's own (Windows' Cast, the phone's Cast
// screen, Samsung DeX, AirPlay on a Mac). Nothing reaches a TV until the
// person presses a button here, and the system's own prompt (the
// desktop's sharing dialog, Android's) comes first.
//
// Layout: the TVs first, each a card with its state and one main button
// (mirror, or stop while mirroring); the other ways in a sheet; news in a
// snackbar at the bottom, a failure inside the card it belongs to.
// Uses app.js's globals: $, invoke, listen; ask.js's KagazAsk; icons.js's
// KagazIcons.

const KagazTvs = (() => {
  const OS = window.__KAGAZ_OS__ || "gnome";
  const PHONE = OS === "android" || OS === "ios";
  const SAMSUNG = /SM-|Samsung/i.test(navigator.userAgent);
  const DEVICE = PHONE ? "phone" : "computer";
  let saved = [];
  let found = [];
  let scanning = false;
  let lastScan = 0;
  let showing = null; // the TV this screen is on now
  let line = ""; // the session's latest news
  let failed = {}; // TV name -> why its last try failed

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function button(cls, text, onClick, label) {
    // Text, or nodes (an icon and its word).
    const b = el("button", cls, typeof text === "string" ? text : null);
    if (text && typeof text !== "string") b.append(text);
    b.type = "button";
    if (label) b.setAttribute("aria-label", label);
    b.addEventListener("click", onClick);
    return b;
  }

  const icon = (name) => {
    const i = el("span", "ic");
    i.innerHTML = KagazIcons.svg(name);
    return i;
  };
  const withIcon = (name, word) => {
    const b = document.createDocumentFragment();
    b.append(icon(name), el("span", "", word));
    return b;
  };

  // The snackbar: a short line at the bottom that goes by itself.
  let toastTimer = null;
  function toast(text, long) {
    const t = $("tvs-toast");
    t.textContent = text;
    t.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => t.classList.add("hidden"), long ? 8000 : 4000);
  }

  // For the Home tile.
  function homeWords(t) {
    if (showing === t.name) return `Showing this ${DEVICE}`;
    const here = found.find((f) => f.name === t.name);
    return here ? (here.mirror ? "Ready to mirror" : "On the network") : t.model || "TV";
  }

  // The TV's state in a few words, and how it should look.
  function state(t) {
    if (showing === t.name) return { text: line || "Starting…", tone: "on" };
    if (failed[t.name]) return { text: "Did not connect", tone: "bad" };
    const here = found.find((f) => f.name === t.name);
    if (here && here.mirror) return { text: "Ready to mirror", tone: "ok" };
    if (here) return { text: "On the network, not taking mirroring now", tone: "" };
    return { text: lastScan ? "Not found on this network" : "Looking…", tone: "" };
  }

  // Kagaz's own: this device's screen on the TV.
  async function show(t, what) {
    if (showing) return;
    delete failed[t.name];
    try {
      if (OS === "android") {
        await invoke("tv_phone", { what: "consent" });
      } else if (OS === "gnome" && what !== "extend") {
        toast("Choose what to share in the desktop's dialog.");
      }
      await invoke("tv_show", { name: t.name, what });
      showing = t.name;
      line = "Starting…";
    } catch (e) {
      failed[t.name] = String(e);
    }
    render();
  }

  // The system's own way.
  async function system(what) {
    try {
      const said = PHONE ? await invoke("tv_phone", { what }) : await invoke("tv_system_sender");
      toast(said, true);
    } catch (e) {
      toast(String(e), true);
    }
  }

  // A TV's sheet (a bottom sheet on a phone, a dialog on a computer, as
  // for every simple device): its other ways, then the TV itself.
  function sheet(t) {
    const ways = [];
    if (PHONE) {
      ways.push({ name: "The phone's Cast screen", what: "Android's own. Pick the TV there; turn on Wireless display at the top for TVs that take screen mirroring.", go: () => system("systemCast") });
      if (SAMSUNG) ways.push({ name: "Samsung DeX", what: "A desktop of its own on the TV, not a copy of the phone. Link to Windows has to be off.", go: () => system("dex") });
    } else {
      if (OS === "gnome") {
        ways.push({ name: "Mirror one window", what: "Only the window you choose goes to the TV.", go: () => show(t, "window"), disabled: !!showing });
        ways.push({ name: "Use the TV as a second screen", what: "A new monitor on the TV: drag windows onto it.", go: () => show(t, "extend"), disabled: !!showing });
      }
      const own = {
        windows: ["Windows' Cast", "Windows' own (Win+K); it also reaches TVs on no network."],
        macos: ["AirPlay", "The Mac's own, for TVs with AirPlay."],
        gnome: ["The system's own sender", "GNOME Network Displays, if it is installed."],
      }[OS] || ["The system's own sender", ""];
      ways.push({ name: own[0], what: own[1], go: () => system("system") });
    }
    KagazAsk.ask((dlg, finish) => {
      dlg.classList.add("sheet");
      dlg.appendChild(el("h3", "ask-title", t.name));
      const st = state(t);
      dlg.appendChild(el("p", "muted small", [t.model, st.text].filter(Boolean).join(" · ")));
      if (failed[t.name]) dlg.appendChild(el("p", "cap-warn small", failed[t.name]));
      const list = el("div", "tv-ways");
      list.appendChild(el("div", "tv-way-section", "Other ways"));
      const add = (c) => {
        const b = button("tv-way" + (c.danger ? " danger-text" : ""), null, () => {
          finish(null);
          c.go();
        });
        b.appendChild(el("span", "tv-way-name", c.name));
        if (c.what) b.appendChild(el("span", "tv-way-what muted small", c.what));
        b.disabled = !!c.disabled;
        list.appendChild(b);
      };
      ways.forEach(add);
      add({ name: "Remove from your TVs", danger: true, go: () => remove(t) });
      dlg.appendChild(list);
      const row = el("div", "actions");
      row.appendChild(button("", "Close", () => finish(null)));
      dlg.appendChild(row);
    });
  }

  async function remove(t) {
    if (!(await KagazAsk.confirm(`Remove ${t.name} from your TVs?`, "Remove TV", "The TV stays as it is; a search finds it again."))) return;
    try {
      saved = await invoke("tv_forget", { name: t.name });
    } catch (e) {
      toast("Could not remove it: " + e);
    }
    render();
    if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
  }

  // A TV as Home's device tile: tap it for its sheet; the tile's own
  // button mirrors (Stop while mirroring), as Connect does on Computers &
  // phones.
  function tile(t) {
    const st = state(t);
    const live = showing === t.name;
    const tile = el("div", "home-tile has-actions" + (live ? " on live" : st.tone === "bad" ? " bad" : ""));
    const main = button("tile-main", null, () => sheet(t), `${t.name}: ${st.text}`);
    const ic = el("span", "tile-icon");
    ic.innerHTML = KagazIcons.svg("tv");
    const text = el("span", "tile-text");
    const sub = el("span", "tile-sub");
    sub.append(el("span", "rc-dot" + (st.tone === "ok" || live ? " ok" : "")), st.text);
    if (live) sub.setAttribute("aria-live", "polite");
    text.append(el("span", "tile-name", t.name), sub);
    const chev = el("span", "home-chevron rc-chev");
    chev.innerHTML = KagazIcons.svg("forward");
    main.append(ic, text, chev);
    const acts = el("span", "tile-actions");
    const go = live
      ? button("tile-act", withIcon("pause", "Stop"), () => invoke("tv_stop").catch(() => {}), `Stop mirroring to ${t.name}`)
      : button("tile-act", withIcon("tv", "Mirror"), () => show(t, "screen"), `Mirror this ${DEVICE} to ${t.name}`);
    if (!live) go.disabled = !!showing;
    acts.append(go);
    tile.append(main, acts);
    return tile;
  }

  // A TV found nearby, as Computers & phones lists them.
  function foundRow(f) {
    const li = el("li", "rc-row");
    const ic = el("span", "tile-icon");
    ic.innerHTML = KagazIcons.svg("tv");
    const text = el("span", "tile-text");
    text.append(el("span", "tile-name", f.name), el("span", "tile-sub", [f.model, f.mirror ? "takes screen mirroring" : f.ways.join(", ")].filter(Boolean).join(" · ")));
    const add = button("", "Add", async () => {
      add.disabled = true;
      try {
        saved = await invoke("tv_add", { name: f.name, model: f.model, address: f.address });
        toast(`${f.name} is in your TVs.`);
      } catch (e) {
        toast("Could not add it: " + e);
      }
      render();
      if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
    }, `Add ${f.name}`);
    li.append(ic, text, add);
    return li;
  }

  function render() {
    $("tvs-tiles").replaceChildren(...saved.map(tile));
    $("tvs-tiles").classList.toggle("hidden", saved.length === 0);
    $("tvs-empty").classList.toggle("hidden", saved.length > 0);
    const others = found.filter((f) => !f.added && !saved.some((s) => s.name === f.name));
    $("tvs-found").replaceChildren(...others.map(foundRow));
    $("tvs-found-head").classList.toggle("hidden", others.length === 0);
  }

  async function rescan() {
    if (scanning) return;
    scanning = true;
    // The bar's refresh turns while looking: the one sign of it.
    const b = $("bar-refresh");
    b.disabled = true;
    b.classList.add("working");
    try {
      found = await invoke("tvs_search");
      saved = await invoke("tvs_saved");
      lastScan = Date.now();
      if (!found.length) toast("No TV answered on this network. Is it on, and on this Wi-Fi?", true);
    } catch (e) {
      toast("Could not search: " + e, true);
    }
    b.classList.remove("working");
    b.disabled = false;
    scanning = false;
    render();
    if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
  }

  async function showPage() {
    try {
      saved = (await invoke("tvs_saved")) || saved;
      showing = (await invoke("tv_showing")) || showing;
    } catch (e) {
      toast("Could not read your TVs: " + e);
    }
    render();
    if (Date.now() - lastScan > 2 * 60 * 1000) rescan();
  }

  // A session's news: the live line in the card, the end in a snackbar,
  // a failure in the card.
  listen("tv-session", (e) => {
    const n = e.payload || {};
    if (n.phase === "ended") {
      showing = null;
      toast(n.line);
    } else if (n.phase === "failed") {
      showing = null;
      failed[n.tv] = n.line;
    } else {
      showing = n.tv;
      // The progress line, shortened for the card.
      const m = /(\d+) s, (\d+) pictures, (\d+) kbit\/s; the screen gave (\d+)\/s/.exec(n.line);
      line = m ? `Mirroring · ${m[4]} pictures a second · ${(m[3] / 1000).toFixed(1)} Mbit/s` : n.line;
    }
    render();
  });

  $("tvs").addEventListener("refresh", rescan);

  return { show: showPage, rescan, homeWords, saved: () => invoke("tvs_saved").catch(() => []) };
})();
