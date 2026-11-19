// A session's own window (session.html; remote_windows.rs opens it): the
// device this window was opened for, its screen through the viewer
// (remote-view.js), and the little of Kagaz that belongs in it: a way to
// the main window, and a call at the door.

(async () => {
  const root = document.documentElement;
  const me = window.__KAGAZ_SESSION__ || {};
  const { listen, emit } = window.__TAURI__.event;

  // The tools sit in the bar where it has room, else under the screen.
  const fit = () => (root.dataset.layout = window.innerWidth < 550 ? "bar" : "sidebar");
  fit();
  window.addEventListener("resize", fit);

  $("sw-main").addEventListener("click", () => invoke("remote_main_show").catch(() => {}));

  let can = {}, data = { devices: [] };
  try {
    can = await invoke("remote_abilities");
  } catch (e) {}
  try {
    data = (await invoke("remote_saved")) || data;
  } catch (e) {}
  const device = data.devices.find((d) => d.id === me.device);
  if (!device) {
    // Forgotten in the main window while this one was opening.
    $("bar-title").textContent = "Kagaz";
    const gone = document.createElement("p");
    gone.className = "muted";
    gone.textContent = "This device is no longer in Kagaz. Close this window.";
    $("rv-stage").append(gone);
    return;
  }
  $("bar-title").textContent = device.name;
  document.title = `${device.name} · Kagaz`;
  invoke("page_log", { line: `remote: ${me.label || "a window"} opened for a session` }).catch(() => {});

  KagazRemoteView.open(device, {
    // The window is the session's: leaving the viewer closes it.
    window: true,
    canRemember: !!can.remember,
    canRdp: !!can.rdp,
    closed: () => invoke("remote_close_window").catch(() => {}),
    // Its addresses are edited on its page, in the main window.
    edit: () => emit("remote-edit-device", device.id).then(() => invoke("remote_main_show")).catch(() => {}),
    // What is chosen in the viewer is the device's from now on.
    settings(id, settings) {
      keepSettings(settings, false);
    },
  });

  // ---------- settings behind the gear ----------
  // This device's own (as on its page in the main window) and every
  // session window's, without leaving the screen. Where this computer's
  // keys go changes at once; size, picture and sound at the next
  // connection, as their notes say.
  const tell = (what) => emit("remote-device-settings", { id: device.id, settings: device.settings || {}, from: me.label || "session", ...what }).catch(() => {});
  function keepSettings(settings, toViewer) {
    device.settings = { ...settings };
    if (toViewer) KagazRemoteView.settings(device.settings);
    invoke("remote_settings", { id: device.id, settings: device.settings, user: device.user || null, shape: device.shape || null })
      .then(() => tell({}))
      .catch((e) => panelNote("Not saved: " + e));
    if (panelOpen()) drawPanel();
  }
  const panel = $("sw-panel");
  const gear = $("sw-settings");
  const panelOpen = () => !panel.classList.contains("hidden");
  let note = null;
  const panelNote = (text) => note && (note.textContent = text);
  function drawPanel() {
    const body = $("sw-panel-body");
    const keepScroll = body.scrollTop;
    note = document.createElement("p");
    note.className = "muted small";
    note.setAttribute("role", "status");
    const { screen, mouse, keys, signIn, files } = KagazRemoteSettings.device(device, {
      rdp: !!can.rdp && device.ways.some((w) => w.speaks === "rdp"),
      vnc: device.ways.some((w) => w.speaks === "vnc"),
      canRemember: !!can.remember,
      settings: (next) => keepSettings(next, true),
      user: (name) => {
        device.user = name || null;
        invoke("remote_settings", { id: device.id, settings: device.settings || {}, user: name, shape: device.shape || null })
          .then(() => tell({})).catch((e) => panelNote("Not saved: " + e));
      },
      pickFolder: () => invoke("remote_pick_folder").catch(() => null),
      remember: (on) => {
        invoke("remote_remember", { id: device.id, on }).then((saved) => {
          const now = saved && saved.devices && saved.devices.find((d) => d.id === device.id);
          if (now) device.remember = now.remember;
          tell({});
        }).catch((e) => panelNote("Not changed: " + e));
      },
    });
    // Which settings are this device's alone, and which every window's.
    const head = document.createElement("div");
    head.className = "sw-panel-device";
    const name = document.createElement("strong");
    name.textContent = device.name;
    const only = document.createElement("span");
    only.className = "muted small";
    only.textContent = "Kept for this device only";
    head.append(name, only);
    const every = document.createElement("div");
    every.className = "sw-panel-device";
    const all = document.createElement("span");
    all.className = "muted small";
    all.textContent = "Below: for every session window";
    every.append(all);
    const windows = KagazRemoteSettings.windows({ keep_open: false, ring: true, ...(data.windows || {}) }, async (change) => {
      try {
        data = (await invoke("remote_windows_settings", { windows: { keep_open: false, ring: true, ...(data.windows || {}), ...change } })) || data;
        panelNote("");
      } catch (e) {
        panelNote("Not saved: " + e);
      }
      drawPanel();
    });
    // Its addresses, what it is, forgetting it: on its page.
    const more = document.createElement("p");
    const page = document.createElement("button");
    page.type = "button";
    page.className = "link";
    page.textContent = "Its addresses and more, in Kagaz's main window";
    page.addEventListener("click", () => emit("remote-edit-device", device.id).then(() => invoke("remote_main_show")).catch(() => {}));
    more.append(page);
    body.replaceChildren(head, note, keys, screen, mouse, files, signIn, every, windows, more);
    if (typeof KagazIcons !== "undefined") KagazIcons.fill(body);
    body.scrollTop = keepScroll;
  }
  function showPanel(on) {
    panel.classList.toggle("hidden", !on);
    gear.setAttribute("aria-expanded", on ? "true" : "false");
    gear.classList.toggle("on", on);
    if (on) {
      drawPanel();
      $("sw-panel-close").focus();
    } else {
      $("rv-stage").focus();
    }
  }
  gear.addEventListener("click", () => showPanel(!panelOpen()));
  $("sw-panel-close").addEventListener("click", () => showPanel(false));
  panel.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      ev.stopPropagation();
      showPanel(false);
    }
  });
  // Changed on its page in the main window, or in another window.
  listen("remote-device-settings", (ev) => {
    const p = ev.payload || {};
    if (p.id !== device.id || p.from === (me.label || "session")) return;
    invoke("remote_saved").then((saved) => {
      const now = saved && saved.devices.find((d) => d.id === device.id);
      if (!now) return;
      Object.assign(device, { settings: now.settings || {}, user: now.user, remember: now.remember });
      KagazRemoteView.settings(device.settings);
      if (panelOpen()) drawPanel();
    }).catch(() => {});
  });

  // ---------- a call at the door ----------
  // The main window says when one rings and when it is over (shell.js);
  // Answer brings that window forward with the call, Decline is only this
  // device's. Not shown where the person turned it off (Computers &
  // phones' settings).
  let ringShown = data.windows ? data.windows.ring !== false : true;
  let ringing = null;
  function showRing() {
    const on = !!ringing && ringShown;
    $("urgent").classList.toggle("hidden", !on);
    if (!on) return;
    $("urgent-title").textContent = ringing.title;
    $("urgent-sub").textContent = ringing.sub || "";
  }
  listen("ring-strip", (ev) => {
    ringing = ev.payload;
    showRing();
  });
  listen("remote-windows-settings", (ev) => {
    ringShown = ev.payload.ring !== false;
    data.windows = ev.payload;
    showRing();
    if (panelOpen()) drawPanel();
  });
  $("urgent-answer").addEventListener("click", () => {
    emit("ring-strip-do", "answer");
    invoke("remote_main_show").catch(() => {});
  });
  $("urgent-decline").addEventListener("click", () => emit("ring-strip-do", "decline"));
  // A call already ringing when this window opened.
  emit("ring-strip-ask", null);
})();
