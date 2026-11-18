// TVs & displays: the TVs the person added and the ones found nearby, and
// every way this device can show its screen on one: Kagaz's own sender
// (Miracast, over the network, or Wi-Fi Direct from Linux), a second
// screen (Linux), and the system's own (Windows' Cast, Android's Cast or
// Samsung's Smart View, Samsung DeX, AirPlay on a Mac). Nothing reaches a
// TV until the person presses a button here, and the system's own prompt
// (the desktop's sharing dialog, Android's) comes first.
// Uses app.js's globals: $, invoke; ask.js's KagazAsk.

const KagazTvs = (() => {
  const OS = window.__KAGAZ_OS__ || "gnome";
  const PHONE = OS === "android" || OS === "ios";
  const SAMSUNG = /SM-|Samsung/i.test(navigator.userAgent);
  let saved = [];
  let found = [];
  let scanning = false;
  let lastScan = 0;
  let showing = null; // the TV this screen is on now
  let line = "";

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function button(cls, text, onClick, label) {
    const b = el("button", cls, text);
    b.type = "button";
    if (label) b.setAttribute("aria-label", label);
    b.addEventListener("click", onClick);
    return b;
  }

  function note(text) {
    $("tvs-note").textContent = text;
  }

  // For the Home tile.
  function homeWords(t) {
    if (showing === t.name) return "Showing this screen";
    return found.some((f) => f.name === t.name) ? "On the network" : t.model || "TV";
  }

  // Kagaz's own: this device's screen on the TV.
  async function show(t, what) {
    if (showing) return;
    try {
      if (OS === "android") {
        note("Allow screen sharing in Android's prompt…");
        await invoke("tv_phone", { what: "consent" });
      } else if (OS === "gnome" && what !== "extend") {
        note("Choose what to share in the desktop's dialog…");
      }
      await invoke("tv_show", { name: t.name, what });
      showing = t.name;
      line = "Starting…";
      render();
    } catch (e) {
      note(String(e));
    }
  }

  // The system's own sender.
  async function system(what) {
    try {
      const said = PHONE ? await invoke("tv_phone", { what }) : await invoke("tv_system_sender");
      note(said);
    } catch (e) {
      note(String(e));
    }
  }

  function ways(t) {
    const row = el("div", "actions");
    const busy = !!showing;
    if (PHONE) {
      const b = button("accent", "Show this phone", () => show(t, "screen"));
      b.disabled = busy;
      row.appendChild(b);
      row.appendChild(button("", SAMSUNG ? "Smart View" : "Android's Cast", () => system("systemCast"), "Use the phone's own way to the TV"));
      if (SAMSUNG) row.appendChild(button("", "Samsung DeX", () => system("dex"), "A desktop on the TV with Samsung DeX"));
    } else {
      const b = button("accent", "Show this screen", () => show(t, "screen"));
      b.disabled = busy;
      row.appendChild(b);
      if (OS === "gnome") {
        const w = button("", "Show a window", () => show(t, "window"));
        w.disabled = busy;
        const x = button("", "Use as a second screen", () => show(t, "extend"), "Make the TV a second screen for this computer");
        x.disabled = busy;
        row.append(w, x);
      }
      const own = { windows: "Windows' Cast", macos: "AirPlay", gnome: "The system's own" }[OS] || "The system's own";
      row.appendChild(button("", own, () => system("system"), "Use this computer's own way to the TV"));
    }
    return row;
  }

  function card(t) {
    const li = el("li", "purifier-card tv-card");
    li.appendChild(el("div", "name", t.name));
    li.appendChild(el("div", "where", [t.model, t.address].filter(Boolean).join(" · ")));
    const here = found.find((f) => f.name === t.name);
    const state = el("div", "caps");
    if (showing === t.name) {
      state.appendChild(el("span", "cap-ok", "Showing this screen"));
    } else if (here) {
      state.appendChild(el("span", here.mirror ? "cap-ok" : "muted", here.mirror ? "Ready for this screen" : "On the network"));
      if (here.ways.length) state.append(" · ", el("span", "muted", here.ways.join(", ")));
    } else {
      state.appendChild(el("span", "muted", lastScan ? "Not on this network now (Wi-Fi Direct may still reach it)" : "Not looked for yet"));
    }
    li.appendChild(state);
    li.appendChild(ways(t));
    const more = el("div", "actions");
    more.appendChild(button("danger-text", "Remove", async () => {
      if (!(await KagazAsk.confirm(`Remove ${t.name} from your TVs?`, "Remove TV", "The TV stays as it is; a search finds it again."))) return;
      try {
        saved = await invoke("tv_forget", { name: t.name });
      } catch (e) {
        note("Could not remove it: " + e);
      }
      render();
      if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
    }));
    li.appendChild(more);
    return li;
  }

  function foundCard(f) {
    const li = el("li", "purifier-card tv-card");
    li.appendChild(el("div", "name", f.name));
    li.appendChild(el("div", "where", [f.model, f.address].filter(Boolean).join(" · ")));
    li.appendChild(el("div", "caps muted", f.ways.join(", ") || "a TV"));
    const add = button("add-printer accent", "Add", async () => {
      add.disabled = true;
      try {
        saved = await invoke("tv_add", { name: f.name, model: f.model, address: f.address });
      } catch (e) {
        note("Could not add it: " + e);
      }
      render();
      if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
    }, `Add ${f.name} to your TVs`);
    li.appendChild(add);
    return li;
  }

  function render() {
    $("tvs-list").replaceChildren(...saved.map(card));
    $("tvs-empty").classList.toggle("hidden", saved.length > 0);
    const others = found.filter((f) => !f.added && !saved.some((s) => s.name === f.name));
    $("tvs-found").replaceChildren(...others.map(foundCard));
    $("tvs-found-head").classList.toggle("hidden", others.length === 0);
    const live = $("tvs-live");
    live.classList.toggle("hidden", !showing);
    if (showing) {
      $("tvs-live-title").textContent = `On ${showing}`;
      $("tvs-live-line").textContent = line;
    }
  }

  async function rescan() {
    if (scanning) return;
    scanning = true;
    $("tvs-rescan").disabled = true;
    const n = $("tvs-note");
    n.textContent = "Looking for TVs on the network…";
    n.classList.add("working");
    try {
      found = await invoke("tvs_search");
      saved = await invoke("tvs_saved");
      lastScan = Date.now();
      const fresh = found.filter((f) => !saved.some((s) => s.name === f.name)).length;
      n.textContent = found.length
        ? fresh ? `${fresh} found. Add one to keep it in your TVs.` : "Your TVs answered."
        : "No TV answered on this network. Is it on, and on this Wi-Fi?" + (OS === "gnome" ? " A TV on no network is reached over Wi-Fi Direct from its card." : "");
    } catch (e) {
      n.textContent = "Could not search: " + e;
    }
    n.classList.remove("working");
    $("tvs-rescan").disabled = false;
    scanning = false;
    render();
    if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
  }

  async function showPage() {
    try {
      saved = (await invoke("tvs_saved")) || saved;
      showing = (await invoke("tv_showing")) || showing;
    } catch (e) {
      note("Could not read your TVs: " + e);
    }
    render();
    if (Date.now() - lastScan > 2 * 60 * 1000) rescan();
  }

  // A session's news.
  listen("tv-session", (e) => {
    const n = e.payload || {};
    if (n.phase === "ended" || n.phase === "failed") {
      showing = null;
      note(n.line);
    } else {
      showing = n.tv;
      line = n.line;
    }
    render();
  });

  $("tvs-rescan").addEventListener("click", rescan);
  $("tvs-stop").addEventListener("click", () => invoke("tv_stop").catch(() => {}));

  return { show: showPage, rescan, homeWords, saved: () => invoke("tvs_saved").catch(() => []) };
})();
