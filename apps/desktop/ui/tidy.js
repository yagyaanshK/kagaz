// Tidy a saved scan or photo, no printer needed (Printers & scanners page):
// Clean background and Find the card, as for an ID scan (idscan.js), on a
// PDF, JPEG or PNG already on this device. The result shows first; Save
// writes it as a new file in the same format, and the original stays as it
// was (printing.rs, tidy_preview and tidy_save).

(() => {
  const onPhone = /Android|iPhone|iPad/.test(navigator.userAgent);
  const note = $("tidy-note");
  const say = (text, cls) => {
    note.textContent = text;
    note.className = "small" + (cls ? " " + cls : "");
  };
  let chosen = null; // { path, name }
  let view = null; // the full-size picture of the result
  let crop = null; // the frames on the first page (Find the card), adjustable
  let busy = false;

  const tidy = () => ({ background: $("tidy-clean").checked, card: $("tidy-card").checked });
  const nothingTicked = () => !$("tidy-clean").checked && !$("tidy-card").checked;

  function showResult(on) {
    $("tidy-result").classList.toggle("hidden", !on);
    if (!on) view = null;
  }

  // Tidy the chosen file and show the result (again when a choice changes).
  async function preview() {
    if (!chosen || busy) return;
    if (nothingTicked()) {
      showResult(false);
      return say("Tick Clean background, Find the card, or both.", "cap-warn");
    }
    busy = true;
    $("tidy-go").disabled = $("tidy-save").disabled = true;
    say("Tidying " + chosen.name + "…", "working");
    try {
      const r = await invoke("tidy_preview", { path: chosen.path, tidy: tidy() });
      view = r.view;
      $("tidy-look").querySelector("img").src = r.thumb;
      const pages = r.pages > 1 ? ` · ${r.pages} pages (the first shown)` : "";
      // Find the card: the page with a frame per card, to adjust by hand.
      const framed = r.outlines.length > 0;
      crop = framed
        ? KagazCrop.attach($("tidy-crop"), {
            src: r.original,
            outlines: r.outlines,
            names: r.outlines.length === 2 ? ["front", "back"] : ["card"],
          })
        : null;
      $("tidy-crop").classList.toggle("hidden", !framed);
      $("tidy-look").classList.toggle("hidden", framed);
      $("tidy-result-see").classList.toggle("hidden", !framed);
      $("tidy-what").textContent = framed
        ? `${chosen.name}${pages} · drag each frame's corners onto its card; outside the frames turns white. Remove a frame for a one-sided ID.`
        : `${chosen.name}${pages} · tap to see it full size`;
      showResult(true);
      if (framed) {
        const missed = r.outlines.map((o, i) => (o.found ? null : r.outlines.length === 2 ? ["top", "bottom"][i] : "picture")).filter(Boolean);
        say(missed.length ? `No card stood out in the ${missed.join(" or ")} half; its frame covers it all, so drag its corners onto the card.`.replace("picture half", "picture") : "", missed.length ? "cap-warn" : "");
      } else {
        say(r.notes.join(" "), r.notes.length ? "cap-warn" : "");
      }
    } catch (e) {
      showResult(false);
      say("Could not tidy it: " + e, "cap-warn");
    }
    busy = false;
    $("tidy-go").disabled = $("tidy-save").disabled = false;
  }

  function choose(path, name) {
    chosen = { path, name: name.replace(/^.*[\\/]/, "") };
    preview();
  }

  // A phone hands the page a file's contents, not a path (as on the Print tab).
  const picker = document.createElement("input");
  picker.type = "file";
  picker.accept = ".pdf,.jpg,.jpeg,.png,application/pdf,image/jpeg,image/png";
  picker.addEventListener("change", async () => {
    const file = picker.files && picker.files[0];
    picker.value = "";
    if (!file) return;
    say("Reading " + file.name + "…", "working");
    try {
      const data = await new Promise((resolve, reject) => {
        const r = new FileReader();
        r.onload = () => resolve(String(r.result).replace(/^data:[^,]*,/, ""));
        r.onerror = () => reject(r.error || "the phone would not hand over the file");
        r.readAsDataURL(file);
      });
      const ext = (file.name.split(".").pop() || "").toLowerCase();
      choose(await invoke("upload_document", { ext, data }), file.name);
    } catch (e) {
      say("Could not read " + file.name + ": " + e, "cap-warn");
    }
  });

  $("tidy-go").addEventListener("click", async () => {
    if (nothingTicked()) return say("Tick Clean background, Find the card, or both.", "cap-warn");
    if (onPhone) return picker.click();
    try {
      const path = await invoke("choose_print_file");
      if (path) choose(path, path);
    } catch (e) {
      say("Could not open the file chooser: " + e, "cap-warn");
    }
  });
  for (const id of ["tidy-clean", "tidy-card"]) $(id).addEventListener("change", preview);

  $("tidy-look").addEventListener("click", () => {
    if (view) KagazHistory.picture("Tidied: " + chosen.name, view);
  });
  // The frames as set now, applied to the first page.
  async function applyFrames() {
    return invoke("tidy_outlines", { outlines: crop.quads(), background: $("tidy-clean").checked });
  }
  $("tidy-result-see").addEventListener("click", async () => {
    if (!crop || busy) return;
    try {
      KagazHistory.picture("Tidied: " + chosen.name, await applyFrames());
    } catch (e) {
      say("Could not tidy it: " + e, "cap-warn");
    }
  });
  $("tidy-discard").addEventListener("click", () => {
    chosen = null;
    crop = null;
    $("tidy-crop").classList.add("hidden");
    showResult(false);
    say("");
  });
  $("tidy-save").addEventListener("click", async () => {
    if (!chosen || busy) return;
    let ext = (chosen.name.split(".").pop() || "").toLowerCase();
    if (ext === "jpeg") ext = "jpg";
    if (!["pdf", "jpg", "png"].includes(ext)) ext = "pdf";
    const base = chosen.name.replace(/\.[^.]+$/, "");
    try {
      const out = await invoke("choose_save_path", { suggestedName: `${base}-tidied.${ext}`, extension: ext });
      if (!out) return;
      say("Saving…", "working");
      if (crop) await applyFrames();
      const saved = await invoke("tidy_save", { out });
      say("Saved " + saved + ".", "cap-ok");
    } catch (e) {
      say("Could not save it: " + e, "cap-warn");
    }
  });
})();
