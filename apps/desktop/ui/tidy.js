// Tidy a saved scan or photo, no printer needed (Printers & scanners page):
// Clean background and Find the card, as for an ID scan (idscan.js), on a
// PDF, JPEG or PNG already on this device. The result is a new file in the
// same format; the original stays as it was (printing.rs, tidy_file).

(() => {
  const onPhone = /Android|iPhone|iPad/.test(navigator.userAgent);
  const note = $("tidy-note");
  const say = (text, cls) => {
    note.textContent = text;
    note.className = "small" + (cls ? " " + cls : "");
  };

  async function run(path, name) {
    const tidy = { background: $("tidy-clean").checked, card: $("tidy-card").checked };
    let ext = (name.split(".").pop() || "").toLowerCase();
    if (ext === "jpeg") ext = "jpg";
    if (!["pdf", "jpg", "png"].includes(ext)) ext = "pdf";
    const base = name.replace(/\.[^.]+$/, "").replace(/^.*[\\/]/, "");
    const out = await invoke("choose_save_path", { suggestedName: `${base}-tidied.${ext}`, extension: ext });
    if (!out) return say("");
    say("Tidying…", "working");
    $("tidy-go").disabled = true;
    try {
      const done = await invoke("tidy_file", { path, tidy, out });
      say(["Saved " + done.path + ".", ...done.notes].join(" "), done.notes.length ? "cap-warn" : "cap-ok");
    } catch (e) {
      say("Could not tidy it: " + e, "cap-warn");
    }
    $("tidy-go").disabled = false;
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
      await run(await invoke("upload_document", { ext, data }), file.name);
    } catch (e) {
      say("Could not read " + file.name + ": " + e, "cap-warn");
    }
  });

  $("tidy-go").addEventListener("click", async () => {
    if (!$("tidy-clean").checked && !$("tidy-card").checked) {
      say("Tick Clean background, Find the card, or both.", "cap-warn");
      return;
    }
    if (onPhone) {
      picker.click();
      return;
    }
    try {
      const path = await invoke("choose_print_file");
      if (path) await run(path, path);
    } catch (e) {
      say("Could not open the file chooser: " + e, "cap-warn");
    }
  });
})();
