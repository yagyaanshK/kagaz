// Questions the page asks, in place of the browser's confirm() and prompt()
// (OK and Cancel, with Enter on OK): the buttons say what they do, the safe
// one has the focus, and Escape or Back leaves things as they were.
//
//   if (await KagazAsk.confirm("Forget Side Door?", "Forget station")) …
//   const name = await KagazAsk.name("Name the group", "Downstairs", "Create group");

const KagazAsk = (() => {
  let open = null; // the dialog on screen

  function ask(build) {
    if (open) open.close();
    return new Promise((resolve) => {
      const dlg = document.createElement("dialog");
      dlg.className = "ask";
      let answer = null;
      const finish = (value) => {
        answer = value;
        dlg.close();
      };
      build(dlg, finish);
      if (dlg.classList.contains("sheet")) sheetGestures(dlg, finish);
      dlg.addEventListener("close", () => {
        // A dialog replaced by the next (a sheet showing what it found)
        // closes after that one opened: its Back level is the new one's
        // now, so only the one on screen gives it up. Otherwise Back went
        // past the new sheet and left the app (reported 2026-11-22).
        if (open === dlg) {
          open = null;
          KagazNav.pop("ask");
        }
        dlg.remove();
        resolve(answer);
      });
      document.body.appendChild(dlg);
      open = dlg;
      KagazNav.push("ask", () => dlg.close());
      dlg.showModal();
    });
  }

  // A bottom sheet goes as Android's and iOS's do: a tap on the dimmed
  // screen above it, or a drag down (from anywhere on it while its list is
  // at the top; a drag that starts upwards scrolls the list instead).
  function sheetGestures(dlg, finish) {
    dlg.addEventListener("click", (e) => {
      if (e.target !== dlg) return;
      const r = dlg.getBoundingClientRect();
      if (e.clientY < r.top || e.clientY > r.bottom || e.clientX < r.left || e.clientX > r.right) finish(null);
    });
    let startY = null;
    let lastY = 0;
    let lastT = 0;
    let speed = 0;
    let dragging = false;
    dlg.addEventListener("touchstart", (e) => {
      if (e.touches.length !== 1) return;
      startY = lastY = e.touches[0].clientY;
      lastT = e.timeStamp;
      speed = 0;
      dragging = false;
    }, { passive: true });
    dlg.addEventListener("touchmove", (e) => {
      if (startY === null) return;
      const y = e.touches[0].clientY;
      const dy = y - startY;
      if (!dragging) {
        if (Math.abs(dy) < 8) return;
        if (dy < 0 || dlg.scrollTop > 0) {
          startY = null;
          return;
        }
        dragging = true;
        dlg.style.transition = "none";
      }
      e.preventDefault();
      speed = (y - lastY) / Math.max(1, e.timeStamp - lastT);
      lastY = y;
      lastT = e.timeStamp;
      dlg.style.transform = `translateY(${Math.max(0, dy)}px)`;
    }, { passive: false });
    const end = () => {
      if (startY === null) return;
      const dy = lastY - startY;
      startY = null;
      if (!dragging) return;
      dragging = false;
      dlg.style.transition = "transform 200ms cubic-bezier(0.2, 0, 0, 1)";
      if (dy > dlg.offsetHeight * 0.3 || speed > 0.6) {
        dlg.style.transform = `translateY(${dlg.offsetHeight}px)`;
        setTimeout(() => finish(null), 180);
      } else {
        dlg.style.transform = "";
      }
    };
    dlg.addEventListener("touchend", end);
    dlg.addEventListener("touchcancel", end);
  }

  function buttons(dlg, keepLabel, actionLabel, danger, finish) {
    const row = document.createElement("div");
    row.className = "actions";
    const keep = document.createElement("button");
    keep.type = "button";
    keep.textContent = keepLabel;
    keep.addEventListener("click", () => finish(null));
    const act = document.createElement("button");
    act.type = "submit";
    act.textContent = actionLabel;
    act.className = danger ? "danger" : "accent";
    row.append(keep, act);
    dlg.appendChild(row);
    return { keep, act };
  }

  /** True when the person chose `actionLabel` (a destructive verb). */
  function confirm(text, actionLabel, detail = "", keepLabel = "Keep") {
    return ask((dlg, finish) => {
      const form = document.createElement("form");
      form.method = "dialog";
      const p = document.createElement("p");
      p.className = "ask-title";
      p.textContent = text;
      form.appendChild(p);
      if (detail) {
        const d = document.createElement("p");
        d.className = "muted small";
        d.textContent = detail;
        form.appendChild(d);
      }
      dlg.appendChild(form);
      const { keep } = buttons(form, keepLabel, actionLabel, true, finish);
      form.addEventListener("submit", (e) => {
        e.preventDefault();
        finish(true);
      });
      // Enter on the focused Keep keeps; the destructive button needs a deliberate press.
      setTimeout(() => keep.focus(), 0);
    }).then((v) => v === true);
  }

  /** The name typed, trimmed; null when cancelled. */
  function name(title, value, actionLabel) {
    return ask((dlg, finish) => {
      const form = document.createElement("form");
      form.method = "dialog";
      const label = document.createElement("label");
      label.className = "ask-title";
      label.textContent = title;
      const input = document.createElement("input");
      input.type = "text";
      input.value = value || "";
      input.autocomplete = "off";
      label.appendChild(input);
      const note = document.createElement("p");
      note.className = "small cap-warn";
      note.setAttribute("role", "alert");
      form.append(label, note);
      dlg.appendChild(form);
      buttons(form, "Cancel", actionLabel, false, finish);
      form.addEventListener("submit", (e) => {
        e.preventDefault();
        const v = input.value.trim();
        if (!v) {
          note.textContent = "Type a name.";
          input.focus();
          return;
        }
        finish(v);
      });
      setTimeout(() => {
        input.focus();
        input.select();
      }, 0);
    });
  }

  return { confirm, name, ask };
})();
