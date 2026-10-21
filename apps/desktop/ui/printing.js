// Print and Copy on the device page. Printing goes to the printer without a
// driver (IPP Everywhere) or through one of the system's print queues (the
// installed driver). A copy scans the originals here, then prints them;
// "Scan and look first" shows the pages before anything is printed.
// Uses app.js's globals: $, invoke, listen, devices, has, title, human,
// describeEvent.

(() => {
  let device = null; // the device page's device
  let deviceIndex = null;
  let filePath = null;
  let queueInfo = null; // { queues, matched, available, system_dialog }
  let order = []; // scanned page indices, in print order
  let thumbs = new Map(); // index -> { src, width, height, dpi, keep }
  let busy = false;

  const PAPER_MEDIA = {
    A4: "iso_a4_210x297mm",
    Letter: "na_letter_8.5x11in",
    Legal: "na_legal_8.5x14in",
  };
  const MEDIA_NAME = {
    iso_a4_210x297mm: "A4",
    iso_a5_148x210mm: "A5",
    na_letter_8_5x11in: "Letter",
    "na_letter_8.5x11in": "Letter",
    "na_legal_8.5x14in": "Legal",
  };
  const SIDES_TEXT = {
    OneSided: "one side",
    TwoSidedLongEdge: "both sides",
    TwoSidedShortEdge: "both sides, short edge",
  };

  const printsDriverless = (d) => has(d, "Ipp");
  const scansHere = (d) => has(d, "Escl", "WsdScan", "SaneNet");

  // ---------- where a job goes ----------

  function targetChoices(forCopy) {
    const out = [];
    if (forCopy) {
      devices.forEach((d, i) => {
        if (!printsDriverless(d)) return;
        const name = i === deviceIndex ? "this printer" : title(d);
        out.push({ value: `driverless:${i}`, label: `${name}, no driver` });
      });
    } else if (device && printsDriverless(device)) {
      out.push({ value: `driverless:${deviceIndex}`, label: "Kagaz, no driver (IPP Everywhere)" });
    }
    if (queueInfo) {
      for (const q of queueInfo.queues) {
        const mine = q.name === queueInfo.matched ? " (this printer)" : "";
        out.push({ value: `queue:${q.name}`, label: `installed driver: ${q.description}${mine}` });
      }
      if (queueInfo.system_dialog) out.push({ value: "system", label: "the phone's print dialog" });
    }
    return out;
  }

  function preferred(choices, forCopy) {
    const here = choices.find((c) => c.value === `driverless:${deviceIndex}`);
    if (here) return here.value;
    if (queueInfo && queueInfo.matched) return `queue:${queueInfo.matched}`;
    if (choices.some((c) => c.value === "system")) return "system";
    return choices.length ? choices[0].value : "";
  }

  function fillTargets(sel, forCopy) {
    const before = sel.value;
    const choices = targetChoices(forCopy);
    sel.innerHTML = "";
    for (const c of choices) {
      const o = document.createElement("option");
      o.value = c.value;
      o.textContent = c.label;
      sel.appendChild(o);
    }
    sel.value = choices.some((c) => c.value === before) ? before : preferred(choices, forCopy);
    sel.disabled = choices.length === 0;
  }

  function target(value) {
    if (value.startsWith("driverless:")) return { via: "driverless", device: devices[Number(value.slice(11))] };
    if (value.startsWith("queue:")) return { via: "queue", name: value.slice(6) };
    return { via: "system" };
  }

  function targetNote() {
    const sel = $("print-target");
    const note = $("print-target-note");
    if (!sel.options.length) {
      note.textContent = queueInfo
        ? "This device takes no driverless printing and no print queue on this computer matches it. Installing its driver adds one."
        : "Looking for this computer's print queues…";
      return;
    }
    note.textContent = sel.value.startsWith("queue:")
      ? "Copies, sides, paper and colour go to the driver with this job; the driver's own settings stay as they are."
      : sel.value === "system"
        ? "Opens the phone's print dialog, which uses the print service installed for this printer."
        : "Kagaz prepares the pages itself and sends them straight to the printer.";
  }

  async function loadQueues() {
    try {
      queueInfo = await invoke("print_queues", { device });
    } catch (e) {
      queueInfo = { queues: [], matched: null, available: false, system_dialog: false };
      $("print-target-note").textContent = "Could not list this computer's print queues: " + e;
    }
    fillTargets($("print-target"), false);
    fillTargets($("copy-target"), true);
    targetNote();
  }

  // ---------- printer status ----------

  const STATE_TEXT = { Idle: "ready", Processing: "printing", Stopped: "stopped" };

  async function loadStatus(d, i) {
    const box = $("print-status");
    box.classList.add("hidden");
    if (!printsDriverless(d)) return;
    try {
      const s = await invoke("printer_status", { device: d });
      if (deviceIndex !== i) return;
      const pill = $("print-state-pill");
      const state = STATE_TEXT[s.state] || String(s.state).toLowerCase();
      pill.textContent = s.accepting_jobs ? state : state + ", not taking jobs";
      pill.className = "pill " + (s.state === "Stopped" || !s.accepting_jobs ? "off" : "on");
      const problems = (s.reasons || []).filter((r) => r.severity !== "report").map((r) => r.text);
      $("print-message").textContent = problems.length ? problems.join("; ") : s.message || "";
      const ul = $("print-supplies");
      ul.innerHTML = "";
      for (const sup of s.supplies || []) {
        if (sup.level == null || sup.level < 0) continue;
        const li = document.createElement("li");
        const low = sup.low_at != null && sup.level <= sup.low_at;
        li.innerHTML = `<span class="sup-name"></span><span class="sup-bar"><i></i></span><span class="sup-level"></span>`;
        li.querySelector(".sup-name").textContent = sup.name;
        li.querySelector(".sup-bar i").style.width = Math.min(100, sup.level) + "%";
        if (sup.color && /^#[0-9a-f]{6}$/i.test(sup.color)) li.querySelector(".sup-bar i").style.background = sup.color;
        li.querySelector(".sup-level").textContent = sup.level + "%" + (low ? " · low" : "");
        if (low) li.classList.add("low");
        ul.appendChild(li);
      }
      if (s.media_default) {
        const first = $("print-media").options[0];
        first.textContent = `the printer's default (${MEDIA_NAME[s.media_default] || s.media_default})`;
      }
      box.classList.remove("hidden");
    } catch (e) {
      // The About tab already says when the printer cannot be reached.
    }
  }

  // ---------- progress ----------

  function logTo(id, text, cls, replaceKey) {
    const ul = $(id);
    let li = replaceKey ? ul.querySelector(`li[data-key="${replaceKey}"]`) : null;
    if (!li) {
      li = document.createElement("li");
      if (replaceKey) li.dataset.key = replaceKey;
      ul.appendChild(li);
    }
    li.textContent = text;
    li.className = cls || "";
    // Outcomes can land below the fold on a phone; bring them into view.
    if (cls === "ok" || cls === "bad") li.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }

  function describeProgress(p) {
    if (p.stage === "note") return [p.text];
    if (p.stage === "scan") return describeEvent(p);
    switch (p.kind) {
      case "planned": {
        const plan = p.plan;
        const copies = plan.copies > 1 ? `, ${plan.copies} copies` : "";
        const media = MEDIA_NAME[plan.media] || plan.media;
        return [`${media} at ${plan.dpi} dpi, ${plan.gray ? "black and white" : "colour"}, ${SIDES_TEXT[plan.sides] || plan.sides}${copies}`];
      }
      case "rendering":
        return [`Preparing page ${p.page} of ${p.of}…`, "", "rendering"];
      case "validated":
        return ["The printer accepts these settings."];
      case "sending":
        return [`Sending ${human(p.bytes)}…`];
      case "submitted":
        return [`The printer took it: job ${p.job_id}, ${p.state}.`, "ok"];
      default:
        return [JSON.stringify(p)];
    }
  }

  listen("print-progress", (e) => {
    const [text, cls, key] = describeProgress(e.payload);
    logTo("print-log", text, cls, key);
  });
  listen("copy-progress", (e) => {
    const [text, cls, key] = describeProgress(e.payload);
    logTo("copy-log", text, cls, key);
  });

  // ---------- print ----------

  // Problems with the chosen file show right under the file chooser.
  const fileNote = document.createElement("p");
  fileNote.id = "print-file-note";
  fileNote.className = "file-note hidden";
  fileNote.setAttribute("role", "alert");
  $("print-drop").insertAdjacentElement("afterend", fileNote);

  function fileProblem(text) {
    fileNote.textContent = text;
    fileNote.classList.toggle("hidden", !text);
    if (text) fileNote.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }

  function setFile(path, shownName) {
    fileProblem("");
    filePath = path;
    const name = shownName || path.split(/[\\/]/).pop();
    $("print-file").textContent = name;
    $("print-file").title = path;
    $("print-file").classList.remove("muted");
  }

  // A phone hands the page a file's contents, not a path: read it here
  // and keep it in the app's cache.
  const onPhone = /Android|iPhone|iPad/.test(navigator.userAgent);
  const picker = document.createElement("input");
  picker.type = "file";
  picker.accept = ".pdf,.jpg,.jpeg,.png,application/pdf,image/jpeg,image/png";
  picker.addEventListener("change", async () => {
    const file = picker.files && picker.files[0];
    picker.value = "";
    if (!file) return;
    const ext = (file.name.split(".").pop() || "").toLowerCase();
    fileProblem("");
    filePath = null;
    $("print-file").textContent = "Reading " + file.name + "…";
    try {
      const data = await new Promise((resolve, reject) => {
        const r = new FileReader();
        r.onload = () => resolve(String(r.result).replace(/^data:[^,]*,/, ""));
        r.onerror = () => reject(r.error || "the phone would not hand over the file");
        r.readAsDataURL(file);
      });
      const path = await invoke("upload_document", { ext, data });
      setFile(path, file.name);
    } catch (e) {
      $("print-file").textContent = "Nothing chosen.";
      $("print-file").classList.add("muted");
      fileProblem("Could not read " + file.name + ": " + e);
    }
  });

  $("print-pick").addEventListener("click", async () => {
    if (onPhone) {
      picker.click();
      return;
    }
    try {
      const p = await invoke("choose_print_file");
      if (p) setFile(p);
    } catch (e) {
      fileProblem("Could not open the file chooser: " + e);
    }
  });

  listen("tauri://drag-drop", (e) => {
    if ($("tab-print").classList.contains("hidden")) return;
    const paths = (e.payload && e.payload.paths) || [];
    const p = paths.find((x) => /\.(pdf|jpe?g|png)$/i.test(x));
    if (p) setFile(p);
    else if (paths.length) fileProblem("Kagaz prints PDF, JPEG and PNG files.");
  });

  function printRequest(f) {
    return {
      copies: Math.max(1, Number(f.get("copies")) || 1),
      sides: f.get("sides"),
      media: f.get("media") || null,
      color: f.get("color") === "color",
      job_name: null,
      pages: (f.get("pages") || "").trim() || null,
    };
  }

  async function runPrint(dryRun) {
    if (busy || !device) return;
    if (!filePath) {
      fileProblem("Choose a document first.");
      return;
    }
    const sel = $("print-target");
    if (!sel.value) return;
    const f = new FormData($("print-form"));
    busy = true;
    $("print-log").innerHTML = "";
    $("print-go").disabled = $("print-check").disabled = true;
    $("print-state").textContent = dryRun ? "Checking…" : "Printing…";
    try {
      const sent = await invoke("print_file", {
        target: target(sel.value),
        path: filePath,
        request: printRequest(f),
        dryRun,
      });
      logTo("print-log", dryRun ? sent.label : "Sent: " + sent.label, "ok");
      $("print-state").textContent = "Done.";
    } catch (err) {
      logTo("print-log", "Failed: " + err, "bad");
      $("print-state").textContent = "";
    }
    $("print-go").disabled = $("print-check").disabled = false;
    busy = false;
  }

  $("print-form").addEventListener("submit", (ev) => {
    ev.preventDefault();
    runPrint(false);
  });
  $("print-check").addEventListener("click", () => runPrint(true));
  $("print-target").addEventListener("change", () => {
    targetNote();
    $("print-check").disabled = !$("print-target").value.startsWith("driverless:");
  });

  // ---------- copy ----------

  function copySettings() {
    const f = new FormData($("copy-form"));
    const engine = has(device, "Escl", "WsdScan") ? "Driverless" : "Driver";
    return {
      scan: {
        source: f.get("source"),
        dpi: Number(f.get("dpi")),
        color: f.get("color"),
        paper: f.get("paper"),
        engine,
      },
      print: {
        copies: Math.max(1, Number(f.get("copies")) || 1),
        sides: f.get("sides"),
        media: PAPER_MEDIA[f.get("paper")] || null,
        color: f.get("color") === "Color",
        job_name: "Kagaz copy",
        pages: null,
      },
      fit: f.get("fit"),
      target: f.get("target"),
    };
  }

  function copyBusy(on, text) {
    busy = on;
    for (const id of ["copy-go", "copy-preview", "copy-print", "copy-more", "copy-clear"]) $(id).disabled = on;
    $("copy-state").textContent = text || "";
    $("copy-state").classList.toggle("working", on);
    // While it scans, it can be stopped.
    $("copy-cancel").classList.toggle("hidden", !(on && /Scanning/.test(text || "")));
    $("copy-cancel").disabled = false;
  }
  $("copy-cancel").addEventListener("click", () => {
    $("copy-cancel").disabled = true;
    $("copy-state").textContent = "Cancelling…";
    invoke("scan_cancel").catch(() => {});
  });

  async function scanPages(keep) {
    const s = copySettings();
    const list = await invoke("copy_scan", { device, request: s.scan, keep });
    if (!keep) {
      order = [];
      thumbs = new Map();
    }
    for (const t of list) {
      if (!thumbs.has(t.index)) {
        thumbs.set(t.index, { ...t, keep: true });
        order.push(t.index);
      }
    }
    return s;
  }

  async function printPages(indices, s) {
    if (!s.target) throw "there is no printer to print on";
    return invoke("copy_print", {
      target: target(s.target),
      pages: indices,
      request: s.print,
      fit: s.fit,
    });
  }

  // ID card or passport page: both sides on one page, printed at real size.
  const copyId = KagazId.attach({
    form: $("copy-form"),
    kind: "copy",
    device: () => device,
    request: () => ({ ...copySettings().scan, source: "Glass", half_page: true }),
    finish: {
      label: "Print",
      run: async () => {
        const s = copySettings();
        await invoke("id_to_copy");
        order = [0];
        thumbs = new Map();
        const sent = await printPages([0], { ...s, fit: "ActualSize", print: { ...s.print, sides: "OneSided" } });
        return "Sent: " + sent.label;
      },
    },
  });

  $("copy-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    if (busy || !device || copyId.active()) return;
    $("copy-log").innerHTML = "";
    $("copy-pages").classList.add("hidden");
    copyBusy(true, "Scanning…");
    try {
      const s = await scanPages(false);
      copyBusy(true, "Printing…");
      const sent = await printPages(order.slice(), s);
      logTo("copy-log", `Copied ${order.length} page${order.length === 1 ? "" : "s"}: ${sent.label}`, "ok");
      copyBusy(false, "Done.");
    } catch (err) {
      if (/cancelled/.test(String(err))) logTo("copy-log", "Scan cancelled.");
      else logTo("copy-log", "Failed: " + err, "bad");
      copyBusy(false);
    }
  });

  async function preview(keep) {
    if (busy || !device) return;
    if (!keep) $("copy-log").innerHTML = "";
    copyBusy(true, "Scanning…");
    try {
      await scanPages(keep);
      renderThumbs();
      copyBusy(false, `${order.length} page${order.length === 1 ? "" : "s"} scanned.`);
    } catch (err) {
      if (/cancelled/.test(String(err))) logTo("copy-log", "Scan cancelled.");
      else logTo("copy-log", "Failed: " + err, "bad");
      copyBusy(false);
    }
  }

  $("copy-preview").addEventListener("click", () => preview(false));
  $("copy-more").addEventListener("click", () => preview(true));
  $("copy-clear").addEventListener("click", async () => {
    const n = order.length;
    if (n && !(await KagazAsk.confirm(`Discard ${n} scanned page${n === 1 ? "" : "s"}?`, "Discard pages", "They are not printed or saved."))) return;
    await invoke("copy_discard").catch(() => {});
    order = [];
    thumbs = new Map();
    $("copy-pages").classList.add("hidden");
    $("copy-log").innerHTML = "";
    $("copy-state").textContent = "";
  });
  $("copy-print").addEventListener("click", async () => {
    if (busy) return;
    const chosen = order.filter((i) => thumbs.get(i).keep);
    if (!chosen.length) {
      logTo("copy-log", "Tick at least one page.", "warn");
      return;
    }
    copyBusy(true, "Printing…");
    try {
      const sent = await printPages(chosen, copySettings());
      logTo("copy-log", `Printed ${chosen.length} page${chosen.length === 1 ? "" : "s"}: ${sent.label}`, "ok");
      copyBusy(false, "Done.");
    } catch (err) {
      if (/cancelled/.test(String(err))) logTo("copy-log", "Scan cancelled.");
      else logTo("copy-log", "Failed: " + err, "bad");
      copyBusy(false);
    }
  });

  let dragFrom = null;
  function renderThumbs() {
    const ol = $("copy-thumbs");
    ol.innerHTML = "";
    order.forEach((index, pos) => {
      const t = thumbs.get(index);
      const li = document.createElement("li");
      li.draggable = true;
      li.classList.toggle("dropped", !t.keep);
      li.innerHTML = `<button type="button" class="thumb-look"><img alt=""></button><label><input type="checkbox"> <span></span></label>`
        + `<span class="thumb-move"><button type="button" class="icon" data-by="-1">\u2190</button><button type="button" class="icon" data-by="1">\u2192</button></span>`;
      // Dragging needs a mouse; the arrows move a page with a finger or the keyboard.
      li.querySelectorAll(".thumb-move button").forEach((b) => {
        const by = Number(b.dataset.by);
        const to = pos + by;
        b.disabled = to < 0 || to >= order.length;
        b.title = by < 0 ? "Earlier" : "Later";
        b.setAttribute("aria-label", `Move page ${pos + 1} ${by < 0 ? "earlier" : "later"}`);
        b.addEventListener("click", () => {
          const [moved] = order.splice(pos, 1);
          order.splice(to, 0, moved);
          renderThumbs();
          const again = $("copy-thumbs").children[to];
          if (again) again.querySelector(`.thumb-move button[data-by="${by}"]`)?.focus();
        });
      });
      li.querySelector("img").src = t.src;
      // A tap shows the page full size.
      const look = li.querySelector(".thumb-look");
      look.setAttribute("aria-label", `See page ${pos + 1} full size`);
      look.addEventListener("click", async () => {
        try {
          KagazHistory.picture(`Page ${pos + 1}`, await invoke("copy_picture", { index }));
        } catch (e) {
          logTo("copy-log", String(e), "bad");
        }
      });
      li.querySelector("span").textContent = `Page ${pos + 1}`;
      const box = li.querySelector("input");
      box.checked = t.keep;
      box.addEventListener("change", () => {
        t.keep = box.checked;
        li.classList.toggle("dropped", !t.keep);
      });
      li.addEventListener("dragstart", () => {
        dragFrom = pos;
        li.classList.add("dragging");
      });
      li.addEventListener("dragend", () => li.classList.remove("dragging"));
      li.addEventListener("dragover", (ev) => ev.preventDefault());
      li.addEventListener("drop", (ev) => {
        ev.preventDefault();
        if (dragFrom === null || dragFrom === pos) return;
        const [moved] = order.splice(dragFrom, 1);
        order.splice(pos, 0, moved);
        dragFrom = null;
        renderThumbs();
      });
      ol.appendChild(li);
    });
    $("copy-pages").classList.toggle("hidden", order.length === 0);
  }

  // ---------- the device page ----------

  document.addEventListener("kagaz-device", (e) => {
    copyId.reset();
    device = e.detail.device;
    deviceIndex = e.detail.index;
    $("print-log").innerHTML = "";
    $("copy-log").innerHTML = "";
    $("print-state").textContent = "";
    $("copy-state").textContent = "";
    $("copy-pages").classList.add("hidden");
    order = [];
    thumbs = new Map();
    invoke("copy_discard").catch(() => {});
    const scannable = scansHere(device);
    $("copy-form").classList.toggle("hidden", !scannable);
    $("copy-unavailable").classList.toggle("hidden", scannable);
    $("copy-unavailable").textContent = scannable
      ? ""
      : "Copying needs the scanner of this device, and no scan service was seen on it. To copy, pick a device that scans.";
    $("print-check").disabled = false;
    fillTargets($("print-target"), false);
    fillTargets($("copy-target"), true);
    loadQueues();
    loadStatus(device, deviceIndex);
  });
})();
