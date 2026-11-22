// Computers & phones: the person's own devices, each with every way to
// reach it and how it does now; the ones nearby, not added yet (on this
// network, in Tailscale); Add; and one device's page with its paths and
// its settings. Connect opens the viewer (remote-view.js) on a device that
// speaks Remote Desktop or VNC; where this build has no Remote Desktop
// sessions (a phone, so far) a device that speaks only that says so
// instead of offering a Connect that fails.
// Uses app.js's globals: $, invoke; ask.js's KagazAsk; icons.js; nav.js.

const KagazRemote = (() => {
  let data = { devices: [] }; // the saved list
  const states = new Map(); // id -> how it does now (remote_check)
  let near = null; // { network, tailscale, peers }, the last look around
  let checking = false;
  let looking = false;
  let lastLook = 0;
  let openId = null; // the device whose page is open
  let viewId = null; // the device whose screen is shown
  let canRemember = false; // this system has a password store Kagaz can use
  let canRdp = false; // this build opens Remote Desktop sessions

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function button(cls, text, onClick, label) {
    const b = el("button", cls);
    b.type = "button";
    if (typeof text === "string") b.textContent = text;
    else b.append(...text);
    if (label) b.setAttribute("aria-label", label);
    b.addEventListener("click", onClick);
    return b;
  }
  const icon = (name) => {
    const s = el("span", "ic");
    s.innerHTML = KagazIcons.svg(name);
    return s;
  };
  const withIcon = (name, word) => [icon(name), el("span", "", word)];

  const SYSTEM = { windows: "Windows", linux: "Linux", android: "Android", macos: "macOS", ios: "iPhone" };
  const shapeIcon = (d) => ({ phone: "mobile", laptop: "laptop" }[d.shape] || "computer");

  function ago(seconds) {
    if (!seconds) return "";
    const s = Math.max(0, Math.floor(Date.now() / 1000) - seconds);
    if (s < 90) return "just now";
    if (s < 3600) return `${Math.round(s / 60)} min ago`;
    if (s < 86400) return `${Math.round(s / 3600)} h ago`;
    return `${Math.round(s / 86400)} d ago`;
  }

  // How a device does, in a few words: for its tile, its page and Home.
  function reachWords(d) {
    const st = states.get(d.id);
    if (st && st.within) return { ok: true, text: `Within reach: ${low(st.path)}, ${st.millis} ms` };
    if (st) return { ok: false, text: `Out of reach: ${st.why}` };
    if (checking) return { ok: false, text: "Checking…" };
    if (d.last_seen) return { ok: false, text: `Last reached ${ago(d.last_seen)}${d.last_path ? " over " + low(d.last_path) : ""}` };
    return { ok: false, text: "Not reached yet" };
  }
  // A path's label inside a sentence ("its hotspot"; Tailscale is a name).
  const low = (label) => (label === "Tailscale" ? label : label.charAt(0).toLowerCase() + label.slice(1));
  // Home's tile is narrow: the state, without the path and the reason.
  function homeWords(d) {
    const st = states.get(d.id);
    if (st) return st.within ? `Within reach · ${st.millis} ms` : "Out of reach";
    return reachWords(d).text;
  }

  function stateLine(d, withSystem) {
    const r = reachWords(d);
    const line = el("span", "tile-sub");
    const shown = windowsOf(d.id).length;
    line.append(el("span", "rc-dot" + (r.ok || shown ? " ok" : "")));
    const sys = SYSTEM[d.system];
    if (withSystem && sys) line.append(el("span", "rc-what", sys + " · "));
    // A session is open with it: that says more than how it was reached.
    line.append(shown ? (shown > 1 ? `In ${shown} windows of its own` : "In its own window") : r.text);
    return line;
  }

  // ---------- connecting ----------
  // Can a session be opened with it from here?
  const canConnect = (d) => d.ways.some((w) => w.speaks === "vnc" || (canRdp && w.speaks === "rdp"));
  const overRdp = (d) => canRdp && d.ways.some((w) => w.speaks === "rdp");
  // Why Connect is off for a device: what it takes says what is missing.
  // (A device that takes only SSH said Remote Desktop was missing on the
  // phone, and its greyed Connect looked frozen: reported 2026-11-22.)
  function whyNot(d) {
    const takes = (s) => d.ways.some((w) => w.speaks === s);
    if (!takes("rdp") && !takes("vnc")) {
      return `${d.name} takes only SSH here, which shows no screen. To see and use its screen, turn on Remote Desktop on it (Ubuntu or Windows Pro: Settings › System › Remote Desktop; a Mac: Screen Sharing), then Check again.`;
    }
    return `${d.name} takes Remote Desktop, which Kagaz on a phone can't open yet. Connect from Kagaz on a computer, or turn on VNC on it and Check again.`;
  }

  // On a computer every session has a window of its own, and several
  // run at once (remote_windows.rs); the list says which devices have
  // one. A device that has a window is asked about before another is
  // opened for it: what a second connection does is the device's to say,
  // and most Windows computers give an account one screen.
  let windows = []; // [{ label, device, name }]
  const windowsOf = (id) => windows.filter((w) => w.device === id);
  async function connect(id, show) {
    const d = data.devices.find((x) => x.id === id);
    if (!d) return;
    let again = false;
    if (windowsOf(id).length && !show) {
      again = await KagazAsk.confirm(`${d.name} already has a window open`, "Connect again",
        "Windows usually gives each account one screen: connecting again takes it over from the window already open. "
        + "With another account, or on a computer that allows it, both run side by side.", "Show that window");
    }
    try {
      const label = await invoke("remote_window_open", { device: id, again });
      invoke("page_log", { line: `remote: ${label} is the window for this session${again ? " (another for the same device)" : ""}` }).catch(() => {});
      return;
    } catch (e) {
      // No windows of their own here (a phone): the viewer takes this one.
      invoke("page_log", { line: "remote: no window of its own here; the session shows in this one" }).catch(() => {});
    }
    connectHere(id);
  }
  function windowsChanged(list) {
    const before = new Set(windows.map((w) => w.device));
    windows = list || [];
    renderList();
    if (viewIs("device")) renderDevice();
    // A session that ended may have changed how its device does.
    for (const id of before) if (!windowsOf(id).length) check(id);
  }
  if (window.__TAURI__.event && window.__TAURI__.event.listen) {
    window.__TAURI__.event.listen("remote-windows", (ev) => windowsChanged(ev.payload));
    // From a session's window: its device's page, to edit its addresses.
    window.__TAURI__.event.listen("remote-edit-device", (ev) => {
      selectCategory("remote");
      openDevice(ev.payload);
    });
    // A setting changed behind a session window's gear: the list and an
    // open page show it.
    window.__TAURI__.event.listen("remote-device-settings", async (ev) => {
      if (!ev.payload || ev.payload.from === "main") return;
      try {
        data = (await invoke("remote_saved")) || data;
      } catch (e) {}
      if (viewIs("device") && openId === ev.payload.id) renderDevice();
      if (viewIs("settings")) renderSettings();
    });
  }
  invoke("remote_windows").then(windowsChanged).catch(() => {});

  // Kagaz's main window is being closed with sessions open in windows of
  // their own (remote_windows.rs holds it back): that ends them all, so
  // the person is asked once. (Where they chose to keep those windows,
  // nothing is asked: the main window steps out of sight.)
  {
    const tauri = window.__TAURI__;
    const here = tauri.webviewWindow && tauri.webviewWindow.getCurrentWebviewWindow ? tauri.webviewWindow.getCurrentWebviewWindow() : null;
    const hear = (name, fn) => (here ? here.listen(name, fn) : tauri.event && tauri.event.listen ? tauri.event.listen(name, fn) : null);
    hear("main-close-asked", async (ev) => {
      const names = ev.payload || [];
      const n = names.length;
      const yes = await KagazAsk.confirm(n === 1 ? `Close Kagaz and end the session with ${names[0]}?` : `Close Kagaz and end ${n} sessions?`, "Close Kagaz",
        `${n === 1 ? "Everything open on it stays open there." : names.join(", ") + ". Everything open on them stays open there."} `
        + "Computers & phones settings can keep session windows open when this window closes.", "Stay");
      if (yes) invoke("remote_close_all").catch(() => {});
    });
  }

  function connectHere(id) {
    const d = data.devices.find((x) => x.id === id);
    if (!d || KagazRemoteView.isOpen()) return;
    viewId = id;
    KagazRemoteView.open(d, {
      canRemember,
      canRdp,
      show() {
        showView("viewer");
        KagazNav.push("remote-viewer", () => KagazRemoteView.close());
      },
      closed() {
        viewId = null;
        showView(openId ? "device" : "list");
        KagazNav.pop("remote-viewer");
        check(id);
      },
      // The picture setting chosen in the viewer is the device's from now on.
      settings(deviceId, settings) {
        const dev = data.devices.find((x) => x.id === deviceId);
        if (!dev) return;
        dev.settings = settings;
        invoke("remote_settings", { id: deviceId, settings, user: dev.user || null, shape: dev.shape || null })
          .then((out) => (data = out))
          .catch(() => {});
      },
    });
  }

  // ---------- the list ----------
  function tile(d) {
    const r = reachWords(d);
    const t = el("div", "home-tile has-actions" + (r.ok ? " on" : ""));
    const main = button("tile-main", [], () => openDevice(d.id), `${d.name}: ${r.text}`);
    const ic = el("span", "tile-icon");
    ic.innerHTML = KagazIcons.svg(shapeIcon(d));
    const text = el("span", "tile-text");
    text.append(el("span", "tile-name", d.name), stateLine(d, true));
    const chev = el("span", "home-chevron rc-chev");
    chev.innerHTML = KagazIcons.svg("forward");
    main.append(ic, text, chev);
    const acts = el("span", "tile-actions");
    const shown = windowsOf(d.id).length > 0;
    const go = shown
      ? button("tile-act", withIcon("computer", "Show window"), () => connect(d.id, true), `Show the window with ${d.name}'s screen`)
      : button("tile-act", withIcon("computer", "Connect"), () => connect(d.id), `Connect to ${d.name}`);
    const st = states.get(d.id);
    if (!shown) {
      go.disabled = !canConnect(d) || !!(st && !st.within);
      if (!canConnect(d)) go.title = whyNot(d);
      else if (go.disabled) go.title = `${d.name} is out of reach`;
    }
    acts.append(go);
    t.append(main, acts);
    return t;
  }

  function foundRow(iconName, name, where, off, onAdd) {
    const li = el("li", "rc-row" + (off ? " off" : ""));
    const ic = el("span", "tile-icon");
    ic.innerHTML = KagazIcons.svg(iconName);
    const text = el("span", "tile-text");
    text.append(el("span", "tile-name", name), el("span", "tile-sub", where));
    const add = button("", "Add", async () => {
      add.disabled = true;
      await onAdd();
      add.disabled = false;
    }, `Add ${name}`);
    li.append(ic, text, add);
    return li;
  }
  const peerIcon = (os) => (os === "android" || os === "ios" ? "mobile" : "computer");
  function nearRows() {
    if (!near) return [];
    const rows = near.network.map((n) =>
      foundRow(n.way.speaks === "vnc" ? "mobile" : "computer", n.name || `A ${n.words} device`, `${n.name ? n.words + " · " : ""}${n.address}`, false, () => addHost(n.address)));
    for (const p of near.peers) {
      rows.push(foundRow(peerIcon(p.os), p.name,
        `Tailscale${SYSTEM[p.os] ? " · " + SYSTEM[p.os] : ""} · ${p.online ? "online" : "offline"}`, !p.online, () => addHost(p.address, p.name)));
    }
    return rows;
  }

  function renderList() {
    $("remote-tiles").replaceChildren(...data.devices.map(tile));
    $("remote-empty").classList.toggle("hidden", data.devices.length > 0);
    $("remote-tiles").classList.toggle("hidden", data.devices.length === 0);
    const rows = nearRows();
    $("remote-found").replaceChildren(...rows);
    const note = $("remote-found-note");
    note.classList.toggle("working", looking);
    if (looking) note.textContent = "Looking over this network and in Tailscale…";
    else if (!near) note.textContent = "";
    else if (rows.length) note.textContent = `${rows.length} found. Add one to keep it here.`;
    else {
      note.textContent = "Nothing else that shows its screen on this network"
        + (near.tailscale === "on" ? " or in your Tailscale." : near.tailscale === "off" ? "; Tailscale is turned off on this device." : ".");
    }
    if (openId) renderDevice();
  }

  // ---------- checking and looking ----------
  // How the saved devices do now: all at once (about 2 s when one is off).
  async function check(only) {
    if (checking) return;
    checking = true;
    renderList();
    try {
      const out = await invoke("remote_check", { only: only || null });
      data = out.devices;
      for (const s of out.states) states.set(s.id, s);
    } catch (e) {
      $("remote-note").textContent = "Could not check your devices: " + e;
    }
    checking = false;
    renderList();
    if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
  }

  // What is around, not added yet.
  async function look() {
    if (looking) return;
    looking = true;
    $("remote-rescan").disabled = true;
    renderList();
    try {
      near = await invoke("remote_nearby");
      lastLook = Date.now();
    } catch (e) {
      $("remote-note").textContent = "Could not look around: " + e;
    }
    looking = false;
    $("remote-rescan").disabled = false;
    renderList();
  }

  function rescan() {
    $("remote-note").textContent = "";
    check();
    look();
  }

  // ---------- adding ----------
  // Ask the host what it speaks and remember it; its page opens next, to
  // name it and say who signs in.
  async function addHost(host, name, speaks) {
    const note = $(viewIs("add") ? "remote-add-note" : "remote-note");
    note.textContent = `Asking ${name || host} what it speaks…`;
    note.classList.add("working");
    try {
      let found = await invoke("remote_describe", { host });
      if (speaks) {
        // The person said what it speaks: taken at their word.
        found = { ...found, ways: [{ speaks, port: speaks === "vnc" ? 5900 : 3389 }], banners: [""] };
      }
      if (!found.ways.length) {
        note.textContent = `Nothing answered at ${found.host} for Remote Desktop, VNC or SSH. Check it is on, with remote access switched on. To add it anyway, choose what it speaks under "By its address".`;
        note.classList.remove("working");
        const words = note.textContent;
        if (!viewIs("add")) openAdd(host, name);
        else {
          $("remote-add-host").value = host;
          if (name && !$("remote-add-name").value) $("remote-add-name").value = name;
        }
        // Said next to the form that fixes it.
        $("remote-add-note").textContent = words;
        if (note !== $("remote-add-note")) note.textContent = "";
        $("remote-add-speaks").focus();
        return false;
      }
      const screen = found.ways.find((w) => w.speaks !== "ssh");
      const phone = found.system === "android" || found.system === "ios" || (!found.system && screen && screen.speaks === "vnc");
      const out = await invoke("remote_add", {
        new: { name: name || found.name, found, user: null, hotspot: false, shape: phone ? "phone" : "computer" },
      });
      data = out.devices;
      if (near) {
        near.network = near.network.filter((n) => n.address !== found.host);
        near.peers = near.peers.filter((p) => p.address !== found.host);
      }
      note.textContent = "";
      note.classList.remove("working");
      closeAdd();
      renderList();
      openDevice(out.id);
      check(out.id);
      if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
      return true;
    } catch (e) {
      note.textContent = "Could not add it: " + e;
      note.classList.remove("working");
      return false;
    }
  }

  const viewIs = (v) => !$(`remote-${v}-view`).classList.contains("hidden");
  function showView(v) {
    for (const x of ["list", "add", "device", "viewer", "settings", "share"]) $(`remote-${x}-view`).classList.toggle("hidden", x !== v);
    const pane = $("remote");
    const d = (v === "device" || v === "viewer") && data.devices.find((x) => x.id === (v === "viewer" ? viewId : openId));
    if (v === "add") pane.dataset.title = "Add a computer or phone";
    else if (v === "settings") pane.dataset.title = "Settings for computers & phones";
    else if (v === "share") pane.dataset.title = "This computer";
    else if (d) pane.dataset.title = d.name;
    else delete pane.dataset.title;
    $("main").scrollTop = 0;
  }

  function openAdd(host, name) {
    $("remote-add-note").textContent = "";
    $("remote-add-host").value = host || "";
    $("remote-add-name").value = name || "";
    $("remote-add-speaks").value = "";
    const net = near ? near.network : [];
    const peers = near ? near.peers : [];
    $("remote-add-network").replaceChildren(...net.map((n) =>
      foundRow(n.way.speaks === "vnc" ? "mobile" : "computer", n.name || `A ${n.words} device`, `${n.name ? n.words + " · " : ""}${n.address}`, false, () => addHost(n.address))));
    $("remote-add-network-note").textContent = looking ? "Looking…" : net.length ? "" : "Nothing that shows its screen was found on this network.";
    $("remote-add-peers").replaceChildren(...peers.map((p) =>
      foundRow(peerIcon(p.os), p.name, `${SYSTEM[p.os] || p.os || "Device"} · ${p.online ? "online" : "offline"}`, !p.online, () => addHost(p.address, p.name))));
    $("remote-add-peers-note").textContent = !near ? "" : near.tailscale === "none" ? "Tailscale is not on this device."
      : near.tailscale === "off" ? "Tailscale is turned off on this device."
        : peers.length ? "Read from Tailscale on this device. Nothing is changed there." : "Every device in your Tailscale is already added.";
    if (viewIs("add")) return;
    showView("add");
    KagazNav.push("remote-add", () => showView("list"));
    if (host) $("remote-add-host").focus();
  }
  function closeAdd() {
    if (viewIs("add")) {
      showView("list");
      KagazNav.pop("remote-add");
    }
  }

  // ---------- one device's page ----------
  function openDevice(id) {
    openId = id;
    renderDevice();
    if (viewIs("device")) return;
    showView("device");
    KagazNav.push("remote-device", () => {
      openId = null;
      leaveDevice();
    });
  }
  // Back from a device's page: to where it was opened from.
  function leaveDevice() {
    if (KagazNav.has("remote-settings")) {
      renderSettings();
      showView("settings");
    } else showView("list");
  }
  function closeDevice() {
    if (viewIs("device")) {
      openId = null;
      // The list first, then the level: the bar takes its title when the
      // level goes (a forgotten device's name stayed there).
      leaveDevice();
      KagazNav.pop("remote-device");
    }
  }

  // A device's settings changed here: its session window, if open, takes
  // them at once (session.js).
  function tellSettings(id) {
    const x = data.devices.find((y) => y.id === id);
    if (x && window.__TAURI__) window.__TAURI__.event.emit("remote-device-settings", { id, settings: x.settings || {}, from: "main" }).catch(() => {});
  }

  async function save(change, failed) {
    try {
      data = await change();
    } catch (e) {
      $("remote-device-note").textContent = `${failed}: ${e}`;
    }
    renderList();
    if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
  }

  function pathRow(d, p) {
    const st = states.get(d.id);
    const row = el("div", "set-row");
    const what = el("span");
    what.append(el("strong", "", p.kind === "gateway" ? "Its hotspot" : PATH_LABEL[p.kind] || p.kind));
    if (p.host) what.append(" · " + p.host);
    row.append(what);
    const mine = st ? st.paths.filter((x) => x.host === (p.host || "") && x.label === (PATH_LABEL[p.kind] || p.kind)) : [];
    const right = el("span", "rc-path-state");
    for (const x of mine) {
      const one = el("span", x.answers ? "rc-good" : "muted");
      if (x.answers) one.innerHTML = KagazIcons.svg("check");
      // Named when there is more than one line: its ways, or Remote Desktop
      // and VNC asked of a device that takes only SSH.
      const way = d.ways.length > 1 || mine.length > 1 ? x.way + ": " : "";
      one.append(x.answers ? `${way}answers${x.address ? " at " + x.address : ""}, ${x.millis} ms` : `${way}${x.trouble}`);
      right.append(one);
    }
    if (!mine.length) {
      // From afar is tried only on connecting, when nothing nearby answers.
      const idle = p.kind === "far" ? "tried when nothing nearby answers" : "not tried yet";
      right.append(el("span", "muted", checking && p.kind !== "far" ? "checking…" : p.last_ok ? `answered ${ago(p.last_ok)}` : idle));
    }
    row.append(right);
    // From afar comes and goes with the computer's own setting.
    if (d.paths.length > 1 && p.kind !== "far") {
      const name = p.host || "its hotspot";
      row.append(button("danger-text rc-remove", "Remove", async () => {
        if (!(await KagazAsk.confirm(`Remove ${name} from the ways to reach ${d.name}?`, "Remove", "The device stays; this address is no longer tried."))) return;
        save(() => invoke("remote_path_remove", { id: d.id, host: p.host || "" }), "Could not remove it");
      }, `Remove ${name}`));
    }
    return row;
  }
  const PATH_LABEL = { address: "Local network", name: "Name", tailscale: "Tailscale", gateway: "Its hotspot", far: "From afar" };

  function renderDevice() {
    const d = data.devices.find((x) => x.id === openId);
    const box = $("remote-device-view");
    if (!d) {
      if (viewIs("device")) closeDevice();
      return;
    }
    if (!viewId) $("remote").dataset.title = d.name;
    const head = el("div", "rc-head");
    const big = el("span", "rc-big");
    big.innerHTML = KagazIcons.svg(shapeIcon(d));
    const text = el("div", "rc-head-text");
    text.append(el("h3", "rc-name", d.name));
    const sub = el("p", "muted");
    const r = reachWords(d);
    sub.append(el("span", "rc-dot" + (r.ok ? " ok" : "")), [SYSTEM[d.system], d.ways.map(wayWords).join(", "), r.text].filter(Boolean).join(" · "));
    text.append(sub);
    const go = button("accent big", "Connect", () => connect(d.id));
    go.disabled = !canConnect(d);
    head.append(big, text, button("", "Rename", async () => {
      const name = await KagazAsk.name("Name the device", d.name, "Rename");
      if (!name || name === d.name) return;
      save(() => invoke("remote_rename", { id: d.id, name }), "Could not rename it");
    }), go);

    const soon = el("p", "muted small", canConnect(d) ? "" : whyNot(d));
    soon.classList.toggle("hidden", canConnect(d));

    // How its screen is shown and driven, and its sign-in: drawn as in a
    // session's own window (remote-settings.js). What is saved is read
    // back from the list, so that one change does not undo another.
    const now = () => data.devices.find((x) => x.id === d.id) || d;
    const { screen, mouse, keys, signIn, files } = KagazRemoteSettings.device(d, {
      rdp: overRdp(d),
      vnc: canConnect(d),
      canRemember,
      settings: (next) => save(() => invoke("remote_settings", { id: d.id, settings: next, user: now().user || null, shape: now().shape || null }), "Could not save it").then(() => tellSettings(d.id)),
      user: (name) => save(() => invoke("remote_settings", { id: d.id, settings: now().settings || {}, user: name, shape: now().shape || null }), "Could not save it"),
      remember: (on) => save(() => invoke("remote_remember", { id: d.id, on }), "Could not change it"),
      pickFolder: () => invoke("remote_pick_folder").catch(() => null),
    });

    const paths = el("section", "set-section");
    paths.append(el("h3", "", "Ways to reach it"), el("p", "muted", "All are tried at once; the first to answer is used."));
    for (const p of d.paths) paths.append(pathRow(d, p));
    const pathButtons = el("div", "rc-buttons start");
    pathButtons.append(button("", "Add an address", async () => {
      const host = await KagazAsk.name("Another address or name for it", "", "Add address");
      if (!host) return;
      await save(() => invoke("remote_path_add", { id: d.id, host }), "Could not add it");
      check(d.id);
    }));
    if (d.shape === "phone" && !d.paths.some((p) => p.kind === "gateway")) {
      pathButtons.append(button("", "It also shares its hotspot", async () => {
        await save(() => invoke("remote_path_add", { id: d.id, host: "" }), "Could not add it");
        check(d.id);
      }, "It is a phone that also shares its hotspot: reach it there too"));
    }
    const again = button("", "Check again", () => check(d.id));
    again.disabled = checking;
    pathButtons.append(again);
    paths.append(pathButtons);


    const about = el("section", "set-section");
    about.append(el("h3", "", "About it"));
    const shapeRow = el("div", "set-row");
    const shapeLabel = el("label", "", "It is a");
    shapeLabel.htmlFor = "remote-shape";
    const shape = el("select");
    shape.id = "remote-shape";
    for (const [v, w] of [["computer", "Computer"], ["laptop", "Laptop"], ["phone", "Phone"]]) {
      const o = el("option", "", w);
      o.value = v;
      o.selected = d.shape === v;
      shape.append(o);
    }
    shapeRow.append(shapeLabel, shape);
    shape.addEventListener("change", () => save(() => invoke("remote_settings", { id: d.id, settings: now().settings || {}, user: now().user || null, shape: shape.value }), "Could not save it"));
    about.append(shapeRow);

    const forget = el("p");
    forget.append(button("danger-text", "Forget this device", async () => {
      if (!(await KagazAsk.confirm(`Forget ${d.name}?`, "Forget device", "The device itself is not touched. Add brings it back."))) return;
      const id = d.id;
      states.delete(id);
      await save(() => invoke("remote_forget", { id }), "Could not forget it");
      closeDevice();
      lastLook = 0; // it may show under Found nearby again
    }));

    const note = el("p", "muted");
    note.id = "remote-device-note";
    note.setAttribute("role", "status");
    // Keep what is being typed when a check finishes under the person.
    const typing = document.activeElement && document.activeElement.id === "remote-user" ? document.activeElement.value : null;
    box.replaceChildren(head, soon, note, paths, screen, mouse, keys, files, signIn, about, forget);
    if (typing !== null) {
      const again = $("remote-user");
      again.value = typing;
      again.focus();
    }
  }
  const wayWords = (w) => {
    const name = { rdp: "Remote Desktop", vnc: "VNC", ssh: "SSH" }[w.speaks] || w.speaks;
    const usual = { rdp: 3389, vnc: 5900, ssh: 22 }[w.speaks];
    return w.port === usual ? name : `${name} on port ${w.port}`;
  };

  // ---------- this kind's own settings ----------
  // Behind the gear: what holds for every computer and phone, then a way
  // to each device's own (its page). Settings lists it too.
  function renderSettings() {
    const box = $("remote-settings-view");
    const w = { keep_open: false, ring: true, ...(data.windows || {}) };
    const note = el("p", "muted small");
    note.setAttribute("role", "status");
    const keep = async (change) => {
      const next = { ...w, ...change };
      try {
        data = await invoke("remote_windows_settings", { windows: next });
        note.textContent = "";
      } catch (e) {
        note.textContent = "Not saved: " + e;
      }
      renderSettings();
    };
    const win = KagazRemoteSettings.windows(w, keep);
    win.append(note);
    const each = el("section", "set-section");
    each.append(el("h3", "", "Each device"), el("p", "muted small", "Its keyboard, screen, sound and sign-in are on its own page."));
    for (const d of data.devices.slice().sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }))) {
      const r = el("div", "set-row set-door");
      r.tabIndex = 0;
      r.setAttribute("role", "button");
      const ic = el("span", "set-door-icon ic");
      ic.innerHTML = KagazIcons.svg(shapeIcon(d));
      const l = el("span", "set-label");
      l.append(el("span", "", d.name));
      const go = el("span", "set-door-go ic");
      go.innerHTML = KagazIcons.svg("forward");
      r.append(ic, l, go);
      const open = () => openDevice(d.id);
      r.addEventListener("click", open);
      r.addEventListener("keydown", (ev) => {
        if (ev.key === "Enter" || ev.key === " ") {
          ev.preventDefault();
          open();
        }
      });
      each.append(r);
    }
    if (!data.devices.length) each.append(el("p", "muted", "No computers or phones yet."));
    const parts = [win, each];
    if (share && share.supported) {
      const mine = el("section", "set-section");
      mine.append(el("h3", "", "This computer"));
      const r = el("div", "set-row set-door");
      r.tabIndex = 0;
      r.setAttribute("role", "button");
      const ic = el("span", "set-door-icon ic");
      ic.innerHTML = KagazIcons.svg("screens");
      const l = el("span", "set-label");
      l.append(el("span", "", "Share this computer"), el("span", "muted small", share.settings && share.settings.on ? "On" : "Off"));
      const go = el("span", "set-door-go ic");
      go.innerHTML = KagazIcons.svg("forward");
      r.append(ic, l, go);
      r.addEventListener("click", openShare);
      mine.append(r);
      parts.unshift(mine);
    }
    box.replaceChildren(...parts);
  }
  async function openSettings() {
    try {
      data = (await invoke("remote_saved")) || data;
    } catch (e) {}
    renderSettings();
    if (viewIs("settings")) return;
    showView("settings");
    KagazNav.push("remote-settings", () => showView("list"));
  }
  $("remote-gear").addEventListener("click", openSettings);

  // ---------- the page ----------
  async function show() {
    invoke("remote_abilities").then((can) => {
      const changed = canRdp !== !!can.rdp;
      canRemember = !!can.remember;
      canRdp = !!can.rdp;
      if (changed) renderList();
    }).catch(() => {});
    try {
      data = (await invoke("remote_saved")) || data;
    } catch (e) {
      $("remote-note").textContent = "Could not read your devices: " + e;
    }
    renderList();
    check();
    if (Date.now() - lastLook > 2 * 60 * 1000) look();
  }

  // From outside (`kagaz remote open`): straight to a device's screen,
  // named by its id, its name or the start of its name.
  async function connectTo(which) {
    try {
      const can = await invoke("remote_abilities");
      canRemember = !!can.remember;
      canRdp = !!can.rdp;
    } catch (e) {}
    try {
      data = (await invoke("remote_saved")) || data;
    } catch (e) {}
    renderList();
    const w = which.trim().toLowerCase();
    const d = data.devices.find((x) => x.id === which) || data.devices.find((x) => x.name.toLowerCase() === w)
      || data.devices.find((x) => x.name.toLowerCase().startsWith(w));
    if (!d) $("remote-note").textContent = `There is no device called "${which}" here.`;
    else if (!canConnect(d)) openDevice(d.id);
    else connect(d.id);
  }

  // From Home: a device's own page.
  function open(id) {
    if (data.devices.some((d) => d.id === id)) openDevice(id);
  }

  // ---------- this computer, shared (share.js) ----------
  let share = null;
  function openShare() {
    if (!viewIs("share")) {
      const from = viewIs("settings") ? "settings" : viewIs("add") ? "add" : "list";
      showView("share");
      KagazNav.push("remote-share", () => showView(from));
    }
    KagazShare.open();
  }
  // Where this computer can share: Add offers it, the gear's page has a
  // way to it, and while it is on the list says so.
  function shareChanged(st) {
    share = st;
    const can = !!(st && st.supported);
    $("remote-add-share").classList.toggle("hidden", !can);
    const rowBox = $("remote-share-row");
    rowBox.classList.toggle("hidden", !(st && st.settings && st.settings.on));
    if (st && st.settings && st.settings.on) {
      const r = el("div", "set-row set-door share-row");
      r.tabIndex = 0;
      r.setAttribute("role", "button");
      const ic = el("span", "set-door-icon ic");
      ic.innerHTML = KagazIcons.svg("screens");
      const l = el("span", "set-label");
      const words = st.session ? `${st.session.name} is connected now` : `${(st.devices || []).length} paired; nobody connected`;
      l.append(el("span", "", "This computer is shared"), el("span", "muted small", st.problem || words));
      const go = el("span", "set-door-go ic");
      go.innerHTML = KagazIcons.svg("forward");
      r.append(ic, l, go);
      r.addEventListener("click", openShare);
      rowBox.replaceChildren(r);
    }
    if (viewIs("settings")) renderSettings();
    // Turned on or off: Home and the sidebar list the kind accordingly.
    const on = !!(st && st.settings && st.settings.on);
    if (on !== sharedBefore && typeof KagazHome !== "undefined") KagazHome.devicesChanged();
    sharedBefore = on;
  }
  let sharedBefore = false;
  $("remote-add-share-go").addEventListener("click", openShare);
  KagazShare.status().then(shareChanged);

  // ---------- pairing with a computer that runs Kagaz ----------
  function pairShown(check) {
    $("remote-pair-form").classList.toggle("hidden", check);
    $("remote-pair-check").classList.toggle("hidden", !check);
  }
  $("remote-pair-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const host = $("remote-pair-host").value.trim();
    const code = $("remote-pair-code").value.replace(/\D/g, "");
    const note = $("remote-pair-note");
    if (!host) return $("remote-pair-host").focus();
    if (code.length !== 6) {
      note.textContent = "The code is six digits.";
      return $("remote-pair-code").focus();
    }
    const go = $("remote-pair-go");
    go.disabled = true;
    note.textContent = `Reaching ${host}…`;
    try {
      const said = await invoke("remote_pair_begin", { host, port: Number($("remote-pair-port").value) || 3390, code });
      note.textContent = "";
      $("remote-pair-said").textContent = `${said.name} shows these words:`;
      $("remote-pair-words").textContent = said.words;
      pairShown(true);
      $("remote-pair-yes").focus();
    } catch (e) {
      note.textContent = "Not paired: " + e;
    } finally {
      go.disabled = false;
    }
  });
  async function pairFinish(yes) {
    const note = $("remote-pair-note");
    $("remote-pair-yes").disabled = $("remote-pair-no").disabled = true;
    note.textContent = yes ? "Waiting for the person there to confirm too…" : "";
    try {
      const done = await invoke("remote_pair_finish", { yes });
      data = done.devices;
      $("remote-pair-code").value = "";
      note.textContent = "";
      pairShown(false);
      renderList();
      if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
      openDevice(done.id);
    } catch (e) {
      pairShown(false);
      note.textContent = yes ? "Not paired: " + e : "Not paired.";
    } finally {
      $("remote-pair-yes").disabled = $("remote-pair-no").disabled = false;
    }
  }
  $("remote-pair-yes").addEventListener("click", () => pairFinish(true));
  $("remote-pair-no").addEventListener("click", () => pairFinish(false));

  $("remote-rescan").addEventListener("click", rescan);
  $("remote-add").addEventListener("click", () => openAdd());
  $("remote-empty-add").addEventListener("click", () => openAdd());
  $("remote-add-form").addEventListener("submit", (ev) => {
    ev.preventDefault();
    const host = $("remote-add-host").value.trim();
    if (!host) {
      $("remote-add-note").textContent = "Type the device's address or name first.";
      $("remote-add-host").focus();
      return;
    }
    const go = $("remote-add-go");
    go.disabled = true;
    addHost(host, $("remote-add-name").value.trim() || null, $("remote-add-speaks").value || null).finally(() => (go.disabled = false));
  });

  return { show, rescan, open, openAdd, openSettings, openShare, shareChanged, connectTo, homeWords, saved: () => data.devices, shapeIcon, within: (id) => !!(states.get(id) && states.get(id).within) };
})();
