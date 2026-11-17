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
    line.append(el("span", "rc-dot" + (r.ok ? " ok" : "")));
    const sys = SYSTEM[d.system];
    if (withSystem && sys) line.append(el("span", "rc-what", sys + " · "));
    line.append(r.text);
    return line;
  }

  // ---------- connecting ----------
  // Can a session be opened with it from here?
  const canConnect = (d) => d.ways.some((w) => w.speaks === "vnc" || (canRdp && w.speaks === "rdp"));
  const overRdp = (d) => canRdp && d.ways.some((w) => w.speaks === "rdp");
  const NOT_YET = "Remote Desktop sessions are not on this kind of device yet. A device that speaks VNC can be connected to.";

  function connect(id) {
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
    const go = button("tile-act", withIcon("computer", "Connect"), () => connect(d.id), `Connect to ${d.name}`);
    const st = states.get(d.id);
    go.disabled = !canConnect(d) || !!(st && !st.within);
    if (!canConnect(d)) go.title = NOT_YET;
    else if (go.disabled) go.title = `${d.name} is out of reach`;
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
        note.textContent = `Nothing answered at ${found.host} for Remote Desktop, VNC or SSH. Check that it is on and that remote access is switched on there. To add it anyway, choose what it speaks under "By its address".`;
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
    for (const x of ["list", "add", "device", "viewer"]) $(`remote-${x}-view`).classList.toggle("hidden", x !== v);
    const pane = $("remote");
    const d = (v === "device" || v === "viewer") && data.devices.find((x) => x.id === (v === "viewer" ? viewId : openId));
    if (v === "add") pane.dataset.title = "Add a computer or phone";
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
      showView("list");
    });
  }
  function closeDevice() {
    if (viewIs("device")) {
      openId = null;
      showView("list");
      KagazNav.pop("remote-device");
    }
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
      const way = d.ways.length > 1 ? x.way + ": " : "";
      one.append(x.answers ? `${way}answers${x.address ? " at " + x.address : ""}, ${x.millis} ms` : `${way}${x.trouble}`);
      right.append(one);
    }
    if (!mine.length) right.append(el("span", "muted", checking ? "checking…" : p.last_ok ? `answered ${ago(p.last_ok)}` : "not tried yet"));
    row.append(right);
    if (d.paths.length > 1) {
      const name = p.host || "its hotspot";
      row.append(button("danger-text rc-remove", "Remove", async () => {
        if (!(await KagazAsk.confirm(`Remove ${name} from the ways to reach ${d.name}?`, "Remove", "The device stays; this address is no longer tried."))) return;
        save(() => invoke("remote_path_remove", { id: d.id, host: p.host || "" }), "Could not remove it");
      }, `Remove ${name}`));
    }
    return row;
  }
  const PATH_LABEL = { address: "Local network", name: "Name", tailscale: "Tailscale", gateway: "Its hotspot" };

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

    const soon = el("p", "muted small", canConnect(d) ? "" : NOT_YET);
    soon.classList.toggle("hidden", canConnect(d));

    // How its screen is shown and driven: kept with the device.
    const settings = { ...(d.settings || {}) };
    const keepSettings = () => save(() => invoke("remote_settings", { id: d.id, settings, user: d.user || null, shape: d.shape || null }), "Could not save it");
    const screen = el("section", "set-section");
    screen.append(el("h3", "", "Picture"));
    const picRow = el("div", "set-row");
    const picLabel = el("label", "", "Picture");
    picLabel.htmlFor = "remote-picture";
    const slider = el("span", "rc-slider");
    const range = el("input");
    range.id = "remote-picture";
    range.type = "range";
    range.min = 0;
    range.max = 10;
    range.value = settings.picture == null ? 0 : settings.picture;
    range.disabled = settings.picture == null;
    slider.append(el("span", "", "Sharper"), range, el("span", "", "Smoother"));
    picRow.append(picLabel, slider);
    const autoRow = el("div", "set-row");
    const autoLabel = el("label", "switch");
    const auto = el("input");
    auto.type = "checkbox";
    auto.checked = settings.picture == null;
    autoLabel.append(auto, " Adjust by itself to the connection");
    autoRow.append(autoLabel);
    const keepPicture = () => {
      if (auto.checked) delete settings.picture;
      else settings.picture = Number(range.value);
      keepSettings();
    };
    range.addEventListener("change", keepPicture);
    auto.addEventListener("change", keepPicture);
    if (overRdp(d)) {
      // Remote Desktop draws its screen for the viewer, and suits the
      // picture to the connection by itself.
      screen.replaceChildren(el("h3", "", "Screen and sound"));
      const sizeRow = el("div", "set-row");
      const sizeLabel = el("label", "", "Size");
      sizeLabel.htmlFor = "remote-size";
      const size = el("select");
      size.id = "remote-size";
      for (const [v, w] of [["", "Fit this window, sharp"], ["own", "A fixed size (1920 × 1080), scaled to the window"]]) {
        const o = el("option", "", w);
        o.value = v;
        o.selected = (settings.own_size ? "own" : "") === v;
        size.append(o);
      }
      size.addEventListener("change", () => {
        if (size.value) settings.own_size = true;
        else delete settings.own_size;
        keepSettings();
      });
      sizeRow.append(sizeLabel, size);
      // Remote Desktop's newer picture (H.264 and its companions) is
      // used unless it comes out wrong on this computer.
      const olderRow = el("div", "set-row");
      const olderLabel = el("label", "switch wrap");
      const older = el("input");
      older.type = "checkbox";
      older.checked = !!settings.older_picture;
      olderLabel.append(older, " Use the older picture", el("small", "muted", "Slower and heavier on the connection. For a computer whose screen comes out wrong or stays black otherwise. Takes effect at the next connection."));
      older.addEventListener("change", () => {
        if (older.checked) settings.older_picture = true;
        else delete settings.older_picture;
        keepSettings();
      });
      olderRow.append(olderLabel);
      // Its sound plays here unless turned off for this computer.
      const soundRow = el("div", "set-row");
      const soundLabel = el("label", "switch wrap");
      const sound = el("input");
      sound.type = "checkbox";
      sound.checked = !settings.mute;
      soundLabel.append(sound, " Play its sound here", el("small", "muted", "Instead of on the computer itself. The speaker button over its screen turns it off for the moment. Takes effect at the next connection."));
      sound.addEventListener("change", () => {
        if (sound.checked) delete settings.mute;
        else settings.mute = true;
        keepSettings();
      });
      soundRow.append(soundLabel);
      screen.append(sizeRow, olderRow, soundRow, el("p", "muted small", "The computer suits its picture to the connection by itself, and shows one screen, made for this window."));
    } else {
      screen.append(picRow, autoRow);
      if (canConnect(d)) screen.append(el("p", "muted small", "Over VNC the device's whole screen is shown at its own size, fitted to the window. Sound and files need Remote Desktop."));
    }

    const mouse = el("section", "set-section");
    mouse.append(el("h3", "", "Mouse"));
    const swapRow = el("div", "set-row");
    const swapLabel = el("label", "switch wrap");
    const swap = el("input");
    swap.type = "checkbox";
    swap.checked = !!settings.swapped_buttons;
    swapLabel.append(swap, " Keep left click on the left", el("small", "muted", "For a device whose mouse buttons are swapped: a left click here stays a left click there."));
    swap.addEventListener("change", () => {
      if (swap.checked) settings.swapped_buttons = true;
      else delete settings.swapped_buttons;
      keepSettings();
    });
    swapRow.append(swapLabel);
    mouse.append(swapRow);

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

    const signIn = el("section", "set-section");
    signIn.append(el("h3", "", "Sign-in"));

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
    const userRow = el("div", "set-row");
    const userLabel = el("label", "", "Sign in as");
    userLabel.htmlFor = "remote-user";
    const user = el("input");
    user.id = "remote-user";
    user.type = "text";
    user.autocomplete = "username";
    user.placeholder = "the account's user name";
    user.value = d.user || "";
    userRow.append(userLabel, user);
    const keep = () => save(() => invoke("remote_settings", { id: d.id, settings: d.settings || {}, user: user.value, shape: shape.value }), "Could not save it");
    shape.addEventListener("change", keep);
    user.addEventListener("change", keep);
    about.append(shapeRow);
    signIn.append(userRow);
    if (canRemember) {
      const keepRow = el("div", "set-row");
      const keepLabel = el("label", "switch wrap");
      const remember = el("input");
      remember.type = "checkbox";
      remember.checked = !!d.remember;
      keepLabel.append(remember, " Remember the password", el("small", "muted", "In this computer's password store, once the device has accepted it. Turning this off forgets the one that is kept."));
      remember.addEventListener("change", () => save(() => invoke("remote_remember", { id: d.id, on: remember.checked }), "Could not change it"));
      keepRow.append(keepLabel);
      signIn.append(keepRow);
    }

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
    box.replaceChildren(head, soon, note, paths, screen, mouse, signIn, about, forget);
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

  return { show, rescan, open, openAdd, connectTo, homeWords, saved: () => data.devices, shapeIcon, within: (id) => !!(states.get(id) && states.get(id).within) };
})();
