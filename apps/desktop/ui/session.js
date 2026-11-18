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
      device.settings = settings;
      invoke("remote_settings", { id, settings, user: device.user || null, shape: device.shape || null }).catch(() => {});
    },
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
    showRing();
  });
  $("urgent-answer").addEventListener("click", () => {
    emit("ring-strip-do", "answer");
    invoke("remote_main_show").catch(() => {});
  });
  $("urgent-decline").addEventListener("click", () => emit("ring-strip-do", "decline"));
  // A call already ringing when this window opened.
  emit("ring-strip-ask", null);
})();
