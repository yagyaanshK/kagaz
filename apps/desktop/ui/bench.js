// The tools panel, the same wherever there is something to work on: a
// scan, an ID page, a file on this device, later a camera's photo. The
// pictures sit on a workbench in Rust (bench.rs), one per place; the tools
// are kagaz_core::tools. A place adds the panel and puts its pictures on
// its bench; that is all.
//
//   const panel = KagazBench.attach(box, {
//     bench: "scan",                       // which bench
//     paper: () => "A4",                   // the page Crop's Fill fills
//     finish: [{ label: "Save…", accent: true,
//                run: async (view) => ({ text: "Saved …", path }) }],  // or just the text
//   });
//   panel.show(view);   // after putting pictures on the bench
//
// Tidy: Clean background, Find the card (frames to adjust). Crop: a frame,
// then just the area (default), fill the page, or keep the page. Compress:
// under a size. Undo goes back one tool; Start over to how it came. A
// saved file can be shown in its folder (Downloads on a phone).

const KagazBench = (() => {
  const onPhone = /Android|iPhone|iPad/i.test(navigator.userAgent);
  // Crop's shapes: [value, label]; "mm:w:h" also makes it that true size,
  // "r:w:h" only that shape.
  const SHAPES = [
    ["Photos, at their true size", [
      ["mm:35:45", "Passport photo, 35 × 45 mm"],
      ["mm:50.8:50.8", "US passport or visa, 2 × 2 in"],
      ["mm:20:25", "Stamp size, 20 × 25 mm"],
      ["mm:101.6:152.4", "Photo print, 4 × 6 in"],
      ["mm:152.4:101.6", "Photo print, 6 × 4 in"],
    ]],
    ["Screens", [["r:16:9", "16:9 wide"], ["r:9:16", "9:16 tall"], ["r:4:3", "4:3"], ["r:3:4", "3:4"], ["r:1:1", "1:1 square"]]],
    ["Paper", [
      ["r:210:297", "A4, upright"], ["r:297:210", "A4, sideways"],
      ["r:8.5:11", "Letter, upright"], ["r:11:8.5", "Letter, sideways"],
    ]],
  ];
  const SIZES = [["200000", "200 KB"], ["500000", "500 KB"], ["1000000", "1 MB"], ["2000000", "2 MB"], ["5000000", "5 MB"]];

  function attach(box, { bench, paper = () => "A4", finish = [] }) {
    box.classList.add("bench");
    box.innerHTML = `
      <p class="bench-what small muted"></p>
      <div class="bench-pages hidden"><button type="button" class="bench-prev" aria-label="Previous page">&#x2039;</button><span class="bench-page small"></span><button type="button" class="bench-next" aria-label="Next page">&#x203A;</button></div>
      <button type="button" class="bench-look" aria-label="See it full size"><img alt=""></button>
      <div class="bench-tools" role="toolbar" aria-label="Tools">
        <button type="button" data-tool="tidy" aria-pressed="false">Tidy</button>
        <button type="button" data-tool="crop" aria-pressed="false">Crop</button>
        <button type="button" data-tool="compress" aria-pressed="false">Compress</button>
        <button type="button" class="bench-undo">Undo</button>
        <button type="button" class="bench-reset">Start over</button>
      </div>
      <div class="bench-panel" data-panel="tidy" hidden>
        <label class="switch"><input type="checkbox" class="bench-clean"> Clean background (the lid's grey becomes white)</label>
        <label class="switch"><input type="checkbox" class="bench-card"> Find the card (white outside it; frames to adjust)</label>
        <div class="actions"><button type="button" class="bench-tidy-go accent">Tidy</button></div>
        <div class="bench-frames hidden">
          <p class="small muted">Drag each frame's corners onto its card, and a side's middle to bend it. Remove a frame (&#x2715;) for a one-sided ID.</p>
          <div class="bench-crop-editor"></div>
          <div class="actions"><button type="button" class="bench-frames-go accent">Use these frames</button></div>
        </div>
      </div>
      <div class="bench-panel" data-panel="crop" hidden>
        <div class="bench-row">
          <label>Shape <select class="bench-shape">
            <option value="free">Free: drag each corner</option>
            <option value="lock">Lock the shape it has now</option>
            ${SHAPES.map(([group, list]) => `<optgroup label="${group}">${list.map(([v, t]) => `<option value="${v}">${t}</option>`).join("")}</optgroup>`).join("")}
            <option value="custom">Your own size or shape…</option>
          </select></label>
        </div>
        <div class="bench-row bench-custom hidden">
          <label>Width <input type="number" class="bench-custom-w" min="0.1" step="any" inputmode="decimal" value="35"></label>
          <label>Height <input type="number" class="bench-custom-h" min="0.1" step="any" inputmode="decimal" value="45"></label>
          <label>In <select class="bench-custom-unit"><option value="mm">mm (true size)</option><option value="in">inches (true size)</option><option value="r">a shape only (like 3 : 2)</option></select></label>
        </div>
        <p class="bench-shape-note small muted">Drag the frame's corners around what to keep (a photo, a signature), or drag inside it to move it. A tilted frame comes out level, along its bottom side.</p>
        <div class="bench-crop-editor"></div>
        <fieldset class="bench-modes"><legend class="small">Then</legend>
          <label class="switch"><input type="radio" name="${bench}-crop-mode" value="Area" checked> Just the cropped area, at its real size</label>
          <label class="switch"><input type="radio" name="${bench}-crop-mode" value="FillPage"> Enlarge it to fill the page, without stretching</label>
          <label class="switch"><input type="radio" name="${bench}-crop-mode" value="KeepPage"> Keep the whole page, white outside the frame</label>
        </fieldset>
        <div class="actions"><button type="button" class="bench-crop-go accent">Crop</button></div>
      </div>
      <div class="bench-panel" data-panel="compress" hidden>
        <div class="bench-row">
          <label>Keep under <select class="bench-size">${SIZES.map(([v, t]) => `<option value="${v}" ${v === "1000000" ? "selected" : ""}>${t}</option>`).join("")}</select></label>
          <label>As <select class="bench-format"><option value="Pdf">PDF</option><option value="Jpeg">JPEG</option><option value="Png">PNG</option></select></label>
        </div>
        <div class="actions"><button type="button" class="bench-compress-go accent">Make it smaller</button></div>
      </div>
      <div class="actions bench-finish"></div>
      <p class="bench-note small" role="status"></p>
      <div class="actions bench-saved hidden"><button type="button" class="bench-show">${onPhone ? "Show in Downloads" : "Show in folder"}</button></div>`;
    const q = (s) => box.querySelector(s);
    let view = null;
    let page = 0;
    let busy = false;
    let tidyCrop = null;
    let cropCrop = null;
    let saved = null;

    const savedAt = (path) => {
      saved = path || null;
      q(".bench-saved").classList.toggle("hidden", !saved);
    };
    const say = (text, cls) => {
      q(".bench-note").textContent = text || "";
      q(".bench-note").className = "bench-note small" + (cls ? " " + cls : "");
    };
    const human = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.round(n / 1e3)} KB`);

    function show(v) {
      view = v;
      page = v.page;
      q(".bench-look img").src = v.view;
      q(".bench-what").textContent = `${v.name}${v.pages > 1 ? ` · ${v.pages} pages` : ""} · tap the picture to see it full size`;
      q(".bench-pages").classList.toggle("hidden", v.pages < 2);
      q(".bench-page").textContent = `Page ${v.page + 1} of ${v.pages}`;
      q(".bench-prev").disabled = v.page === 0;
      q(".bench-next").disabled = v.page >= v.pages - 1;
      q(".bench-undo").disabled = !v.can_undo;
      if (v.size != null) say(`About ${human(v.size)} as ${q(".bench-format").selectedOptions[0].text}.${v.warning ? " " + v.warning : ""}`, v.warning ? "cap-warn" : "cap-ok");
      else if (v.notes && v.notes.length) say(v.notes.join(" "), "cap-warn");
    }

    // Run a bench command, showing what it gives back.
    async function run(cmd, args, working) {
      if (busy) return null;
      busy = true;
      savedAt(null);
      box.querySelectorAll("button").forEach((b) => (b.dataset.was = b.disabled, b.disabled = true));
      say(working, "working");
      try {
        const v = await invoke(cmd, { bench, page, ...args });
        say("");
        show(v);
        return v;
      } catch (e) {
        say(String(e), "cap-warn");
        return null;
      } finally {
        busy = false;
        box.querySelectorAll("button").forEach((b) => { b.disabled = b.dataset.was === "true"; });
        if (view) show(view);
      }
    }

    // Crop's shape: (width / height, or "lock", or null for free; the true
    // size in mm, if it has one).
    function shapeChosen() {
      const v = q(".bench-shape").value;
      q(".bench-custom").classList.toggle("hidden", v !== "custom");
      if (v === "free") return { ratio: null, mm: null };
      if (v === "lock") return { ratio: "lock", mm: null };
      let kind, w, h;
      if (v === "custom") {
        kind = q(".bench-custom-unit").value;
        [w, h] = [Number(q(".bench-custom-w").value), Number(q(".bench-custom-h").value)];
        if (!(w > 0 && h > 0)) return null;
        if (kind === "in") [kind, w, h] = ["mm", w * 25.4, h * 25.4];
      } else {
        [kind, w, h] = v.split(":").map((x, i) => (i ? Number(x) : x));
      }
      return { ratio: w / h, mm: kind === "mm" ? [w, h] : null };
    }
    const fmm = (n) => (Math.round(n * 10) / 10).toString();
    function shapeNote(chosen) {
      const base = "Drag the frame's corners around what to keep (a photo, a signature), or drag inside it to move it. A tilted frame comes out level, along its bottom side.";
      q(".bench-shape-note").textContent = !chosen
        ? "Give a width and a height above zero."
        : chosen.mm
          ? `The frame keeps this shape: its corners resize it, the round handle turns it, and inside it moves it. It comes out at exactly ${fmm(chosen.mm[0])} × ${fmm(chosen.mm[1])} mm, to print at its true size (Fill the page makes it larger).`
          : chosen.ratio
            ? "The frame keeps this shape: its corners resize it, the round handle turns it, and inside it moves it. It comes out level."
            : base;
    }
    function applyShape() {
      const chosen = shapeChosen();
      shapeNote(chosen);
      if (chosen && cropCrop) cropCrop.setRatio(chosen.ratio);
    }
    q(".bench-shape").addEventListener("change", applyShape);
    for (const sel of [".bench-custom-w", ".bench-custom-h", ".bench-custom-unit"]) q(sel).addEventListener("change", applyShape);

    // One tool's panel open at a time.
    function open(tool) {
      for (const b of box.querySelectorAll("[data-tool]")) {
        const on = b.dataset.tool === tool && b.getAttribute("aria-pressed") !== "true";
        b.setAttribute("aria-pressed", String(on));
        box.querySelector(`[data-panel="${b.dataset.tool}"]`).hidden = !on;
        if (on && tool === "crop") startCrop();
      }
    }
    box.querySelectorAll("[data-tool]").forEach((b) => b.addEventListener("click", () => open(b.dataset.tool)));

    async function startCrop() {
      const v = await run("bench_crop_start", {}, "Looking for what is on the page…");
      if (!v) return;
      const chosen = shapeChosen();
      shapeNote(chosen);
      cropCrop = KagazCrop.attach(q('[data-panel="crop"] .bench-crop-editor'), {
        src: v.view, outlines: v.frames, radius: 0, names: ["crop"], ratio: chosen ? chosen.ratio : null, removable: false,
      });
    }

    q(".bench-show").addEventListener("click", () => {
      if (saved) invoke("show_in_folder", { path: saved }).catch((e) => say(String(e), "cap-warn"));
    });
    q(".bench-look").addEventListener("click", () => {
      if (view) KagazHistory.picture(view.name, view.view);
    });
    q(".bench-prev").addEventListener("click", () => run("bench_view", { page: page - 1 }, ""));
    q(".bench-next").addEventListener("click", () => run("bench_view", { page: page + 1 }, ""));
    q(".bench-undo").addEventListener("click", () => run("bench_undo", {}, "Undoing…"));
    q(".bench-reset").addEventListener("click", () => run("bench_reset", {}, "Starting over…"));

    q(".bench-tidy-go").addEventListener("click", async () => {
      const tidy = { background: q(".bench-clean").checked, card: q(".bench-card").checked };
      if (!tidy.background && !tidy.card) return say("Tick Clean background, Find the card, or both.", "cap-warn");
      const v = await run("bench_tidy", { tidy }, "Tidying…");
      q(".bench-frames").classList.toggle("hidden", !(v && v.frames.length));
      if (v && v.frames.length) {
        tidyCrop = KagazCrop.attach(q('[data-panel="tidy"] .bench-crop-editor'), {
          src: v.before || v.view, outlines: v.frames, radius: v.radius,
          names: v.frames.length === 2 ? ["front", "back"] : ["card"],
        });
        const missed = v.frames.map((f, i) => (f.found ? null : v.frames.length === 2 ? ["top", "bottom"][i] + " half" : "picture")).filter(Boolean);
        if (missed.length) say(`No card stood out in the ${missed.join(" or the ")}; its frame covers it all, so drag its corners onto the card.`, "cap-warn");
      }
    });
    q(".bench-frames-go").addEventListener("click", async () => {
      if (!tidyCrop) return;
      const v = await run("bench_frames", { shapes: tidyCrop.quads(), background: q(".bench-clean").checked }, "Applying the frames…");
      if (v) q(".bench-frames").classList.add("hidden");
    });
    q(".bench-crop-go").addEventListener("click", async () => {
      if (!cropCrop) return;
      const [shape] = cropCrop.quads();
      const chosen = shapeChosen();
      if (!chosen) return say("Give the shape a width and a height above zero.", "cap-warn");
      const mode = q(`input[name="${bench}-crop-mode"]:checked`).value;
      const v = await run("bench_crop", { shape, mode, paper: paper(), sizeMm: chosen.mm }, "Cropping…");
      if (v) open("crop"); // closes the panel
    });
    q(".bench-compress-go").addEventListener("click", () =>
      run("bench_compress", { format: q(".bench-format").value, maxBytes: Number(q(".bench-size").value) }, "Making it smaller…"));

    // The place's own ending: Save…, Print.
    for (const f of finish) {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = f.label;
      if (f.accent) b.className = "accent";
      b.addEventListener("click", async () => {
        if (busy || !view) return;
        busy = true;
        try {
          const done = await f.run(view, { compressed: view.size != null ? q(".bench-format").value : null });
          const text = typeof done === "string" ? done : done && done.text;
          if (text) say(text, "cap-ok");
          savedAt(done && done.path);
        } catch (e) {
          say(String(e), "cap-warn");
        }
        busy = false;
      });
      q(".bench-finish").appendChild(b);
    }

    return {
      show,
      // A new picture on the bench: tools closed, frames gone.
      fresh(v) {
        for (const b of box.querySelectorAll("[data-tool]")) {
          b.setAttribute("aria-pressed", "false");
          box.querySelector(`[data-panel="${b.dataset.tool}"]`).hidden = true;
        }
        q(".bench-frames").classList.add("hidden");
        say("");
        savedAt(null);
        show(v);
      },
      say,
    };
  }

  return { attach };
})();
