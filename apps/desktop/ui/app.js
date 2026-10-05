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
  return has(d, "Escl", "WsdScan");
}

async function select(i) {
  current = i;
  const d = devices[i];
  renderList();
  $("empty").classList.add("hidden");
  $("detail").classList.remove("hidden");
  $("d-title").textContent = title(d);
  $("d-sub").textContent = [d.manufacturer, d.model, where(d)].filter(Boolean).join(" · ");
  $("scan-log").innerHTML = "";
  $("scan-state").textContent = "";
  showTab("about");
  const scanTab = document.querySelector('.tab[data-tab="scan"]');
  scanTab.disabled = !canScan(d);
  scanTab.title = canScan(d) ? "" : "This device does not offer driverless scanning";

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

$("rescan").addEventListener("click", rescan);
$("scan-form").addEventListener("change", () => Pictures.update($("scan-form")));
Pictures.update($("scan-form"));
rescan();
