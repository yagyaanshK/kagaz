// The Kagaz window. No framework: a device list, an "About" pane that shows
// kagaz explain, and a "Scan" pane that mirrors kagaz scan.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

let devices = [];
let current = null;

// ---------- device list ----------

function title(d) {
  return d.name || d.model || "Unknown device";
}

function where(d) {
  if (d.addresses && d.addresses.length) return d.addresses[0] + (d.hostname ? "  " + d.hostname : "");
  if (d.usb) return `USB ${hex(d.usb.vendor_id)}:${hex(d.usb.product_id)}`;
  return "";
}

function hex(n) {
  return n.toString(16).padStart(4, "0");
}

function protocols(d) {
  return (d.services || []).map((s) => s.protocol);
}

function has(d, ...names) {
  const p = protocols(d);
  return names.some((n) => p.includes(n));
}

function capLine(d) {
  const print = has(d, "Ipp", "Ipps", "WsdPrint", "IppUsb")
    ? ["cap-ok", "prints without a driver"]
    : has(d, "PdlDataStream", "Lpd", "UsbPrinter")
      ? ["cap-warn", "printing needs a driver"]
      : has(d, "Snmp") && protocols(d).length === 1
        ? ["cap-warn", "found over SNMP only"]
        : ["cap-bad", "no printing advertised"];
  const scan = has(d, "Escl", "Escls", "WsdScan")
    ? ["cap-ok", "scans without a driver"]
    : has(d, "IppUsb")
      ? ["cap-warn", "may scan over IPP-USB"]
      : has(d, "SaneNet")
        ? ["cap-warn", "scanning needs the vendor's driver"]
        : null;
  return [print, scan].filter(Boolean);
}

function renderList() {
  const ul = $("devices");
  ul.innerHTML = "";
  devices.forEach((d, i) => {
    const li = document.createElement("li");
    li.dataset.index = i;
    if (current === i) li.classList.add("active");
    const caps = capLine(d)
      .map(([cls, text]) => `<span class="${cls}">${text}</span>`)
      .join(" · ");
    li.innerHTML = `<div class="name"></div><div class="where"></div><div class="caps">${caps}</div>`;
    li.querySelector(".name").textContent = title(d);
    li.querySelector(".where").textContent = where(d);
    li.addEventListener("click", () => select(i));
    ul.appendChild(li);
  });
}

async function rescan() {
  $("rescan").disabled = true;
  $("search-note").textContent = "Looking for printers and scanners…";
  $("search-note").classList.remove("hidden");
  try {
    devices = await invoke("discover", { timeout: 3 });
    $("search-note").textContent = devices.length
      ? `${devices.length} found`
      : "Nothing answered. Is the device on and on the same network?";
  } catch (e) {
    devices = [];
    $("search-note").textContent = "Could not search: " + e;
  }
  $("rescan").disabled = false;
  if (current !== null && current >= devices.length) current = null;
  renderList();
}

// ---------- detail ----------

function canScan(d) {
  return has(d, "Escl", "WsdScan", "SaneNet");
}

async function select(i) {
  current = i;
  const d = devices[i];
  renderList();
  $("empty").classList.add("hidden");
  $("cameras").classList.add("hidden");
  $("detail").classList.remove("hidden");
  $("d-title").textContent = title(d);
  $("d-sub").textContent = [d.manufacturer, d.model, where(d)].filter(Boolean).join(" · ");
  $("scan-log").innerHTML = "";
  $("scan-state").textContent = "";
  showTab("about");
  const scannable = canScan(d);
  $("scan-unavailable").classList.toggle("hidden", scannable);
  $("scan-form").classList.toggle("hidden", !scannable);
  if (!scannable) {
    $("scan-unavailable-text").textContent = has(d, "SaneNet")
      ? "This device was seen only with its vendor's own scanning protocol, which needs the vendor's driver (see About). No driverless scan service (eSCL or WSD) answered this time."
      : "No driverless scan service (eSCL or WSD) was seen on this device, so Kagaz cannot scan from it.";
  }

  $("button-state").textContent = "";
  loadButtonSettings(d);
  if (canScan(d)) loadCapabilities(d, i);
  $("about-loading").classList.remove("hidden");
  $("about-body").classList.add("hidden");
  try {
    const e = await invoke("explain_device", { device: d });
    if (current !== i) return; // moved on
    fillList("about-print-details", e.print.details);
    fillList("about-scan-details", e.scan.details);
    fillList("about-notes", e.notes);
    $("about-notes-head").classList.toggle("hidden", e.notes.length === 0);
    $("about-print-summary").textContent = e.print.summary;
    $("about-scan-summary").textContent = e.scan.summary;
    $("about-print-badge").innerHTML = Pictures.verdict(e.print.status);
    $("about-scan-badge").innerHTML = Pictures.verdict(e.scan.status);
    $("about-loading").classList.add("hidden");
    $("about-body").classList.remove("hidden");
  } catch (err) {
    $("about-loading").textContent = "Could not work it out: " + err;
  }
}

function fillList(id, items) {
  const ul = $(id);
  ul.innerHTML = "";
  for (const text of items) {
    const li = document.createElement("li");
    li.textContent = text;
    ul.appendChild(li);
  }
}

function showTab(name) {
  document.querySelectorAll(".tab").forEach((b) => b.classList.toggle("active", b.dataset.tab === name));
  $("tab-about").classList.toggle("hidden", name !== "about");
  $("tab-scan").classList.toggle("hidden", name !== "scan");
}

// ---------- the printer's own Scan button (Brother scan-key tool) ----------

const RESOLUTIONS = [100, 150, 200, 300, 400, 600, 1200];
const SIZES = ["A4", "Letter", "Legal", "A5", "A6", "A3", "MAX"];

function renderButtonSettings(list) {
  const tbody = $("button-table").querySelector("tbody");
  tbody.innerHTML = "";
  for (const a of list) {
    const tr = document.createElement("tr");
    const opts = (values, current) =>
      values.map((v) => `<option value="${v}" ${String(v) === String(current) ? "selected" : ""}>${v}</option>`).join("");
    const origin = { User: "your setting", System: "Brother's default", BuiltIn: "built-in default" }[a.origin] || a.origin;
    tr.innerHTML = `<td>${a.action}</td>
      <td><select data-key="resolution">${opts(RESOLUTIONS.includes(a.resolution) ? RESOLUTIONS : [a.resolution, ...RESOLUTIONS], a.resolution)}</select> dpi</td>
      <td><select data-key="size">${opts(SIZES.includes(a.size) ? SIZES : [a.size, ...SIZES], a.size)}</select></td>
      <td><select data-key="duplex"><option value="false" ${!a.duplex ? "selected" : ""}>no</option><option value="true" ${a.duplex ? "selected" : ""}>yes</option></select></td>
      <td class="muted">${origin}</td>
      <td>${a.origin === "User" ? '<button type="button" data-reset>Brother default</button>' : ""}</td>`;
    tr.querySelectorAll("select").forEach((sel) =>
      sel.addEventListener("change", async () => {
        const get = (k) => tr.querySelector(`select[data-key="${k}"]`).value;
        try {
          const updated = await invoke("button_settings_set", {
            change: { action: a.action, resolution: Number(get("resolution")), size: get("size"), duplex: get("duplex") === "true" },
          });
          $("button-state").textContent = `Saved for "${a.action}" in your ~/.brscan-skey copy.`;
          renderButtonSettings(updated);
        } catch (e) {
          $("button-state").textContent = "Could not save: " + e;
        }
      })
    );
    const reset = tr.querySelector("button[data-reset]");
    if (reset)
      reset.addEventListener("click", async () => {
        try {
          const updated = await invoke("button_settings_reset", { action: a.action });
          $("button-state").textContent = `"${a.action}" is back to Brother's default.`;
          renderButtonSettings(updated);
        } catch (e) {
          $("button-state").textContent = "Could not reset: " + e;
        }
      });
    tbody.appendChild(tr);
  }
}

async function loadButtonSettings(d) {
  const panel = $("button-panel");
  const brother = (d.manufacturer || "").toLowerCase() === "brother";
  let list = null;
  if (brother) {
    try { list = await invoke("button_settings"); } catch (_) { list = null; }
  }
  panel.classList.toggle("hidden", !list);
  if (list) renderButtonSettings(list);
}

// ---------- what this scanner offers ----------

function dpiLabel(dpi) {
  const hint = dpi <= 150 ? "quick" : dpi <= 300 ? "documents" : "photos, slow";
  return `${dpi} dpi (${hint})`;
}

let caps = null; // what the selected device offers, per engine

function engineCaps() {
  if (!caps) return null;
  const engine = $("scan-form").querySelector('select[name="engine"]').value;
  return engine === "Driver" ? caps.driver : caps.driverless;
}

// The resolution list always shows every value either engine offers; the
// note says which engine takes the chosen one. The choice stays with you.
function renderResolutions() {
  const dpiSel = $("scan-form").querySelector('select[name="dpi"]');
  const srcSel = $("scan-form").querySelector('select[name="source"]');
  const engSel = $("scan-form").querySelector('select[name="engine"]');
  if (!caps) return;
  const dl = caps.driverless ? caps.driverless.resolutions : [];
  const dr = caps.driver ? caps.driver.resolutions : [];
  const all = [...new Set([...dl, ...dr])].sort((a, b) => a - b);
  const current = Number(dpiSel.value) || 300;
  const chosen = all.includes(current) ? current : all.includes(300) ? 300 : all[all.length - 1];
  dpiSel.innerHTML = all
    .map((r) => {
      const who = dl.includes(r) && dr.includes(r) ? "" : dl.includes(r) ? " · no driver" : " · driver";
      return `<option value="${r}" ${r === chosen ? "selected" : ""}>${dpiLabel(r)}${who}</option>`;
    })
    .join("");
  for (const opt of engSel.options) {
    opt.disabled = (opt.value === "Driver" && !caps.driver) || (opt.value === "Driverless" && !caps.driverless);
  }
  if (engSel.selectedOptions[0]?.disabled) engSel.value = caps.driverless ? "Driverless" : "Driver";
  const ec = engineCaps();
  for (const opt of srcSel.options) {
    const needsFeeder = opt.value === "Feeder" || opt.value === "FeederDuplex";
    opt.disabled = !ec || (needsFeeder && !ec.feeder) || (opt.value === "Glass" && !ec.glass) || (opt.value === "FeederDuplex" && !ec.duplex);
  }
  if (srcSel.selectedOptions[0]?.disabled) srcSel.value = ec && ec.glass ? "Glass" : "Feeder";
  const dpi = Number(dpiSel.value);
  const note = $("engine-note");
  if (ec && !ec.resolutions.includes(dpi)) {
    const other = engSel.value === "Driver" ? "no driver" : "the installed vendor driver";
    note.textContent = `${dpi} dpi is not offered via ${ec.via}; it is offered with "${other}". Scanning will refuse as set.`;
    note.classList.add("cap-warn");
  } else if (ec) {
    note.textContent = `via ${ec.via}: ${ec.resolutions.join(", ")} dpi` + (ec.feeder ? ", glass and feeder" : ", glass only") + (ec.duplex ? ", both sides" : "");
    note.classList.remove("cap-warn");
  } else {
    note.textContent = "";
  }
}

async function loadCapabilities(d, i) {
  caps = null;
  $("engine-note").textContent = "asking the scanner what it offers…";
  try {
    const c = await invoke("scan_capabilities", { device: d });
    if (current !== i) return;
    caps = c;
    renderResolutions();
  } catch (e) {
    $("engine-note").textContent = "Could not ask the scanner what it offers: " + e;
  }
  Pictures.update($("scan-form"));
}

document.querySelectorAll(".tab").forEach((b) =>
  b.addEventListener("click", () => {
    if (!b.disabled) showTab(b.dataset.tab);
  })
);

// ---------- scan ----------

function log(text, cls) {
  const li = document.createElement("li");
  li.textContent = text;
  if (cls) li.classList.add(cls);
  $("scan-log").appendChild(li);
}

function describeEvent(ev) {
  switch (ev.kind) {
    case "starting": {
      const src = { Glass: "the glass", Feeder: "the feeder", FeederDuplex: "the feeder, both sides", Auto: "the scanner" }[ev.source] || ev.source;
      return [`Scanning from ${src} at ${ev.dpi} dpi…`];
    }
    case "feeder_empty":
      return ["The feeder is empty; using the glass instead.", "warn"];
    case "page":
      return [`Page ${ev.number} received (${human(ev.bytes)})`];
    case "substituted":
      return ["Note: " + ev.note, "warn"];
    default:
      return [JSON.stringify(ev)];
  }
}

function human(bytes) {
  if (bytes >= 1e6) return (bytes / 1e6).toFixed(1) + " MB";
  if (bytes >= 1e3) return Math.round(bytes / 1e3) + " KB";
  return bytes + " B";
}

listen("scan-progress", (e) => {
  const [text, cls] = describeEvent(e.payload);
  log(text, cls);
});

$("scan-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  if (current === null) return;
  const d = devices[current];
  const f = new FormData(ev.target);
  const format = f.get("format");
  const ext = { Pdf: "pdf", Jpeg: "jpg", Png: "png" }[format];
  const suggested = await invoke("default_scan_name", { extension: ext });
  const path = await invoke("choose_save_path", { suggestedName: suggested, extension: ext });
  if (!path) return;

  $("scan-log").innerHTML = "";
  $("scan-go").disabled = true;
  $("scan-state").textContent = "Scanning…";
  try {
    const written = await invoke("scan", {
      device: d,
      request: {
        source: f.get("source"),
        dpi: Number(f.get("dpi")),
        color: f.get("color"),
        paper: f.get("paper"),
        engine: f.get("engine"),
      },
      output: {
        format,
        color: f.get("color"),
        quality: null,
        max_bytes: f.get("max") ? Number(f.get("max")) : null,
      },
      path,
    });
    for (const w of written) {
      log(`Saved ${w.path} (${w.pages} page${w.pages === 1 ? "" : "s"}, ${human(w.bytes)})`, "ok");
      if (w.warning) log("Warning: " + w.warning, "warn");
    }
    $("scan-state").textContent = "Done.";
  } catch (err) {
    log("Failed: " + err, "bad");
    $("scan-state").textContent = "";
  }
  $("scan-go").disabled = false;
});

// ---------- cameras (Tapo through the account, go2rtc as the engine) ----------

let camStatus = null;
// Groups ("rooms"): { groups: [{name, cameras: [id], collapsed}], ungrouped: [id], ungrouped_collapsed }
let layout = { groups: [], ungrouped: [], ungrouped_collapsed: false };
let rearranging = false;
// null = every group; -1 = Ungrouped; otherwise an index into layout.groups
let viewGroup = null;
// Playback: per camera, the clips of the chosen day; and each tile's running playback
let pbDays = {};      // device_id -> { utc_offset_minutes, dates, today }
let pbClips = {};     // device_id -> { day_start, clips: [{start,end,video_type}], utc_offset_minutes }
let pbOffset = 0;     // the cameras' clock, minutes east of UTC (first camera asked)

function showCameras() {
  current = null;
  renderList();
  $("empty").classList.add("hidden");
  $("detail").classList.add("hidden");
  $("cameras").classList.remove("hidden");
  refreshCameras();
}

async function refreshCameras() {
  try {
    camStatus = await invoke("cameras_status");
  } catch (e) {
    $("cam-message").textContent = "Could not check the cameras: " + e;
    return;
  }
  $("cam-message").textContent = camStatus.message || "";
  $("cam-start").classList.toggle("hidden", camStatus.running || !camStatus.logged_in);
  $("cam-login").classList.toggle("hidden", camStatus.logged_in);
  $("cam-logout").classList.toggle("hidden", !camStatus.logged_in || camStatus.running);
  if (camStatus.running) {
    await loadLayout();
    renderCameraGrid();
  }
}

// ---- TP-Link account: log in from the window, the same steps as the CLI ----

function showLogin(show, note) {
  $("cam-login-form").classList.toggle("hidden", !show);
  $("cam-login-note").textContent = note || "";
  $("cam-login-code-row").classList.add("hidden");
}

$("cam-login").addEventListener("click", () => showLogin(true, ""));
$("cam-login-cancel").addEventListener("click", () => showLogin(false, ""));
$("cam-login-go").addEventListener("click", async () => {
  const email = $("cam-login-email").value.trim();
  const password = $("cam-login-password").value;
  if (!email || !password) { $("cam-login-note").textContent = "Email and password, please."; return; }
  $("cam-login-go").disabled = true;
  $("cam-login-note").textContent = "asking TP-Link…";
  try {
    const r = await invoke("tapo_login", { email, password });
    if (r.code_needed) {
      $("cam-login-note").textContent = r.message;
      $("cam-login-code-row").classList.remove("hidden");
      $("cam-login-code").focus();
    } else {
      showLogin(false, "");
      $("cam-login-password").value = "";
      refreshCameras();
    }
  } catch (e) {
    $("cam-login-note").textContent = "" + e;
  }
  $("cam-login-go").disabled = false;
});
$("cam-login-code-go").addEventListener("click", async () => {
  const code = $("cam-login-code").value.trim();
  if (!code) return;
  $("cam-login-code-go").disabled = true;
  try {
    await invoke("tapo_login_code", { code });
    showLogin(false, "");
    $("cam-login-password").value = "";
    $("cam-login-code").value = "";
    refreshCameras();
  } catch (e) {
    $("cam-login-note").textContent = "" + e;
  }
  $("cam-login-code-go").disabled = false;
});
$("cam-logout").addEventListener("click", async () => {
  try { await invoke("tapo_logout"); } catch (e) { $("cam-message").textContent = "" + e; }
  refreshCameras();
});

async function loadLayout() {
  try {
    layout = await invoke("camera_layout");
  } catch (e) {
    $("cam-message").textContent = "Could not read the camera groups: " + e;
  }
}

async function saveLayout() {
  try {
    await invoke("save_camera_layout", { layout });
  } catch (e) {
    $("cam-message").textContent = "Could not save the camera groups: " + e;
  }
}

function loadPlayer(src) {
  return new Promise((resolve, reject) => {
    if (customElements.get("video-stream")) return resolve();
    const s = document.createElement("script");
    s.type = "module";
    s.src = src;
    s.onload = () => resolve();
    s.onerror = () => reject(new Error("could not load the player from the video engine"));
    document.head.appendChild(s);
  });
}

function isPlayback() {
  return $("cam-when").value === "playback";
}

// Release a tile's player, its connection and any playback it holds.
function teardownTile(tile) {
  const v = tile.querySelector("video");
  if (v) {
    try { v.pause(); v.removeAttribute("src"); v.load(); } catch (_) {}
  }
  const vs = tile.querySelector("video-stream");
  if (vs) {
    try { vs.src = ""; } catch (_) {}
  }
  tile.querySelector("video, video-stream")?.remove();
  if (tile._watch) { clearInterval(tile._watch); tile._watch = null; }
  if (tile._playback) {
    const pb = tile._playback;
    tile._playback = null;
    invoke("playback_stop", { token: pb.token, stream: pb.stream }).catch(() => {});
  }
}

// Watch the clock: if it stops advancing, say so over the last frame.
function watchTile(tile, onTick) {
  let lastT = -1, stalledSince = 0, started = Date.now();
  tile._watch = setInterval(() => {
    const v = tile.querySelector("video") || tile.querySelector("video-stream")?.video;
    if (!v) return;
    const t = v.currentTime || 0;
    const now = Date.now();
    if (onTick && onTick(v, t, lastT)) return;
    if (v.error) {
      setState(tile, "lost", "no data from the camera");
      return;
    }
    if (t > lastT + 0.05) {
      lastT = t; stalledSince = 0;
      setState(tile, "live", "");
      return;
    }
    if (!stalledSince) stalledSince = now;
    const quiet = (now - stalledSince) / 1000;
    const total = (now - started) / 1000;
    if (lastT < 0 && total > 20) setState(tile, "lost", "no data from the camera");
    else if (lastT < 0) setState(tile, "connecting", "connecting…");
    else if (quiet > 15) setState(tile, "lost", "no data from the camera");
    else if (quiet > 2) setState(tile, "waiting", "waiting for the camera…");
  }, 1000);
}

// HD for an enlarged tile, or when the user asked for HD on that camera.
function wantsHd(tile, cam) {
  return tile.classList.contains("big") || (layout.hd || []).includes(cam.device_id);
}

function mountTile(tile, cam, big) {
  teardownTile(tile);
  const mode = $("cam-mode").value;
  const hd = big || wantsHd(tile, cam);
  let player;
  if (mode === "ts") {
    // MPEG-TS over HTTP in a plain <video>: measured as the route this
    // webview decodes at real time (its MP4 input is refused).
    player = document.createElement("video");
    player.autoplay = true;
    player.muted = true;
    player.playsInline = true;
    player.src = hd ? cam.ts_url : cam.ts_url_vga;
  } else {
    player = document.createElement("video-stream");
    player.setAttribute("mode", mode);
    player.src = hd ? cam.ws_url : cam.ws_url_vga;
  }
  tile.insertBefore(player, tile.firstChild);
  const q = tile.querySelector(".cam-quality");
  if (q) q.textContent = hd ? "HD" : "VGA";
  setState(tile, "connecting", "connecting…");
  // After a long silence, reconnect by itself: a few times, spaced out.
  let lostSince = 0;
  tile._retries = tile._retries || 0;
  watchTile(tile, (v, t, lastT) => {
    const lost = tile.dataset.state === "lost";
    if (!lost) { lostSince = 0; if (t > 0.05 && t > lastT) tile._retries = 0; return false; }
    if (!lostSince) lostSince = Date.now();
    const wait = Math.min(60, 5 * 2 ** tile._retries);
    if ((Date.now() - lostSince) / 1000 > wait && tile._retries < 6) {
      tile._retries += 1;
      mountTile(tile, cam, big);
      return true;
    }
    return false;
  });
}

// Play recorded footage from `from` (unix) to the end of that camera-day.
async function mountPlayback(tile, cam, from) {
  teardownTile(tile);
  const clips = pbClips[cam.device_id];
  const to = clips ? clips.day_start + 86400 : from + 3600;
  if (from >= to) { setState(tile, "lost", "that time is after the end of the day"); return; }
  setState(tile, "connecting", "asking the camera for its recording…");
  let handle;
  try {
    handle = await invoke("playback_start", { deviceId: cam.device_id, from, to });
  } catch (e) {
    setState(tile, "lost", "could not start: " + e);
    return;
  }
  tile._playback = { token: handle.token, stream: handle.stream, from };
  const player = document.createElement("video");
  player.autoplay = true;
  player.muted = true;
  player.playsInline = true;
  player.src = handle.ts_url;
  tile.insertBefore(player, tile.firstChild);
  let finished = false, lastPoll = 0;
  const cell = tile.closest(".cam-cell");
  watchTile(tile, (v, t, lastT) => {
    // Position marker and clock on the timeline.
    if (cell) updatePosition(cell, cam, from + t);
    const now = Date.now();
    if (now - lastPoll > 2000 && tile._playback) {
      lastPoll = now;
      invoke("playback_state", { token: tile._playback.token }).then((st) => {
        if (st.finished) finished = st.error ? "ended: " + st.error : "end of the recording";
      }).catch(() => {});
    }
    if (finished && t <= lastT + 0.05) {
      setState(tile, "lost", finished);
      tile.querySelector(".cam-reload")?.classList.add("hidden");
      return true;
    }
    return false;
  });
}

function setState(tile, state, text) {
  tile.dataset.state = state;
  const o = tile.querySelector(".cam-overlay");
  if (!o) return;
  o.querySelector(".cam-overlay-text").textContent = text;
  o.classList.toggle("hidden", state === "live");
}

function cameraById(id) {
  return camStatus.cameras.find((c) => c.device_id === id);
}

// The groups to show, in order: each as { key, name, ids, collapsed }.
function visibleGroups() {
  const all = layout.groups.map((g, i) => ({ key: i, name: g.name, ids: g.cameras, collapsed: g.collapsed }));
  if (layout.ungrouped.length || !all.length || rearranging) {
    all.push({ key: -1, name: "Ungrouped", ids: layout.ungrouped, collapsed: layout.ungrouped_collapsed });
  }
  if (viewGroup === null) return all;
  return all.filter((g) => g.key === viewGroup);
}

function groupList(key) {
  return key === -1 ? layout.ungrouped : layout.groups[key].cameras;
}

// ---- building the grid ----

function buildTile(cam) {
  const tile = document.createElement("div");
  tile.className = "cam-tile";
  const name = document.createElement("div");
  name.className = "cam-name";
  name.textContent = cam.name;
  tile.appendChild(name);
  const quality = document.createElement("button");
  quality.type = "button";
  quality.className = "cam-quality";
  quality.title = "HD or VGA for this camera (enlarged tiles are always HD)";
  quality.textContent = "VGA";
  quality.addEventListener("click", (ev) => {
    ev.stopPropagation();
    layout.hd = layout.hd || [];
    const i = layout.hd.indexOf(cam.device_id);
    if (i >= 0) layout.hd.splice(i, 1); else layout.hd.push(cam.device_id);
    saveLayout();
    if (!isPlayback()) mountTile(tile, cam, tile.classList.contains("big"));
  });
  tile.appendChild(quality);
  const overlay = document.createElement("div");
  overlay.className = "cam-overlay hidden";
  overlay.innerHTML = '<div class="spinner"></div><div class="cam-overlay-text"></div><button type="button" class="cam-reload">Reload</button>';
  overlay.querySelector(".cam-reload").addEventListener("click", (ev) => {
    ev.stopPropagation();
    remount(tile, cam);
  });
  tile.appendChild(overlay);
  tile.addEventListener("click", (ev) => {
    if (ev.target.closest(".cam-reload")) return;
    const cell = tile.closest(".cam-cell");
    const big = !cell.classList.contains("big");
    cell.classList.toggle("big", big);
    tile.classList.toggle("big", big);
    if (!isPlayback()) mountTile(tile, cam, big);
  });
  return tile;
}

function remount(tile, cam) {
  const big = tile.classList.contains("big");
  if (isPlayback()) {
    const from = tile._playback ? tile._playback.from : chosenTime();
    if (from !== null) mountPlayback(tile, cam, from);
  } else {
    mountTile(tile, cam, big);
  }
}

function buildCell(cam) {
  const cell = document.createElement("div");
  cell.className = "cam-cell";
  cell.dataset.id = cam.device_id;
  const tile = buildTile(cam);
  cell.appendChild(tile);
  if (isPlayback()) cell.appendChild(buildTimeline(cam, tile));
  return cell;
}

function buildPlaceholder(cam) {
  const cell = document.createElement("div");
  cell.className = "cam-cell";
  cell.dataset.id = cam.device_id;
  const tile = document.createElement("div");
  tile.className = "cam-tile placeholder";
  tile.textContent = cam.name;
  tile.draggable = true;
  tile.addEventListener("dragstart", (ev) => {
    ev.dataTransfer.setData("text/plain", cam.device_id);
    ev.dataTransfer.effectAllowed = "move";
    tile.classList.add("dragging");
  });
  tile.addEventListener("dragend", () => tile.classList.remove("dragging"));
  cell.appendChild(tile);
  return cell;
}

function moveCamera(id, toKey, beforeId) {
  for (const g of layout.groups) g.cameras = g.cameras.filter((x) => x !== id);
  layout.ungrouped = layout.ungrouped.filter((x) => x !== id);
  const list = groupList(toKey);
  const at = beforeId ? list.indexOf(beforeId) : -1;
  if (at >= 0) list.splice(at, 0, id); else list.push(id);
}

function buildGroup(g) {
  const box = document.createElement("div");
  box.className = "cam-group" + (g.collapsed ? " collapsed" : "");
  box.dataset.key = g.key;
  const head = document.createElement("div");
  head.className = "cam-group-head";
  const collapse = document.createElement("button");
  collapse.className = "collapse";
  collapse.type = "button";
  collapse.textContent = g.collapsed ? "▸" : "▾";
  collapse.title = g.collapsed ? "Show this group" : "Hide this group";
  collapse.addEventListener("click", () => {
    if (g.key === -1) layout.ungrouped_collapsed = !layout.ungrouped_collapsed;
    else layout.groups[g.key].collapsed = !layout.groups[g.key].collapsed;
    saveLayout();
    renderCameraGrid();
  });
  head.appendChild(collapse);
  const title = document.createElement("span");
  title.textContent = `${g.name} (${g.ids.length})`;
  head.appendChild(title);
  const tools = document.createElement("div");
  tools.className = "cam-group-tools";
  const small = (text, titleText, onClick) => {
    const b = document.createElement("button");
    b.type = "button"; b.className = "small"; b.textContent = text; b.title = titleText;
    b.addEventListener("click", onClick);
    tools.appendChild(b);
  };
  if (rearranging) {
    if (g.key !== -1) {
      small("Rename", "Rename this group", () => {
        const name = prompt("Group name", g.name);
        if (name && name.trim()) { layout.groups[g.key].name = name.trim(); saveLayout(); renderCameraGrid(); }
      });
      small("Remove", "Remove the group; its cameras go back to Ungrouped", () => {
        layout.ungrouped.push(...layout.groups[g.key].cameras);
        layout.groups.splice(g.key, 1);
        if (viewGroup === g.key) viewGroup = null;
        saveLayout(); renderCameraGrid();
      });
    }
  } else {
    if (viewGroup === null && visibleGroups().length > 1) {
      small("View", "Show only this group", () => { viewGroup = g.key; renderCameraGrid(); });
    }
    if (isPlayback()) {
      small("Play group", "Play every camera in this group from the chosen time", () => playGroup(box));
    }
  }
  head.appendChild(tools);
  box.appendChild(head);
  const inner = document.createElement("div");
  inner.className = "cam-grid-inner";
  if (rearranging) {
    inner.addEventListener("dragover", (ev) => { ev.preventDefault(); ev.dataTransfer.dropEffect = "move"; inner.classList.add("drop-target"); });
    inner.addEventListener("dragleave", () => inner.classList.remove("drop-target"));
    inner.addEventListener("drop", (ev) => {
      ev.preventDefault();
      inner.classList.remove("drop-target");
      const id = ev.dataTransfer.getData("text/plain");
      if (!id) return;
      const over = ev.target.closest(".cam-cell");
      moveCamera(id, g.key, over && over.dataset.id !== id ? over.dataset.id : null);
      saveLayout();
      renderCameraGrid();
    });
  }
  for (const id of g.ids) {
    const cam = cameraById(id);
    if (!cam) continue;
    inner.appendChild(rearranging ? buildPlaceholder(cam) : buildCell(cam));
  }
  box.appendChild(inner);
  return box;
}

async function renderCameraGrid() {
  const grid = $("cam-grid");
  grid.style.setProperty("--cols", $("cam-layout").value);
  if (!isPlayback() && !rearranging && $("cam-mode").value !== "ts") {
    try {
      await loadPlayer(camStatus.player_script);
    } catch (e) {
      $("cam-message").textContent = e.message;
      return;
    }
  }
  grid.querySelectorAll(".cam-tile").forEach(teardownTile);
  grid.innerHTML = "";
  $("cam-rearrange-bar").classList.toggle("hidden", !rearranging);
  $("cam-viewing").classList.toggle("hidden", viewGroup === null);
  $("cam-rearrange").classList.toggle("hidden", rearranging);
  $("pb-controls").classList.toggle("hidden", !isPlayback());
  $("cam-reload-all").classList.toggle("hidden", isPlayback());
  for (const g of visibleGroups()) grid.appendChild(buildGroup(g));
  if (!rearranging) {
    if (isPlayback()) {
      await preparePlayback();
    } else {
      grid.querySelectorAll(".cam-cell").forEach((cell) => {
        const tile = cell.querySelector(".cam-tile");
        mountTile(tile, cameraById(cell.dataset.id), false);
      });
    }
  }
  $("cam-state").textContent = `${camStatus.cameras.length} cameras`;
}

// ---- playback ----

// The chosen date and time as a unix time on the cameras' clock, or null.
function chosenTime() {
  const d = $("pb-date").value, t = $("pb-time").value || "00:00";
  if (!d) return null;
  const [y, m, day] = d.split("-").map(Number);
  const [hh, mm] = t.split(":").map(Number);
  return Date.UTC(y, m - 1, day, hh, mm) / 1000 - pbOffset * 60;
}

function clockText(unix) {
  const d = new Date((unix + pbOffset * 60) * 1000);
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getUTCHours())}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())}`;
}

// Ask every visible camera for the chosen day's clips; fill the timelines.
async function preparePlayback() {
  const cells = [...$("cam-grid").querySelectorAll(".cam-cell")];
  if (!cells.length) return;
  if (!$("pb-date").value) {
    try {
      const first = await invoke("recording_days", { deviceId: cells[0].dataset.id, days: 30 });
      pbOffset = first.utc_offset_minutes;
      $("pb-date").value = first.dates.length ? first.dates[first.dates.length - 1] : first.today;
      $("pb-date").max = first.today;
      $("pb-date").title = `Day, on the cameras' own clock. Days with footage on the first camera: ${first.dates.join(", ") || "none in the last 30 days"}`;
    } catch (e) {
      $("cam-message").textContent = "Could not ask the camera for its recordings: " + e;
      return;
    }
  }
  if (!$("pb-time").value) $("pb-time").value = "10:00";
  const date = $("pb-date").value;
  await Promise.all(cells.map(async (cell) => {
    const id = cell.dataset.id;
    const cam = cameraById(id);
    const tile = cell.querySelector(".cam-tile");
    setState(tile, "lost", "pick a time and press Play, or click the timeline");
    tile.querySelector(".cam-reload")?.classList.add("hidden");
    try {
      pbClips[id] = await invoke("recording_clips", { deviceId: id, date });
      pbOffset = pbClips[id].utc_offset_minutes;
    } catch (e) {
      pbClips[id] = null;
      setState(tile, "lost", "no recording list: " + e);
    }
    fillTimeline(cell, cam);
  }));
}

function buildTimeline(cam, tile) {
  const wrap = document.createElement("div");
  const bar = document.createElement("div");
  bar.className = "pb-bar";
  bar.title = "The day's recordings; click to play from there";
  bar.addEventListener("click", (ev) => {
    const clips = pbClips[cam.device_id];
    if (!clips) return;
    const r = bar.getBoundingClientRect();
    const at = clips.day_start + Math.floor(((ev.clientX - r.left) / r.width) * 86400);
    mountPlayback(tile, cam, at);
  });
  wrap.appendChild(bar);
  const row = document.createElement("div");
  row.className = "pb-row";
  row.innerHTML = '<span class="pb-at">—</span><button type="button" class="pb-download">Download…</button><span class="muted pb-note"></span>';
  row.querySelector(".pb-download").addEventListener("click", () => downloadForm(wrap, cam, tile));
  wrap.appendChild(row);
  return wrap;
}

function fillTimeline(cell, cam) {
  const bar = cell.querySelector(".pb-bar");
  const note = cell.querySelector(".pb-note");
  if (!bar) return;
  bar.innerHTML = "";
  const clips = pbClips[cam.device_id];
  if (!clips) { note.textContent = ""; return; }
  for (const c of clips.clips) {
    const seg = document.createElement("div");
    seg.className = "clip" + (c.video_type === 2 ? " event" : "");
    seg.style.left = `${((c.start - clips.day_start) / 86400) * 100}%`;
    seg.style.width = `${Math.max(0.2, ((c.end - c.start) / 86400) * 100)}%`;
    seg.title = `${clockText(c.start)} – ${clockText(c.end)}`;
    bar.appendChild(seg);
  }
  const pos = document.createElement("div");
  pos.className = "pos hidden";
  bar.appendChild(pos);
  const total = clips.clips.reduce((a, c) => a + (c.end - c.start), 0);
  const card = clips.sd_card;
  note.textContent = clips.clips.length
    ? `${clips.clips.length} recordings, ${Math.round(total / 60)} min` + (card.state !== "normal" ? `; card ${card.state}` : "")
    : (card.state === "normal" ? "nothing recorded that day" : `SD card ${card.state}`);
}

function updatePosition(cell, cam, at) {
  const clips = pbClips[cam.device_id];
  const pos = cell.querySelector(".pb-bar .pos");
  if (clips && pos) {
    pos.classList.remove("hidden");
    pos.style.left = `${((at - clips.day_start) / 86400) * 100}%`;
  }
  const label = cell.querySelector(".pb-at");
  if (label) label.textContent = clockText(at);
}

function playAll(root) {
  const from = chosenTime();
  if (from === null) { $("cam-message").textContent = "Pick a date first."; return; }
  root.querySelectorAll(".cam-cell").forEach((cell) => {
    const cam = cameraById(cell.dataset.id);
    mountPlayback(cell.querySelector(".cam-tile"), cam, from);
  });
}

function playGroup(box) { playAll(box); }

function downloadForm(wrap, cam, tile) {
  let form = wrap.querySelector(".pb-form");
  if (form) { form.remove(); return; }
  form = document.createElement("div");
  form.className = "pb-form";
  const at = tile._playback ? tile._playback.from : chosenTime();
  const start = at !== null ? clockText(at).slice(0, 5) : ($("pb-time").value || "10:00");
  form.innerHTML = `from <input type="time" class="pb-from" value="${start}" step="60"> for <input type="number" class="pb-mins" value="10" min="1" max="720"> min <button type="button" class="pb-save">Save as MP4…</button><span class="muted pb-dl-note"></span>`;
  form.querySelector(".pb-save").addEventListener("click", async () => {
    const clips = pbClips[cam.device_id];
    if (!clips) return;
    const [hh, mm] = form.querySelector(".pb-from").value.split(":").map(Number);
    const from = clips.day_start + hh * 3600 + mm * 60;
    const mins = Number(form.querySelector(".pb-mins").value) || 10;
    const to = from + mins * 60;
    const stamp = `${$("pb-date").value}-${String(hh).padStart(2, "0")}${String(mm).padStart(2, "0")}`;
    const safe = cam.name.replace(/[^\p{L}\p{N}]+/gu, "-");
    let path;
    try {
      path = await invoke("choose_save_path", { suggestedName: `${safe}-${stamp}.mp4`, extension: "mp4" });
    } catch (e) { return; }
    if (!path) return;
    const note = form.querySelector(".pb-dl-note");
    note.textContent = "asking the camera…";
    form.querySelector(".pb-save").disabled = true;
    form.dataset.deviceId = cam.device_id;
    try {
      const bytes = await invoke("download_recording", { deviceId: cam.device_id, from, to, path });
      note.textContent = `saved ${human(bytes)}`;
    } catch (e) {
      note.textContent = "failed: " + e;
    }
    form.querySelector(".pb-save").disabled = false;
  });
  wrap.appendChild(form);
}

listen("download-progress", (e) => {
  const p = e.payload;
  document.querySelectorAll(`.pb-form[data-device-id="${p.device_id}"] .pb-dl-note`).forEach((n) => {
    n.textContent = `saving… ${human(p.bytes)}`;
  });
});

// ---- controls ----

$("cameras-open").addEventListener("click", showCameras);
$("cam-layout").addEventListener("change", () => $("cam-grid").style.setProperty("--cols", $("cam-layout").value));
$("cam-mode").addEventListener("change", () => { if (camStatus && camStatus.running) renderCameraGrid(); });
$("cam-reload-all").addEventListener("click", () => { if (camStatus && camStatus.running) renderCameraGrid(); });
$("cam-when").addEventListener("change", () => { if (camStatus && camStatus.running) renderCameraGrid(); });
$("pb-date").addEventListener("change", () => { if (camStatus && camStatus.running) renderCameraGrid(); });
$("pb-play").addEventListener("click", () => playAll($("cam-grid")));
$("cam-rearrange").addEventListener("click", () => { rearranging = true; viewGroup = null; renderCameraGrid(); });
$("cam-rearrange-done").addEventListener("click", () => { rearranging = false; renderCameraGrid(); });
$("cam-add-group").addEventListener("click", () => {
  const name = prompt("Name of the new group (a room, a floor, a side of the house)", "");
  if (!name || !name.trim()) return;
  layout.groups.push({ name: name.trim(), cameras: [], collapsed: false });
  saveLayout();
  renderCameraGrid();
});
$("cam-show-all").addEventListener("click", () => { viewGroup = null; renderCameraGrid(); });
$("cam-start").addEventListener("click", async () => {
  $("cam-start").disabled = true;
  $("cam-state").textContent = "starting…";
  try {
    camStatus = await invoke("cameras_start");
    $("cam-message").textContent = "";
    $("cam-start").classList.add("hidden");
    await loadLayout();
    renderCameraGrid();
  } catch (e) {
    $("cam-message").textContent = "Could not start: " + e;
    $("cam-state").textContent = "";
  }
  $("cam-start").disabled = false;
});
listen("cameras-progress", (e) => {
  $("cam-state").textContent = e.payload;
});

$("rescan").addEventListener("click", rescan);
invoke("startup_view").then((v) => {
  if (v === "cameras") {
    showCameras();
    setTimeout(() => $("cam-start").click(), 800);
  }
}).catch(() => {});
$("scan-form").addEventListener("change", () => {
  renderResolutions();
  Pictures.update($("scan-form"));
});
Pictures.update($("scan-form"));
rescan();
