// The Tools page: Tidy, Crop and Compress for any PDF, JPEG or PNG on this
// device, no printer needed. The file goes on the "tools" workbench
// (bench.rs) and the panel is the same one scans use (bench.js); Save makes
// a new file, and the original stays as it was.

(() => {
  const onPhone = /Android|iPhone|iPad/.test(navigator.userAgent);
  const say = (text, cls) => {
    $("tools-note").textContent = text || "";
    $("tools-note").className = "small" + (cls ? " " + cls : "");
  };
  let fileName = "";

  const panel = KagazBench.attach($("tools-bench"), {
    bench: "tools",
    finish: [{
      label: "Save…",
      accent: true,
      // As a new file: the format Compress made, else the file's own.
      run: async (view, { compressed }) => {
        let ext = { Pdf: "pdf", Jpeg: "jpg", Png: "png" }[compressed] || (fileName.split(".").pop() || "").toLowerCase();
        if (ext === "jpeg") ext = "jpg";
        if (!["pdf", "jpg", "png"].includes(ext)) ext = "pdf";
        const base = fileName.replace(/\.[^.]+$/, "") || "kagaz";
        const out = await invoke("choose_save_path", { suggestedName: `${base}-edited.${ext}`, extension: ext });
        if (!out) return "";
        const written = await invoke("bench_save", { bench: "tools", out, maxBytes: null });
        return written.length ? `Saved ${written.map((w) => w.path).join(", ")}.` : "";
      },
    }],
  });

  async function open(path, name) {
    fileName = name.replace(/^.*[\\/]/, "");
    say("Opening " + fileName + "…", "working");
    try {
      const view = await invoke("bench_open_file", { bench: "tools", path, name: fileName });
      $("tools-bench").classList.remove("hidden");
      panel.fresh(view);
      say("");
    } catch (e) {
      say("Could not open it: " + e, "cap-warn");
    }
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
      await open(await invoke("upload_document", { ext, data }), file.name);
    } catch (e) {
      say("Could not read " + file.name + ": " + e, "cap-warn");
    }
  });

  $("tools-open").addEventListener("click", async () => {
    if (onPhone) return picker.click();
    try {
      const path = await invoke("choose_print_file");
      if (path) await open(path, path);
    } catch (e) {
      say("Could not open the file chooser: " + e, "cap-warn");
    }
  });
})();
