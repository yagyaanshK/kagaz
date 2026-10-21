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
      $("tidy-what").textContent = `${chosen.name}${r.pages > 1 ? ` · ${r.pages} pages (the first shown)` : ""} · tap to see it full size`;
      showResult(true);
      say(r.notes.join(" "), r.notes.length ? "cap-warn" : "");
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
  $("tidy-discard").addEventListener("click", () => {
    chosen = null;
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
      const saved = await invoke("tidy_save", { out });
      say("Saved " + saved + ".", "cap-ok");
    } catch (e) {
      say("Could not save it: " + e, "cap-warn");
    }
  });
})();
