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
      dlg.addEventListener("close", () => {
        if (open === dlg) open = null;
        dlg.remove();
        KagazNav.pop("ask");
        resolve(answer);
      });
      document.body.appendChild(dlg);
      open = dlg;
      KagazNav.push("ask", () => dlg.close());
      dlg.showModal();
    });
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
