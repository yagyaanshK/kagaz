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
  tile._audioWanted = !!tile.querySelector("audio");
  stopAudio(tile);
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
// Recording spans of a camera's day: its clips with the hairline seams
// between chunks (a second or two) closed up. A camera's stream skips its
// own gaps, so a playback is only ever asked for within one span.
function spansOf(id) {
  const c = pbClips[id];
  if (!c) return null;
  if (!c.spans) {
    const out = [];
    for (const clip of c.clips) {
      const last = out[out.length - 1];
      if (last && clip.start - last.end <= 2) last.end = Math.max(last.end, clip.end);
      else out.push({ start: clip.start, end: clip.end });
    }
    c.spans = out;
  }
  return c.spans;
}
function spanAt(id, t) { const sp = spansOf(id); return sp ? sp.find((x) => x.start <= t && t < x.end) || null : null; }
function nextSpan(id, t) { const sp = spansOf(id); return sp ? sp.find((x) => x.start > t) || null : null; }

// Each tile keeps its own playback clock: where it is in the camera's day,
// running at the chosen speed. The video sets it while it plays; through a
// gap it runs on by itself.
function setClock(tile, at) { tile._clock = { at, wall: Date.now(), speed: speedValue() }; }
function tileNow(tile) {
  const c = tile._clock;
  return c ? c.at + ((Date.now() - c.wall) / 1000) * c.speed : null;
}

// Hold a camera in a gap: last frame frozen under the veil, its slot freed.
function enterGap(tile, cam, clock) {
  const v = tile.querySelector("video");
  if (v) { try { v.pause(); } catch (_) {} }
  if (tile._watch) { clearInterval(tile._watch); tile._watch = null; }
  if (tile._playback) {
    const pb = tile._playback; tile._playback = null;
    invoke("playback_stop", { token: pb.token, stream: pb.stream }).catch(() => {});
  }
  stopAudio(tile);
  tile._gap = true;
  gapText(tile, cam, clock);
}
function gapText(tile, cam, clock) {
  const next = nextSpan(cam.device_id, clock);
  setState(tile, "gap", next ? `no recording for this time; resumes at ${clockText(next.start)}` : "no recording for the rest of the day");
}

async function mountPlayback(tile, cam, from) {
  from = Math.floor(from);
  teardownTile(tile);
  tile._gap = false;
  tile._ended = false;
  setClock(tile, from);
  const clips = pbClips[cam.device_id];
  let to;
  if (clips) {
    const span = spanAt(cam.device_id, from);
    if (!span) { enterGap(tile, cam, from); return; }
    to = span.end;
  } else {
    // No list: ask to the end of the day (the camera will skip its own gaps).
    const day = seekDayStart();
    to = day !== null ? day + 86400 : from + 3600;
  }
  if (from >= to) { setState(tile, "lost", "that time is after the end of the day"); return; }
  // Hold the clock until the picture moves, so the cursor does not run ahead while connecting.
  tile._clock = { at: from, wall: Date.now(), speed: 0 };
  setState(tile, "connecting", "asking the camera for its recording…");
  let handle;
  const speed = speedValue();
  const mountId = (tile._mountId = (tile._mountId || 0) + 1);
  try {
    handle = await invoke("playback_start", { deviceId: cam.device_id, from, to, speed });
  } catch (e) {
    if (tile._mountId !== mountId) return;
    setState(tile, "lost", "could not start: " + e);
    return;
  }
  if (tile._mountId !== mountId) {
    // Another seek came in meanwhile: drop this one.
    invoke("playback_stop", { token: handle.token, stream: handle.stream }).catch(() => {});
    return;
  }
  tile._playback = { token: handle.token, stream: handle.stream, from, to, speed };
  tile._retries = tile._retries || 0;
  const player = document.createElement("video");
  player.autoplay = true;
  player.muted = true;
  player.playsInline = true;
  player.src = handle.ts_url;
  tile.insertBefore(player, tile.firstChild);
  let finished = null, lastPoll = 0, lostSince = 0;
  watchTile(tile, (v, t, lastT) => {
    // The stream's clock runs at 1/speed of the camera's.
    if (t > lastT + 0.05) {
      tile._clock = { at: from + t * speed, wall: Date.now(), speed };
      tile._retries = 0;
    }
    const now = Date.now();
    if (now - lastPoll > 2000 && tile._playback) {
      lastPoll = now;
      invoke("playback_state", { token: tile._playback.token }).then((st) => {
        if (st.finished) finished = st.error ? { error: st.error } : { done: true };
      }).catch(() => {});
    }
    if (finished && t <= lastT + 0.05) {
      if (finished.done) {
        // The span is over; the tile's clock carries on into the gap (or,
        // without a list, the recording simply ends here).
        tile._ended = true;
        if (!pbClips[cam.device_id]) setState(tile, "ended", "end of the recording");
        return true;
      }
      setState(tile, "lost", "camera: " + finished.error);
    }
    if (tile.dataset.state !== "lost") { lostSince = 0; return !!finished; }
    if (!lostSince) lostSince = now;
    const wait = Math.min(60, 5 * 2 ** tile._retries);
    if ((now - lostSince) / 1000 > wait && tile._retries < 6) {
      tile._retries += 1;
      mountPlayback(tile, cam, tileNow(tile));
      return true;
    }
    return !!finished;
  });
}

// States: connecting, waiting (spinner), live (no overlay), lost (error:
// reload icon, retried by itself), ended (a recording is over: reload icon
// plays it again, no automatic retry).
function setState(tile, state, text) {
  tile.dataset.state = state;
  const o = tile.querySelector(".cam-overlay");
  if (!o) return;
  o.querySelector(".cam-overlay-text").textContent = text;
  o.querySelector(".cam-reload").classList.toggle("hidden", state !== "lost" && state !== "ended");
  o.classList.toggle("hidden", state === "live");
  if (state === "live" && tile._audioWanted && !tile.querySelector("audio")) {
    tile._audioWanted = false;
    const cam = cameraById(tile.closest(".cam-cell")?.dataset.id);
    if (cam) toggleAudio(tile, cam);
  }
}

// ---- sound: a streamed WAV from the stream server, tapped off the tile's own session ----

function audioBase(cam) {
  // The stream server on this tile's own loopback host (same host and port as its video).
  return cam.ts_url.replace(/\/engine\/.*$/, "");
}

function stopAudio(tile) {
  const a = tile.querySelector("audio");
  if (a) {
    try { a.pause(); a.removeAttribute("src"); a.load(); } catch (_) {}
    a.remove();
  }
  const b = tile.querySelector(".cam-speaker");
  if (b) { b.textContent = "\u{1F507}"; b.classList.remove("on"); }
}

function toggleAudio(tile, cam) {
  if (tile.querySelector("audio")) { stopAudio(tile); return; }
  // One camera audible at a time.
  document.querySelectorAll(".cam-tile audio").forEach((a) => stopAudio(a.closest(".cam-tile")));
  const key = isPlayback() ? (tile._playback && tile._playback.token) : cam.device_id;
  if (!key) return;
  if (isPlayback() && tile._playback.speed !== 1) { $("cam-message").textContent = "Sound plays at 1x only."; return; }
  const a = document.createElement("audio");
  a.autoplay = true;
  a.src = `${audioBase(cam)}/audio/${key}.wav?t=${Date.now()}`;
  a.addEventListener("error", () => stopAudio(tile));
  tile.appendChild(a);
  const b = tile.querySelector(".cam-speaker");
  if (b) { b.textContent = "\u{1F50A}"; b.classList.add("on"); }
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
  const speaker = document.createElement("button");
  speaker.type = "button";
  speaker.className = "cam-speaker";
  speaker.title = "Hear this camera (one at a time)";
  speaker.textContent = "\u{1F507}";
  speaker.addEventListener("click", (ev) => {
    ev.stopPropagation();
    toggleAudio(tile, cam);
  });
  tile.appendChild(speaker);
  const overlay = document.createElement("div");
  overlay.className = "cam-overlay hidden";
  overlay.innerHTML = '<div class="spinner"></div><button type="button" class="cam-reload" title="Reload this camera">&#x21bb;</button><div class="cam-overlay-text"></div>';
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
    const from = tileNow(tile) ?? chosenTime();
    tile._retries = 0;
    if (from !== null) mountPlayback(tile, cam, from);
    else setState(tile, "lost", "pick a date first");
  } else {
    tile._retries = 0;
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

function draggingGroup() { return !!document.querySelector(".cam-group.dragging"); }
function clearDropMarks() {
  document.querySelectorAll(".cam-group.drop-before, .cam-group.drop-after").forEach((b) => b.classList.remove("drop-before", "drop-after"));
}

// Where a camera dragged over `inner` would be inserted: the cell it goes
// before (null = at the end) and the place for the vertical marker, in the
// grid's own coordinates.
function dropPosition(inner, x, y) {
  const base = inner.getBoundingClientRect();
  const cells = [...inner.querySelectorAll(".cam-cell")].filter((c) => !c.querySelector(".dragging"));
  if (!cells.length) return { beforeId: null, x: 0, top: 0, height: base.height };
  // Cells on the row under the cursor; otherwise the nearest row.
  let row = cells.filter((c) => { const r = c.getBoundingClientRect(); return y >= r.top && y <= r.bottom; });
  if (!row.length) {
    const last = cells[cells.length - 1].getBoundingClientRect();
    if (y > last.bottom) row = cells.filter((c) => c.getBoundingClientRect().top === last.top);
    else {
      const first = cells[0].getBoundingClientRect();
      row = cells.filter((c) => c.getBoundingClientRect().top === first.top);
    }
  }
  for (const c of row) {
    const r = c.getBoundingClientRect();
    if (x < r.left + r.width / 2) {
      return { beforeId: c.dataset.id, x: r.left - base.left - 4, top: r.top - base.top, height: r.height };
    }
  }
  const last = row[row.length - 1];
  const r = last.getBoundingClientRect();
  const next = cells[cells.indexOf(last) + 1];
  return { beforeId: next ? next.dataset.id : null, x: r.right - base.left, top: r.top - base.top, height: r.height };
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
  if (rearranging && g.key !== -1) {
    head.draggable = true;
    head.title = "Drag to move this group above or below another";
    head.addEventListener("dragstart", (ev) => {
      ev.dataTransfer.setData("text/plain", "group:" + g.key);
      ev.dataTransfer.effectAllowed = "move";
      box.classList.add("dragging");
    });
    head.addEventListener("dragend", () => {
      box.classList.remove("dragging");
      clearDropMarks();
    });
  }
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
    // A dragged group lands above or below this group (by cursor half).
    box.addEventListener("dragover", (ev) => {
      if (!draggingGroup() || g.key === -1) return;
      ev.preventDefault();
      ev.dataTransfer.dropEffect = "move";
      const r = box.getBoundingClientRect();
      const after = ev.clientY > r.top + r.height / 2;
      clearDropMarks();
      box.classList.add(after ? "drop-after" : "drop-before");
    });
    box.addEventListener("drop", (ev) => {
      const data = ev.dataTransfer.getData("text/plain");
      if (!data.startsWith("group:")) return;
      ev.preventDefault();
      ev.stopPropagation();
      const from = Number(data.slice(6));
      const after = box.classList.contains("drop-after");
      clearDropMarks();
      if (g.key === -1 || from === g.key) return;
      const [moved] = layout.groups.splice(from, 1);
      const to = g.key > from ? g.key - 1 : g.key;
      layout.groups.splice(after ? to + 1 : to, 0, moved);
      saveLayout();
      renderCameraGrid();
    });
    // A dragged camera: show where it will land, then put it there.
    const line = document.createElement("div");
    line.className = "drop-line hidden";
    inner.appendChild(line);
    inner.addEventListener("dragover", (ev) => {
      if (draggingGroup()) return;
      ev.preventDefault();
      ev.dataTransfer.dropEffect = "move";
      inner.classList.add("drop-target");
      const at = dropPosition(inner, ev.clientX, ev.clientY);
      inner._beforeId = at.beforeId;
      line.classList.remove("hidden");
      line.style.left = `${at.x}px`;
      line.style.top = `${at.top}px`;
      line.style.height = `${at.height}px`;
    });
    inner.addEventListener("dragleave", (ev) => {
      if (ev.relatedTarget && inner.contains(ev.relatedTarget)) return;
      inner.classList.remove("drop-target");
      line.classList.add("hidden");
    });
    inner.addEventListener("drop", (ev) => {
      const id = ev.dataTransfer.getData("text/plain");
      if (!id || id.startsWith("group:")) return;
      ev.preventDefault();
      ev.stopPropagation();
      inner.classList.remove("drop-target");
      line.classList.add("hidden");
      moveCamera(id, g.key, inner._beforeId || null);
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
  if (!isPlayback()) { pbMaster = null; updateSeekBar(); }
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

// The speed slider is logarithmic: -2 → 0.25x, 0 → 1x, 3 → 8x.
function speedValue() {
  const v = Math.pow(2, Number($("pb-speed").value));
  return Math.round(v * 20) / 20;
}
function speedText(v) {
  return (v >= 1 ? v.toFixed(v % 1 ? 1 : 0) : v.toFixed(2).replace(/0+$/, "")) + "x";
}

// The shared bar: every visible camera's recordings for the day, one
// position marker for the moment being played.
function fillSeekBar() {
  const clipsBox = $("pb-seek-clips");
  clipsBox.innerHTML = "";
  let dayStart = null;
  for (const cell of $("cam-grid").querySelectorAll(".cam-cell")) {
    const c = pbClips[cell.dataset.id];
    if (!c) continue;
    dayStart = c.day_start;
    for (const clip of c.clips) {
      const seg = document.createElement("div");
      seg.className = "clip";
      seg.style.left = `${((clip.start - c.day_start) / 86400) * 100}%`;
      seg.style.width = `${Math.max(0.15, ((clip.end - clip.start) / 86400) * 100)}%`;
      clipsBox.appendChild(seg);
    }
  }
  $("pb-seek").dataset.dayStart = dayStart === null ? "" : String(dayStart);
}

function seekDayStart() {
  const v = $("pb-seek").dataset.dayStart;
  if (v) return Number(v);
  const t = chosenTime();
  if (t === null) return null;
  return t - ((t + pbOffset * 60) % 86400 + 86400) % 86400;
}

function seekTimeAt(clientX) {
  const r = $("pb-seek").getBoundingClientRect();
  const frac = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
  const day = seekDayStart();
  return day === null ? null : day + Math.floor(frac * 86400);
}

function updateSeekBar(at) {
  const day = seekDayStart();
  const pos = $("pb-seek-pos");
  if (at === undefined || day === null) { pos.classList.add("hidden"); $("pb-seek-at").textContent = "—"; return; }
  pos.classList.remove("hidden");
  pos.style.left = `${((at - day) / 86400) * 100}%`;
  $("pb-seek-at").textContent = clockText(at);
}

// The top bar's cursor: the moment last chosen for all cameras, running on
// at the chosen speed. Each camera keeps its own clock (its bar may have
// moved it elsewhere).
let pbMaster = null;
function startMaster(at) { pbMaster = { at, wall: Date.now(), speed: speedValue() }; }
function masterNow() { return pbMaster ? pbMaster.at + Math.floor(((Date.now() - pbMaster.wall) / 1000) * pbMaster.speed) : null; }

// Move every visible camera to `at`.
function seekAll(at) {
  setPickerTime(clock24(at));
  startMaster(at);
  $("cam-grid").querySelectorAll(".cam-cell").forEach((cell) => {
    mountPlayback(cell.querySelector(".cam-tile"), cameraById(cell.dataset.id), at);
  });
  updateSeekBar(at);
}

// Once a second: move the cursors, and hold or resume each camera by its
// own clock against its recording spans.
setInterval(() => {
  if (!isPlayback()) return;
  if (pbMaster) updateSeekBar(masterNow());
  $("cam-grid").querySelectorAll(".cam-cell").forEach((cell) => {
    const tile = cell.querySelector(".cam-tile");
    const clock = tileNow(tile);
    if (clock === null) return;
    const cam = cameraById(cell.dataset.id);
    updatePosition(cell, cam, clock);
    if (!pbClips[cell.dataset.id]) return;
    const span = spanAt(cam.device_id, clock);
    if (!span) {
      if (!tile._gap) enterGap(tile, cam, clock);
      else gapText(tile, cam, clock);
    } else if (tile._gap || (tile._ended && clock < span.end - 2)) {
      mountPlayback(tile, cam, clock);
    }
  });
}, 1000);

(function wireSeekBar() {
  const bar = $("pb-seek");
  let dragging = false;
  const hover = $("pb-seek-hover");
  bar.addEventListener("mousemove", (ev) => {
    const r = bar.getBoundingClientRect();
    hover.classList.remove("hidden");
    hover.style.left = `${ev.clientX - r.left}px`;
    const t = seekTimeAt(ev.clientX);
    bar.title = t === null ? "" : clockText(t);
    if (dragging) updateSeekBar(t);
  });
  bar.addEventListener("mouseleave", () => hover.classList.add("hidden"));
  bar.addEventListener("mousedown", (ev) => { dragging = true; ev.preventDefault(); });
  window.addEventListener("mouseup", (ev) => {
    if (!dragging) return;
    dragging = false;
    const t = seekTimeAt(ev.clientX);
    if (t !== null) seekAll(t);
  });
})();

$("pb-speed").addEventListener("input", () => { $("pb-speed-label").textContent = speedText(speedValue()); });
$("pb-speed").addEventListener("change", () => {
  if (pbMaster) startMaster(masterNow());
  // Apply the new speed to every camera, from where each one is.
  $("cam-grid").querySelectorAll(".cam-tile").forEach((tile) => {
    const at = tileNow(tile);
    if (at === null) return;
    if (tile._gap) { setClock(tile, at); return; }
    mountPlayback(tile, cameraById(tile.closest(".cam-cell").dataset.id), at);
  });
});

// The gear menu.
$("cam-gear").addEventListener("click", (ev) => { ev.stopPropagation(); $("cam-settings").classList.toggle("hidden"); });
document.addEventListener("click", (ev) => { if (!ev.target.closest(".cam-menu")) $("cam-settings").classList.add("hidden"); });

// The chosen date and time as a unix time on the cameras' clock, or null.
function chosenTime() {
  const d = $("pb-date").value, t = $("pb-time").value || "00:00:00";
  if (!d) return null;
  const [y, m, day] = d.split("-").map(Number);
  const [hh, mm, ss] = t.split(":").map(Number);
  return Date.UTC(y, m - 1, day, hh, mm, ss || 0) / 1000 - pbOffset * 60;
}

// hh:mm:ss on a 12-hour clock with AM/PM.
function clockText(unix) {
  const d = new Date((unix + pbOffset * 60) * 1000);
  const p = (n) => String(n).padStart(2, "0");
  const h = d.getUTCHours();
  return `${p(h % 12 || 12)}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())} ${h < 12 ? "AM" : "PM"}`;
}
// hh:mm:ss on the 24-hour clock, for the download form's time field.
function clock24(unix) {
  const d = new Date((unix + pbOffset * 60) * 1000);
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getUTCHours())}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())}`;
}

// "2026-10-01" → "01/10/2026".
function ddmmyyyy(iso) {
  const [y, m, d] = iso.split("-");
  return `${d}/${m}/${y}`;
}

const pad2 = (n) => String(n).padStart(2, "0");

// The day and time pickers: plain dropdowns, dd/mm/yyyy and hh:mm:ss.
function fillSelect(id, values, labels, current) {
  const sel = $(id);
  const keep = current !== undefined ? current : sel.value;
  sel.innerHTML = "";
  values.forEach((v, i) => {
    const o = document.createElement("option");
    o.value = v; o.textContent = labels ? labels[i] : v;
    sel.appendChild(o);
  });
  if ([...sel.options].some((o) => o.value === keep)) sel.value = keep;
}

let pbDatesWithFootage = [];
function daysInMonth(y, m) { return new Date(Date.UTC(y, m, 0)).getUTCDate(); }

function buildPickers(todayIso) {
  const [ty, tm] = todayIso.split("-").map(Number);
  const years = []; for (let y = ty - 2; y <= ty; y++) years.push(String(y));
  fillSelect("pb-yy", years, null, String(ty));
  const months = []; for (let m = 1; m <= 12; m++) months.push(pad2(m));
  fillSelect("pb-mm", months, null, pad2(tm));
  refreshDays();
  const hours = [], minutes = [];
  for (let h = 0; h < 24; h++) hours.push(pad2(h));
  for (let m = 0; m < 60; m++) minutes.push(pad2(m));
  fillSelect("pb-h", hours.slice(1, 13), null, "10");
  fillSelect("pb-m", minutes, null, "00");
  fillSelect("pb-s", minutes, null, "00");
}

function refreshDays() {
  const y = Number($("pb-yy").value), m = Number($("pb-mm").value);
  const n = daysInMonth(y, m);
  const days = [], labels = [];
  for (let d = 1; d <= n; d++) {
    const iso = `${y}-${pad2(m)}-${pad2(d)}`;
    days.push(pad2(d));
    labels.push(pad2(d) + (pbDatesWithFootage.includes(iso) ? " •" : ""));
  }
  fillSelect("pb-dd", days, labels);
}

function setPickerDate(iso) {
  const [y, m, d] = iso.split("-");
  $("pb-yy").value = y; $("pb-mm").value = m; refreshDays(); $("pb-dd").value = d;
  $("pb-date").value = iso;
}

// Set the time dropdowns from a 24-hour "hh:mm:ss".
function setPickerTime(hms) {
  const [h, m, s] = hms.split(":").map(Number);
  $("pb-h").value = pad2(h % 12 || 12); $("pb-m").value = pad2(m); $("pb-s").value = pad2(s || 0);
  $("pb-ap").value = h < 12 ? "AM" : "PM";
  $("pb-time").value = `${pad2(h)}:${pad2(m)}:${pad2(s || 0)}`;
}

function readPickers() {
  $("pb-date").value = `${$("pb-yy").value}-${$("pb-mm").value}-${$("pb-dd").value}`;
  let h = Number($("pb-h").value) % 12;
  if ($("pb-ap").value === "PM") h += 12;
  $("pb-time").value = `${pad2(h)}:${$("pb-m").value}:${$("pb-s").value}`;
}

["pb-yy", "pb-mm"].forEach((id) => $(id).addEventListener("change", () => { refreshDays(); readPickers(); if (camStatus && camStatus.running) renderCameraGrid(); }));
$("pb-dd").addEventListener("change", () => { readPickers(); if (camStatus && camStatus.running) renderCameraGrid(); });
["pb-h", "pb-m", "pb-s", "pb-ap"].forEach((id) => $(id).addEventListener("change", readPickers));

// Ask every visible camera for the chosen day's clips; fill the timelines.
async function preparePlayback() {
  const cells = [...$("cam-grid").querySelectorAll(".cam-cell")];
  if (!cells.length) return;
  if (!$("pb-date").value) {
    try {
      const first = await invoke("recording_days", { deviceId: cells[0].dataset.id, days: 60 });
      pbOffset = first.utc_offset_minutes;
      pbDatesWithFootage = first.dates;
      buildPickers(first.today);
      setPickerDate(first.dates.length ? first.dates[first.dates.length - 1] : first.today);
      setPickerTime("10:00:00");
    } catch (e) {
      $("cam-message").textContent = "Could not ask the camera for its recordings: " + e;
      return;
    }
  }
  const date = $("pb-date").value;
  pbClips = {};
  pbMaster = null;
  updateSeekBar();
  for (const cell of cells) {
    const tile = cell.querySelector(".cam-tile");
    tile._gap = false;
    tile._clock = null;
    tile._ended = false;
    setState(tile, "lost", "pick a time and press Play, or click the timeline");
    tile.querySelector(".cam-reload")?.classList.add("hidden");
  }
  // One camera at a time (TP-Link's cloud drops some of a burst), a second try each.
  for (const cell of cells) {
    const id = cell.dataset.id;
    const cam = cameraById(id);
    const note = cell.querySelector(".pb-note");
    if (note) note.textContent = "asking the camera for its recordings…";
    let lastError = null;
    for (let attempt = 0; attempt < 2 && !pbClips[id]; attempt++) {
      try {
        pbClips[id] = await invoke("recording_clips", { deviceId: id, date });
        pbOffset = pbClips[id].utc_offset_minutes;
      } catch (e) {
        lastError = e;
        pbClips[id] = null;
      }
    }
    if (!pbClips[id]) {
      const tile = cell.querySelector(".cam-tile");
      setState(tile, "lost", "no recording list: " + lastError);
      tile._retries = 0;
    }
    fillTimeline(cell, cam);
    if (!pbClips[id] && note) note.textContent = "no recording list: " + lastError;
  }
  fillSeekBar();
  updateSeekBar();
}

const WINDOW = 12 * 3600;

// The 12 hours a camera's bar shows: centred on its clock, kept inside the day.
function windowStart(id, clock) {
  const c = pbClips[id];
  const day = c ? c.day_start : seekDayStart();
  if (day === null) return null;
  const at = clock !== null && clock !== undefined ? clock : (chosenTime() ?? day + 12 * 3600);
  const start = Math.round((at - WINDOW / 2) / 600) * 600; // move in 10-minute steps
  return Math.min(day + 86400 - WINDOW, Math.max(day, start));
}

function buildTimeline(cam, tile) {
  const wrap = document.createElement("div");
  const bar = document.createElement("div");
  bar.className = "pb-bar";
  bar.title = "This camera's recordings, 12 hours around where it is; click to move this camera only";
  bar.addEventListener("mousemove", (ev) => {
    const ws = Number(bar.dataset.ws);
    if (!bar.dataset.ws) return;
    const r = bar.getBoundingClientRect();
    bar.title = clockText(ws + Math.floor(((ev.clientX - r.left) / r.width) * WINDOW));
  });
  bar.addEventListener("click", (ev) => {
    if (!bar.dataset.ws) return;
    const r = bar.getBoundingClientRect();
    const at = Number(bar.dataset.ws) + Math.floor(((ev.clientX - r.left) / r.width) * WINDOW);
    mountPlayback(tile, cam, at);
  });
  wrap.appendChild(bar);
  const ends = document.createElement("div");
  ends.className = "pb-ends";
  ends.innerHTML = '<span class="pb-ws"></span><span class="pb-we"></span>';
  wrap.appendChild(ends);
  const row = document.createElement("div");
  row.className = "pb-row";
  row.innerHTML = '<span class="pb-at">—</span><button type="button" class="pb-download">Download…</button><span class="muted pb-note"></span>';
  row.querySelector(".pb-download").addEventListener("click", () => downloadForm(wrap, cam, tile));
  wrap.appendChild(row);
  return wrap;
}

// Draw the camera's recordings inside its bar's 12-hour window, and the cursor.
function drawBar(cell, cam, clock) {
  const bar = cell.querySelector(".pb-bar");
  if (!bar) return;
  const ws = windowStart(cam.device_id, clock);
  if (ws === null) return;
  if (bar.dataset.ws !== String(ws)) {
    bar.dataset.ws = String(ws);
    bar.innerHTML = "";
    const c = pbClips[cam.device_id];
    for (const clip of c ? c.clips : []) {
      if (clip.end <= ws || clip.start >= ws + WINDOW) continue;
      const a = Math.max(clip.start, ws), b = Math.min(clip.end, ws + WINDOW);
      const seg = document.createElement("div");
      seg.className = "clip" + (clip.video_type === 2 ? " event" : "");
      seg.style.left = `${((a - ws) / WINDOW) * 100}%`;
      seg.style.width = `${Math.max(0.15, ((b - a) / WINDOW) * 100)}%`;
      seg.title = `${clockText(clip.start)} – ${clockText(clip.end)}`;
      bar.appendChild(seg);
    }
    const pos = document.createElement("div");
    pos.className = "pos hidden";
    bar.appendChild(pos);
    cell.querySelector(".pb-ws").textContent = clockText(ws).replace(/:\d\d (AM|PM)$/, " $1");
    cell.querySelector(".pb-we").textContent = clockText(ws + WINDOW).replace(/:\d\d (AM|PM)$/, " $1");
  }
  const pos = bar.querySelector(".pos");
  if (pos && clock !== null && clock !== undefined && clock >= ws && clock <= ws + WINDOW) {
    pos.classList.remove("hidden");
    pos.style.left = `${((clock - ws) / WINDOW) * 100}%`;
  } else if (pos) {
    pos.classList.add("hidden");
  }
}

function fillTimeline(cell, cam) {
  const bar = cell.querySelector(".pb-bar");
  const note = cell.querySelector(".pb-note");
  if (!bar) return;
  delete bar.dataset.ws;
  drawBar(cell, cam, tileNow(cell.querySelector(".cam-tile")));
  const clips = pbClips[cam.device_id];
  if (!clips) { note.textContent = "no recording list from this camera"; return; }
  const total = clips.clips.reduce((a, c) => a + (c.end - c.start), 0);
  const card = clips.sd_card;
  note.textContent = clips.clips.length
    ? `${clips.clips.length} recordings, ${Math.round(total / 60)} min` + (card.state !== "normal" ? `; card ${card.state}` : "")
    : (card.state === "normal" ? "nothing recorded that day" : `SD card ${card.state}`);
}

function updatePosition(cell, cam, at) {
  drawBar(cell, cam, at);
  const label = cell.querySelector(".pb-at");
  if (label) label.textContent = clockText(Math.floor(at));
}

function playAll(root) {
  const from = chosenTime();
  if (from === null) { $("cam-message").textContent = "Pick a date first."; return; }
  startMaster(from);
  root.querySelectorAll(".cam-cell").forEach((cell) => {
    const cam = cameraById(cell.dataset.id);
    mountPlayback(cell.querySelector(".cam-tile"), cam, from);
  });
  updateSeekBar(from);
}

function playGroup(box) { playAll(box); }

function downloadForm(wrap, cam, tile) {
  let form = wrap.querySelector(".pb-form");
  if (form) { form.remove(); return; }
  form = document.createElement("div");
  form.className = "pb-form";
  const at = tileNow(tile) ?? chosenTime();
  const start = at !== null ? clock24(at) : ($("pb-time").value || "10:00:00");
  form.innerHTML = `from <input type="time" class="pb-from" value="${start}" step="1"> for <input type="number" class="pb-mins" value="10" min="1" max="720"> min <button type="button" class="pb-save">Save as MP4…</button><span class="muted pb-dl-note"></span>`;
  form.querySelector(".pb-save").addEventListener("click", async () => {
    const clips = pbClips[cam.device_id];
    if (!clips) return;
    const [hh, mm, ss] = form.querySelector(".pb-from").value.split(":").map(Number);
    const from = clips.day_start + hh * 3600 + mm * 60 + (ss || 0);
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
    n.textContent = p.error ? p.error : `saving… ${human(p.bytes)}`;
  });
});

// ---- controls ----

$("cameras-open").addEventListener("click", showCameras);
$("cam-layout").addEventListener("change", () => $("cam-grid").style.setProperty("--cols", $("cam-layout").value));
$("cam-mode").addEventListener("change", () => { if (camStatus && camStatus.running) renderCameraGrid(); });
$("cam-reload-all").addEventListener("click", () => { if (camStatus && camStatus.running) renderCameraGrid(); });
$("cam-when").addEventListener("change", () => { if (camStatus && camStatus.running) renderCameraGrid(); });
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
