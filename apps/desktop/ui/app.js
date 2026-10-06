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
  if (camStatus.running) renderCameraGrid();
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

// Release a tile's player and its connection before replacing it.
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
}

function mountTile(tile, cam, big) {
  teardownTile(tile);
  const mode = $("cam-mode").value;
  let player;
  if (mode === "ts") {
    // MPEG-TS over HTTP in a plain <video>: measured as the route this
    // webview decodes at real time (its MP4 input is refused).
    player = document.createElement("video");
    player.autoplay = true;
    player.muted = true;
    player.playsInline = true;
    player.src = big ? cam.ts_url : cam.ts_url_vga;
  } else {
    player = document.createElement("video-stream");
    player.setAttribute("mode", mode);
    player.src = big ? cam.ws_url : cam.ws_url_vga;
  }
  tile.insertBefore(player, tile.firstChild);
  setState(tile, "connecting", "connecting…");
  // Watch the clock: if it stops advancing, say so over the last frame.
  let lastT = -1, stalledSince = 0, started = Date.now();
  tile._watch = setInterval(() => {
    const v = tile.querySelector("video") || tile.querySelector("video-stream")?.video;
    if (!v) return;
    const t = v.currentTime || 0;
    const now = Date.now();
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

function setState(tile, state, text) {
  tile.dataset.state = state;
  const o = tile.querySelector(".cam-overlay");
  if (!o) return;
  o.querySelector(".cam-overlay-text").textContent = text;
  o.classList.toggle("hidden", state === "live");
}

async function renderCameraGrid() {
  const grid = $("cam-grid");
  grid.style.setProperty("--cols", $("cam-layout").value);
  if ($("cam-mode").value !== "ts") {
    try {
      await loadPlayer(camStatus.player_script);
    } catch (e) {
      $("cam-message").textContent = e.message;
      return;
    }
  }
  grid.querySelectorAll(".cam-tile").forEach(teardownTile);
  grid.innerHTML = "";
  for (const cam of camStatus.cameras) {
    const tile = document.createElement("div");
    tile.className = "cam-tile";
    const name = document.createElement("div");
    name.className = "cam-name";
    name.textContent = cam.name;
    tile.appendChild(name);
    const overlay = document.createElement("div");
    overlay.className = "cam-overlay hidden";
    overlay.innerHTML = '<div class="spinner"></div><div class="cam-overlay-text"></div><button type="button" class="cam-reload">Reload</button>';
    overlay.querySelector(".cam-reload").addEventListener("click", (ev) => {
      ev.stopPropagation();
      mountTile(tile, cam, tile.classList.contains("big"));
    });
    tile.appendChild(overlay);
    mountTile(tile, cam, false);
    tile.addEventListener("click", (ev) => {
      if (ev.target.closest(".cam-reload")) return;
      const big = !tile.classList.contains("big");
      tile.classList.toggle("big", big);
      mountTile(tile, cam, big);
    });
    grid.appendChild(tile);
  }
  $("cam-state").textContent = `${camStatus.cameras.length} cameras`;
}

$("cameras-open").addEventListener("click", showCameras);
$("cam-layout").addEventListener("change", () => $("cam-grid").style.setProperty("--cols", $("cam-layout").value));
$("cam-mode").addEventListener("change", () => { if (camStatus && camStatus.running) renderCameraGrid(); });
$("cam-reload-all").addEventListener("click", () => { if (camStatus && camStatus.running) renderCameraGrid(); });
$("cam-start").addEventListener("click", async () => {
  $("cam-start").disabled = true;
  $("cam-state").textContent = "starting…";
  try {
    camStatus = await invoke("cameras_start");
    $("cam-message").textContent = "";
    $("cam-start").classList.add("hidden");
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
