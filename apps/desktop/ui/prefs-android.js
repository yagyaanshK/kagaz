// Settings rows as Android's own Settings app draws them (Material 3), on
// Android only. settings.js builds every row the same way on every system:
// a label and its note, and a control (a list to pick from, a number with
// an Apply button). On Android:
// - a row with a list shows its title and, under it, the choice made; a
//   tap opens a dialog with the choices as radio buttons (Android's list
//   preference). For a tone, choosing one plays it, as Android's ringtone
//   picker does, and OK keeps it; the row's own Listen button goes.
// - a row with a number and Apply shows the number under its title; a tap
//   opens a dialog with the field (Android's text preference).
// The note moves into the dialog. The <select> and fields stay, hidden,
// and get the same events as before, so settings.js need not know.
// (Reported 2026-11-19: doorbell settings still looked like a computer's.)

(() => {
  const root = document.documentElement;
  if (root.dataset.os !== "android") return;
  const main = document.getElementById("main");
  if (!main) return;

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  let ids = 0;
  let open = null; // the dialog shown: { finish }

  // A Material basic dialog: headline, supporting text, body, text buttons.
  // `finish(ok)` closes it; Back (KagazNav) and the scrim cancel.
  function dialog(title, note, body, buttons, onClose) {
    if (open) open.finish(false);
    const scrim = el("div", "md-scrim");
    const box = el("div", "md-dialog");
    box.setAttribute("role", "dialog");
    box.setAttribute("aria-modal", "true");
    const h = el("h2", "md-headline", title);
    h.id = `md-dialog-${++ids}`;
    box.setAttribute("aria-labelledby", h.id);
    box.appendChild(h);
    if (note) box.appendChild(el("p", "md-support", note));
    box.appendChild(body);
    const actions = el("div", "md-actions");
    let done = false;
    const finish = (ok) => {
      if (done) return;
      done = true;
      scrim.remove();
      if (open && open.finish === finish) open = null;
      if (typeof KagazNav !== "undefined") KagazNav.pop("pref-dialog");
      onClose(ok);
    };
    for (const [text, ok] of buttons) {
      const b = el("button", "md-text", text);
      b.type = "button";
      b.addEventListener("click", () => finish(ok));
      actions.appendChild(b);
    }
    box.appendChild(actions);
    scrim.appendChild(box);
    scrim.addEventListener("click", (ev) => { if (ev.target === scrim) finish(false); });
    document.body.appendChild(scrim);
    open = { finish };
    if (typeof KagazNav !== "undefined") KagazNav.push("pref-dialog", () => finish(false));
    return finish;
  }

  const titleOf = (r) => (r.querySelector(".set-label > span:first-child") || {}).textContent || "";
  const noteOf = (r) => r.querySelector(".set-label > .pref-note");

  // The row's note moves into its dialog; the row shows `value` instead.
  function asPref(r, value) {
    r.classList.add("pref-row");
    const label = r.querySelector(".set-label");
    const note = label.querySelector(":scope > .muted.small:not(.set-error)");
    if (note) note.classList.add("pref-note");
    const shown = el("span", "pref-value");
    label.appendChild(shown);
    const show = () => {
      const now = value();
      if (shown.textContent !== now) shown.textContent = now;
    };
    show();
    // A save that fails puts the old value back without an event, and says
    // so in the label: show what is there then.
    new MutationObserver(show).observe(label, { childList: true });
    r.tabIndex = 0;
    r.setAttribute("role", "button");
    return show;
  }

  function onTap(r, openIt) {
    r.addEventListener("click", (ev) => {
      ev.preventDefault(); // the label would open the hidden field's own picker
      openIt();
    });
    r.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" || ev.key === " ") {
        ev.preventDefault();
        openIt();
      }
    });
  }

  function listRow(r, sel) {
    const choice = () => (sel.options[sel.selectedIndex] || {}).textContent || "";
    const show = asPref(r, choice);
    sel.addEventListener("change", () => setTimeout(show, 0));
    const play = r.querySelector(".play-tone");
    const tone = !!play && typeof KagazRing !== "undefined";
    onTap(r, () => {
      const before = sel.value;
      let chosen = before;
      const list = el("div", "md-list");
      list.setAttribute("role", "radiogroup");
      const name = `md-choice-${++ids}`;
      let finish = null;
      for (const o of sel.options) {
        const item = el("label", "md-radio");
        const radio = el("input");
        radio.type = "radio";
        radio.name = name;
        radio.value = o.value;
        radio.checked = o.value === before;
        item.append(radio, el("span", "", o.textContent));
        radio.addEventListener("change", () => {
          chosen = o.value;
          if (tone) {
            sel.value = chosen; // what the Listen button plays
            KagazRing.stop();
            play.click();
          } else if (finish) {
            finish(true);
          }
        });
        list.appendChild(item);
      }
      const note = noteOf(r);
      finish = dialog(titleOf(r), note ? note.textContent : "", list, tone ? [["Cancel", false], ["OK", true]] : [["Cancel", false]], (ok) => {
        if (tone) KagazRing.stop();
        sel.value = before;
        if (ok && chosen !== before) {
          sel.value = chosen;
          sel.dispatchEvent(new Event("change", { bubbles: true }));
        }
        show();
      });
      const on = list.querySelector("input:checked") || list.querySelector("input");
      if (on) on.focus();
    });
  }

  function fieldRow(r, input, apply) {
    const show = asPref(r, () => input.value);
    input.addEventListener("change", show);
    apply.addEventListener("click", () => setTimeout(show, 0));
    onTap(r, () => {
      const body = el("div", "md-field");
      const field = el("input");
      field.type = input.type;
      for (const a of ["min", "max", "step", "inputmode"]) if (input.hasAttribute(a)) field.setAttribute(a, input.getAttribute(a));
      field.value = input.value;
      field.setAttribute("aria-label", titleOf(r));
      body.appendChild(field);
      field.addEventListener("keydown", (ev) => { if (ev.key === "Enter") finish(true); });
      const note = noteOf(r);
      const finish = dialog(titleOf(r), note ? note.textContent : "", body, [["Cancel", false], ["OK", true]], (ok) => {
        if (ok && field.value !== input.value) {
          input.value = field.value;
          input.dispatchEvent(new Event("input", { bubbles: true }));
          apply.click();
        }
        show();
      });
      field.focus();
      field.select();
    });
  }

  function enhance(r) {
    if (r.classList.contains("pref-row") || r.dataset.prefSeen) return;
    r.dataset.prefSeen = "1";
    if (!r.querySelector(":scope > .set-label")) return;
    const control = r.querySelector(":scope > .set-control");
    if (!control) return;
    const sel = control.querySelector("select");
    if (sel) {
      listRow(r, sel);
      return;
    }
    const inputs = control.querySelectorAll("input");
    const buttons = control.querySelectorAll("button");
    if (inputs.length === 1 && buttons.length === 1 && ["number", "text"].includes(inputs[0].type)) {
      fieldRow(r, inputs[0], buttons[0]);
    }
  }

  // Any other list to pick from (a camera's playback, a fan's timer, a
  // crop's shape, a sign-in's service): a Material field showing the choice
  // with a trailing arrow; a tap opens the same dialog of choices.
  function nameOf(sel) {
    if (sel.getAttribute("aria-label")) return sel.getAttribute("aria-label");
    if (sel.id) {
      const l = document.querySelector(`label[for="${CSS.escape(sel.id)}"]`);
      if (l) return l.textContent.trim();
    }
    const l = sel.closest("label");
    if (l) return l.textContent.replace(sel.textContent, "").trim();
    return sel.title || "Choose";
  }
  const fields = new Set();
  function field(sel) {
    if (sel.dataset.mdSelect) return;
    sel.dataset.mdSelect = "1";
    const b = el("button", "md-select");
    b.type = "button";
    const text = el("span", "md-select-text");
    const arrow = el("span", "md-select-arrow ic");
    arrow.innerHTML = typeof KagazIcons !== "undefined" ? KagazIcons.svg("down") : "";
    b.append(text, arrow);
    sel.after(b);
    sel.classList.add("md-hidden-select");
    const show = () => {
      const now = (sel.options[sel.selectedIndex] || {}).textContent || "";
      if (text.textContent !== now) {
        text.textContent = now;
        b.setAttribute("aria-label", `${nameOf(sel)}: ${now}`);
      }
      if (b.disabled !== sel.disabled) b.disabled = sel.disabled;
      const gone = sel.hidden || sel.classList.contains("hidden");
      if (b.hidden !== gone) b.hidden = gone;
    };
    show();
    fields.add([sel, show]);
    sel.addEventListener("change", () => setTimeout(show, 0));
    new MutationObserver(show).observe(sel, { childList: true, subtree: true, attributes: true });
    b.addEventListener("click", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      const before = sel.value;
      const list = el("div", "md-list");
      list.setAttribute("role", "radiogroup");
      const name = `md-choice-${++ids}`;
      let finish = null;
      for (const o of sel.options) {
        if (o.hidden) continue;
        const item = el("label", "md-radio");
        const radio = el("input");
        radio.type = "radio";
        radio.name = name;
        radio.value = o.value;
        radio.checked = o.value === before;
        radio.disabled = o.disabled;
        item.append(radio, el("span", "", o.textContent));
        radio.addEventListener("change", () => {
          if (o.value !== before) {
            sel.value = o.value;
            sel.dispatchEvent(new Event("input", { bubbles: true }));
            sel.dispatchEvent(new Event("change", { bubbles: true }));
          }
          if (finish) finish(true);
        });
        list.appendChild(item);
      }
      finish = dialog(nameOf(sel), "", list, [["Cancel", false]], () => show());
      const on = list.querySelector("input:checked") || list.querySelector("input");
      if (on) on.focus();
    });
  }
  // A choice set by the page itself (no event) shows within a moment.
  setInterval(() => {
    for (const entry of fields) {
      if (!entry[0].isConnected) fields.delete(entry);
      else entry[1]();
    }
  }, 700);

  const scan = () => {
    for (const r of document.querySelectorAll("#main .set-row")) enhance(r);
    for (const s of document.querySelectorAll("select")) {
      if (s.closest(".pref-row") || s.closest(".md-scrim")) continue;
      field(s);
    }
  };
  // Focus rings only once a keyboard is used (skins.css): a tap leaves no
  // ring behind.
  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Tab" || ev.key.startsWith("Arrow")) root.dataset.keys = "1";
  }, true);
  window.addEventListener("pointerdown", () => { delete root.dataset.keys; }, true);
  // The top app bar takes its scrolled-under colour while the page is
  // scrolled (skins.css).
  const scrolled = () => {
    const on = main.scrollTop > 0;
    if (on !== ("scrolled" in root.dataset)) {
      if (on) root.dataset.scrolled = "";
      else delete root.dataset.scrolled;
    }
  };
  main.addEventListener("scroll", scrolled, { passive: true });

  // Once a frame at most, however much the page changes.
  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      scan();
    });
  }).observe(document.body, { childList: true, subtree: true });
  scan();
})();
