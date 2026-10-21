// ID scan and ID copy, on the Scan and Copy tabs: both sides of an ID card
// or a passport page on one page, as a printer's "2 in 1 ID copy" makes it.
// The front is a scan of the top half of the glass; the card is turned over
// in the same place and the back scanned the same way; the page has the
// front's half above the back's, each just as it lay (printing.rs, id_*).
//
//   KagazId.attach({ form, kind: "scan" | "copy", device: () => d,
//                    request: () => scanRequest, finish: { label, run } })
//
// adds a "What to scan" choice to `form`; with "ID card or passport page"
// the form's own button gives way to three steps. `finish.run()` saves or
// prints the page once both sides are in.

const KagazId = (() => {
  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function attach({ form, kind, device, request, finish }) {
    const copy = kind === "copy";
    const grid = form.querySelector(".grid");

    // The choice, first in the form.
    const what = el("label", "", copy ? "What to copy" : "What to scan");
    const select = el("select");
    select.name = "what";
    for (const [value, text] of [["pages", "Pages"], ["id", "ID card or passport page: both sides on one page"]]) {
      const o = el("option", "", text);
      o.value = value;
      select.appendChild(o);
    }
    what.appendChild(select);
    grid.prepend(what);

    // The steps.
    const box = el("div", "id-steps hidden");
    box.innerHTML = `
      <ol>
        <li data-side="0">
          <p><strong>Front.</strong> Put the card face down on the glass, in the half by the corner mark (the arrow), near that corner. Close the lid.</p>
          <div class="id-row"><button type="button" class="accent id-scan">Scan the front</button><img class="id-thumb hidden" alt="The front, as scanned"></div>
        </li>
        <li data-side="1">
          <p><strong>Back.</strong> Turn the card over and put it back in the same place.</p>
          <div class="id-row"><button type="button" class="id-scan" disabled>Scan the back</button><img class="id-thumb hidden" alt="The back, as scanned"></div>
        </li>
        <li data-side="page">
          <p><strong>The page.</strong> The front in the top half, the back below, each where it lay on the glass.</p>
          <div class="id-row"><button type="button" class="id-finish" disabled></button><img class="id-thumb id-page hidden" alt="Both sides on one page"></div>
        </li>
      </ol>
      <p class="id-note small" role="status"></p>`;
    grid.after(box);
    const steps = box.querySelectorAll("li");
    const note = box.querySelector(".id-note");
    const finishButton = box.querySelector(".id-finish");
    finishButton.textContent = finish.label;
    let busy = false;

    const setBusy = (on, text) => {
      busy = on;
      note.textContent = text || "";
      note.classList.toggle("working", on);
      note.classList.remove("cap-warn", "cap-ok");
      box.querySelectorAll("button").forEach((b) => { if (on) b.disabled = true; });
      if (!on) refresh();
    };
    let have = [false, false, false];
    const refresh = () => {
      steps[0].querySelector(".id-scan").disabled = false;
      steps[1].querySelector(".id-scan").disabled = !have[0];
      finishButton.disabled = !have[2];
      steps[0].querySelector(".id-scan").textContent = have[0] ? "Scan the front again" : "Scan the front";
      steps[1].querySelector(".id-scan").textContent = have[1] ? "Scan the back again" : "Scan the back";
      steps[0].querySelector(".id-scan").classList.toggle("accent", !have[0]);
      steps[1].querySelector(".id-scan").classList.toggle("accent", have[0] && !have[1]);
      finishButton.classList.toggle("accent", have[2]);
    };
    const fail = (e) => {
      setBusy(false);
      note.textContent = String(e);
      note.classList.add("cap-warn");
    };
    const show = (img, thumb) => {
      img.src = thumb.src;
      img.classList.remove("hidden");
    };

    for (const side of [0, 1]) {
      steps[side].querySelector(".id-scan").addEventListener("click", async () => {
        if (busy) return;
        setBusy(true, side ? "Scanning the back…" : "Scanning the front…");
        try {
          const thumb = await invoke("id_scan_side", { device: device(), request: request(), side, copy });
          show(steps[side].querySelector(".id-thumb"), thumb);
          if (side === 0) {
            // A new front starts over.
            have = [true, false, false];
            steps[1].querySelector(".id-thumb").classList.add("hidden");
            steps[2].querySelector(".id-thumb").classList.add("hidden");
            setBusy(false, "Now turn the card over, in the same place.");
          } else {
            have[1] = true;
            setBusy(true, "Putting both sides on one page…");
            show(steps[2].querySelector(".id-thumb"), await invoke("id_page", { paper: request().paper }));
            have[2] = true;
            setBusy(false, "");
            finishButton.focus();
          }
        } catch (e) {
          fail(e);
        }
      });
    }
    finishButton.addEventListener("click", async () => {
      if (busy || !have[2]) return;
      setBusy(true, copy ? "Printing…" : "");
      try {
        const done = await finish.run();
        setBusy(false, done || "");
        if (done) note.classList.add("cap-ok");
      } catch (e) {
        fail(e);
      }
    });

    // Only A4 and Letter halve into a card's place; the glass is the source.
    const paper = form.querySelector('select[name="paper"]');
    const mode = () => {
      const on = select.value === "id";
      form.classList.toggle("id-mode", on);
      box.classList.toggle("hidden", !on);
      if (paper) {
        for (const o of paper.options) o.disabled = on && !["A4", "Letter"].includes(o.value);
        if (on && paper.selectedOptions[0].disabled) paper.value = "A4";
      }
      if (on) refresh();
    };
    select.addEventListener("change", mode);
    // Another device: start again.
    const reset = () => {
      have = [false, false, false];
      box.querySelectorAll(".id-thumb").forEach((i) => i.classList.add("hidden"));
      note.textContent = "";
      refresh();
    };
    mode();
    return { reset, active: () => select.value === "id" };
  }

  return { attach };
})();
