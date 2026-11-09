// The society's gate (society.js): visitors through MyGate, vehicles
// through Park+. Each signs in with the phone number its own app uses and
// a code sent to it. Signing in through Kagaz comes once it can talk to
// the service; until then "See a sample" fills the page with invented
// entries, marked as such, so the look and the flow can be judged.
// Uses app.js's globals: $, invoke; ask.js's KagazAsk; brands.js's KagazBrands.

const KagazSociety = (() => {
  let tab = "visitors";
  const visitors = { signedIn: false, sample: false, pending: [], future: [], activity: [] };
  const vehicles = { signedIn: false, sample: false, list: [], moves: [], alerts: true, filter: "all" };

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function button(cls, text, onClick, label) {
    const b = el("button", cls, text);
    b.type = "button";
    if (label) b.setAttribute("aria-label", label);
    b.addEventListener("click", onClick);
    return b;
  }
  const minutes = (n) => Date.now() - n * 60 * 1000;
  function when(t) {
    const d = new Date(t);
    const today = new Date();
    const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    if (d.toDateString() === today.toDateString()) return time;
    const y = new Date(today); y.setDate(y.getDate() - 1);
    if (d.toDateString() === y.toDateString()) return `Yesterday ${time}`;
    return d.toLocaleDateString([], { day: "numeric", month: "short" }) + " " + time;
  }
  function waiting(t) {
    const s = Math.max(0, Math.round((Date.now() - t) / 1000));
    return s < 60 ? `waiting ${s} s` : `waiting ${Math.round(s / 60)} min`;
  }
  const code6 = () => String(Math.floor(100000 + Math.random() * 900000));

  const ICON = {
    guest: '<svg viewBox="0 0 24 24"><path d="M12 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21v-1a8 8 0 0 1 16 0v1"/></svg>',
    delivery: '<svg viewBox="0 0 24 24"><path d="M3 7h11v9H3zM14 10h4l3 3v3h-7zM7 19a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3zM17 19a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z"/></svg>',
    cab: '<svg viewBox="0 0 24 24"><path d="M5 11l1.5-4.5h11L19 11M4 11h16v6H4zM7 17v2M17 17v2M7.5 14h.01M16.5 14h.01"/></svg>',
    help: '<svg viewBox="0 0 24 24"><path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.5 2.5-2.4-.6-.6-2.4z"/></svg>',
    car: '<svg viewBox="0 0 24 24"><path d="M5 11l1.5-4.5h11L19 11M4 11h16v6H4zM7 17v2M17 17v2M7.5 14h.01M16.5 14h.01"/></svg>',
    in: '<svg viewBox="0 0 24 24"><path d="M4 12h12M11 7l5 5-5 5M20 4v16"/></svg>',
    out: '<svg viewBox="0 0 24 24"><path d="M20 12H8M13 7l-5 5 5 5M4 4v16"/></svg>',
  };
  const KIND_WORDS = { guest: "Guest", delivery: "Delivery", cab: "Cab", help: "Visiting help" };

  // ---------- sample entries (invented; marked on the page) ----------

  function sampleVisitors() {
    visitors.pending = [
      { id: "s1", kind: "delivery", name: "Ravi", company: "Amazon", gate: "Gate 1", guard: "Suresh", vehicle: "", since: minutes(0.7) },
    ];
    visitors.future = [
      { id: "f1", kind: "guest", name: "Meera and Arjun", code: "482913", until: "today, 10 pm" },
      { id: "f2", kind: "help", name: "Lakshmi (cook)", code: "", until: "every day, 7 to 9 am" },
    ];
    visitors.activity = [
      { kind: "cab", who: "Uber cab for Priya", what: "allowed by you", at: minutes(35) },
      { kind: "delivery", who: "Swiggy delivery", what: "left at the gate · code 731056", at: minutes(140) },
      { kind: "guest", who: "Unknown guest", what: "denied: not expected", at: minutes(60 * 26) },
      { kind: "help", who: "Lakshmi (cook)", what: "entered with a pre-approval", at: minutes(60 * 27) },
    ];
  }
  function sampleVehicles() {
    vehicles.list = [
      { id: "v1", plate: "KA 01 ZZ 0001", name: "Grey hatchback", tag: "FASTag", locked: false },
      { id: "v2", plate: "KA 01 ZZ 0002", name: "Blue scooter", tag: "Park+ sticker", locked: true },
    ];
    vehicles.moves = [
      { plate: "KA 01 ZZ 0001", dir: "in", gate: "Gate 2", at: minutes(25) },
      { plate: "KA 01 ZZ 0001", dir: "out", gate: "Gate 1", at: minutes(60 * 3) },
      { plate: "KA 01 ZZ 0002", dir: "in", gate: "Gate 1", at: minutes(60 * 20) },
      { plate: "KA 01 ZZ 0002", dir: "out", gate: "Gate 1", at: minutes(60 * 28) },
    ];
  }

  // ---------- visitors (MyGate) ----------

  function actRow(iconHtml, who, what, time) {
    const row = el("button", "act-item");
    row.type = "button";
    row.disabled = true;
    const pic = el("span", "act-pic icon");
    pic.innerHTML = iconHtml;
    const text = el("span", "act-text");
    text.append(el("strong", "", who), el("span", "muted small", what));
    row.append(pic, text, el("span", "act-time muted small", time));
    return row;
  }

  function pendingCard(v) {
    const card = el("article", "gate-card");
    card.setAttribute("aria-label", `${v.name}, ${KIND_WORDS[v.kind]}, at ${v.gate}`);
    const head = el("div", "gate-head");
    const pic = el("span", "gate-pic");
    pic.innerHTML = ICON[v.kind];
    const text = el("div", "gate-text");
    text.append(el("strong", "", v.name));
    text.append(el("span", "muted small", [KIND_WORDS[v.kind] + (v.company ? ` · ${v.company}` : ""), v.vehicle].filter(Boolean).join(" · ")));
    text.append(el("span", "muted small", `${v.gate} · ${v.guard} · ${waiting(v.since)}`));
    head.append(pic, text);
    card.appendChild(head);
    const row = el("div", "gate-actions");
    row.appendChild(button("accent big", "Allow", () => decide(v, "allowed by you")));
    row.appendChild(button("danger big", "Deny", () => deny(v)));
    if (v.kind === "delivery") row.appendChild(button("big", "Leave at gate", () => leaveAtGate(v)));
    card.appendChild(row);
    return card;
  }

  function decide(v, what) {
    visitors.pending = visitors.pending.filter((x) => x.id !== v.id);
    visitors.activity.unshift({ kind: v.kind, who: v.company ? `${v.company} ${KIND_WORDS[v.kind].toLowerCase()}` : v.name, what, at: Date.now() });
    renderVisitors();
    if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
  }

  async function deny(v) {
    const reason = await KagazAsk.ask((dlg, finish) => {
      const form = document.createElement("form");
      form.method = "dialog";
      form.appendChild(el("p", "ask-title", `Deny ${v.name}?`));
      form.appendChild(el("p", "muted small", "The guard sees the reason."));
      const list = el("div", "ask-choices");
      for (const [i, r] of ["Not expected", "Wrong flat", "Not now, ask them to wait", "I do not know them"].entries()) {
        const label = el("label", "ask-choice");
        const input = document.createElement("input");
        input.type = "radio"; input.name = "why"; input.value = r; input.checked = i === 0;
        label.append(input, " " + r);
        list.appendChild(label);
      }
      form.appendChild(list);
      const row = el("div", "actions");
      row.appendChild(button("", "Keep waiting", () => finish(null)));
      const act = el("button", "danger", "Deny"); act.type = "submit";
      row.appendChild(act);
      form.appendChild(row);
      form.addEventListener("submit", (e) => { e.preventDefault(); finish(new FormData(form).get("why")); });
      dlg.appendChild(form);
    });
    if (reason) decide(v, `denied: ${reason.toLowerCase()}`);
  }

  async function leaveAtGate(v) {
    const code = code6();
    await KagazAsk.ask((dlg, finish) => {
      const form = document.createElement("form");
      form.method = "dialog";
      form.appendChild(el("p", "ask-title", "Left at the gate"));
      form.appendChild(el("p", "", "The guard keeps the parcel. Show this code to collect it:"));
      form.appendChild(el("p", "gate-code", code));
      const row = el("div", "actions");
      const act = el("button", "accent", "Done"); act.type = "submit";
      row.appendChild(act);
      form.appendChild(row);
      form.addEventListener("submit", (e) => { e.preventDefault(); finish(true); });
      dlg.appendChild(form);
    });
    decide(v, `left at the gate · code ${code}`);
  }

  // Allow future entry: a guest gets a code to show the guard; a delivery,
  // cab or helper is let in when it matches what was said.
  async function allowFuture(kind) {
    const fields = {
      guest: [["name", "Who is coming", "text", "Name"], ["until", "Valid", "select", ["Today", "Tomorrow", "This week"]]],
      delivery: [["name", "From", "select", ["Amazon", "Flipkart", "Swiggy", "Zomato", "Blinkit", "Another company"]], ["until", "Valid", "select", ["Next 2 hours", "Today", "Tomorrow"]]],
      cab: [["name", "Company", "select", ["Uber", "Ola", "Rapido", "Another company"]], ["until", "Valid", "select", ["Next hour", "Next 2 hours", "Today"]]],
      help: [["name", "Who", "text", "Name and what they do (Lakshmi, cook)"], ["until", "Days and hours", "text", "Every day, 7 to 9 am"]],
    }[kind];
    const got = await KagazAsk.ask((dlg, finish) => {
      const form = document.createElement("form");
      form.method = "dialog";
      form.appendChild(el("p", "ask-title", `Allow a ${KIND_WORDS[kind].toLowerCase()} in`));
      for (const [name, label, type, extra] of fields) {
        const l = el("label", "", label + " ");
        let input;
        if (type === "select") {
          input = document.createElement("select");
          for (const o of extra) { const op = el("option", "", o); op.value = o; input.appendChild(op); }
        } else {
          input = document.createElement("input");
          input.type = "text"; input.placeholder = extra; input.required = true; input.autocomplete = "off";
        }
        input.name = name;
        l.appendChild(input);
        form.appendChild(l);
      }
      const row = el("div", "actions");
      row.appendChild(button("", "Cancel", () => finish(null)));
      const act = el("button", "accent", "Allow"); act.type = "submit";
      row.appendChild(act);
      form.appendChild(row);
      form.addEventListener("submit", (e) => { e.preventDefault(); finish(Object.fromEntries(new FormData(form))); });
      dlg.appendChild(form);
      setTimeout(() => form.querySelector("input, select").focus(), 0);
    });
    if (!got) return;
    visitors.future.unshift({ id: "f" + Date.now(), kind, name: got.name, code: kind === "guest" ? code6() : "", until: got.until.toLowerCase() });
    renderVisitors();
  }

  function futureRow(f) {
    const row = el("div", "gate-future");
    const pic = el("span", "act-pic icon");
    pic.innerHTML = ICON[f.kind];
    const text = el("span", "act-text");
    text.append(el("strong", "", `${f.name} · ${KIND_WORDS[f.kind].toLowerCase()}`));
    text.append(el("span", "muted small", (f.code ? `Code ${f.code} · ` : "") + `valid ${f.until}`));
    row.append(pic, text, button("small", "Cancel", () => { visitors.future = visitors.future.filter((x) => x.id !== f.id); renderVisitors(); }, `Cancel the entry for ${f.name}`));
    return row;
  }

  function renderVisitors() {
    const on = visitors.signedIn || visitors.sample;
    $("mg-signin").classList.toggle("hidden", on);
    $("mg-main").classList.toggle("hidden", !on);
    $("mg-sample").classList.toggle("hidden", !visitors.sample);
    if (!on) return;
    $("mg-pending").replaceChildren(...visitors.pending.map(pendingCard));
    $("mg-pending-empty").classList.toggle("hidden", visitors.pending.length > 0);
    $("mg-future").replaceChildren(...visitors.future.map(futureRow));
    $("mg-future-empty").classList.toggle("hidden", visitors.future.length > 0);
    $("mg-activity").replaceChildren(...visitors.activity.map((a) => actRow(ICON[a.kind], a.who, a.what, when(a.at))));
  }

  // ---------- vehicles (Park+) ----------

  function vehicleCard(v) {
    const card = el("article", "gate-card vehicle");
    const head = el("div", "gate-head");
    const pic = el("span", "gate-pic");
    pic.innerHTML = ICON.car;
    const text = el("div", "gate-text");
    text.append(el("strong", "plate", v.plate), el("span", "muted small", `${v.name} · ${v.tag}`));
    head.append(pic, text);
    card.appendChild(head);
    const row = el("div", "gate-actions");
    const lock = el("label", "fan-switch", "Tag lock ");
    const input = document.createElement("input");
    input.type = "checkbox"; input.className = "toggle"; input.setAttribute("role", "switch"); input.checked = v.locked;
    input.setAttribute("aria-label", `Tag lock for ${v.plate}`);
    input.title = "Locked, the tag opens no barrier until you unlock it: the car cannot leave on its own tag.";
    input.addEventListener("change", () => { v.locked = input.checked; renderVehicles(); });
    lock.appendChild(input);
    row.appendChild(lock);
    row.appendChild(el("span", "muted small", v.locked ? "Locked: the barrier stays shut for this tag" : "The barrier opens for this tag"));
    row.appendChild(button("small danger-text", "Remove", async () => {
      if (await KagazAsk.confirm(`Remove ${v.plate}?`, "Remove vehicle", "Its tag stays with the society; it is just not listed here.")) {
        vehicles.list = vehicles.list.filter((x) => x.id !== v.id);
        renderVehicles();
      }
    }));
    card.appendChild(row);
    return card;
  }

  async function addVehicle() {
    const got = await KagazAsk.ask((dlg, finish) => {
      const form = document.createElement("form");
      form.method = "dialog";
      form.appendChild(el("p", "ask-title", "Add a vehicle"));
      for (const [name, label, placeholder] of [["plate", "Number plate", "KA 01 AB 1234"], ["name", "Call it", "Grey hatchback"]]) {
        const l = el("label", "", label + " ");
        const input = document.createElement("input");
        input.type = "text"; input.name = name; input.placeholder = placeholder; input.required = name === "plate"; input.autocomplete = "off";
        if (name === "plate") input.style.textTransform = "uppercase";
        l.appendChild(input);
        form.appendChild(l);
      }
      const l = el("label", "", "Tag ");
      const sel = document.createElement("select"); sel.name = "tag";
      for (const o of ["FASTag", "Park+ sticker"]) { const op = el("option", "", o); op.value = o; sel.appendChild(op); }
      l.appendChild(sel);
      form.appendChild(l);
      const row = el("div", "actions");
      row.appendChild(button("", "Cancel", () => finish(null)));
      const act = el("button", "accent", "Add"); act.type = "submit";
      row.appendChild(act);
      form.appendChild(row);
      form.addEventListener("submit", (e) => { e.preventDefault(); finish(Object.fromEntries(new FormData(form))); });
      dlg.appendChild(form);
      setTimeout(() => form.querySelector("input").focus(), 0);
    });
    if (!got) return;
    vehicles.list.push({ id: "v" + Date.now(), plate: got.plate.toUpperCase().trim(), name: got.name.trim() || "Vehicle", tag: got.tag, locked: false });
    renderVehicles();
  }

  function renderVehicles() {
    const on = vehicles.signedIn || vehicles.sample;
    $("pp-signin").classList.toggle("hidden", on);
    $("pp-main").classList.toggle("hidden", !on);
    $("pp-sample").classList.toggle("hidden", !vehicles.sample);
    if (!on) return;
    $("pp-vehicles").replaceChildren(...vehicles.list.map(vehicleCard));
    $("pp-vehicles-empty").classList.toggle("hidden", vehicles.list.length > 0);
    for (const c of $("pp-filters").querySelectorAll(".chip")) c.classList.toggle("active", c.dataset.filter === vehicles.filter);
    const moves = vehicles.moves.filter((m) => vehicles.filter === "all" || m.dir === vehicles.filter);
    $("pp-moves").replaceChildren(...moves.map((m) => actRow(ICON[m.dir], m.plate, `${m.dir === "in" ? "Entered" : "Left"} by ${m.gate}`, when(m.at))));
    $("pp-alerts").checked = vehicles.alerts;
  }

  // ---------- signing in, the tabs, the page ----------

  // A phone number, then the code the service sends to it. Through Kagaz
  // this comes once it can talk to the service; the form says so.
  function signIn(prefix, service, note) {
    const phone = $(`${prefix}-phone`);
    const code = $(`${prefix}-code`);
    const codeRow = $(`${prefix}-code-row`);
    $(`${prefix}-form`).addEventListener("submit", (e) => {
      e.preventDefault();
      if (codeRow.classList.contains("hidden")) {
        if (!/^\+?\d[\d ]{7,}$/.test(phone.value.trim())) {
          note.textContent = "Enter the phone number your " + service + " account uses.";
          phone.focus();
          return;
        }
        codeRow.classList.remove("hidden");
        $(`${prefix}-submit`).textContent = "Sign in";
        note.textContent = `Sending a code through Kagaz comes once it can talk to ${service}. Use the ${service} app meanwhile, or See a sample to look around.`;
        code.focus();
        return;
      }
      note.textContent = `Signing in through Kagaz comes once it can talk to ${service}.`;
    });
  }

  function showTab(which) {
    tab = which;
    for (const b of $("society-tabs").querySelectorAll(".tab")) {
      const on = b.dataset.tab === which;
      b.classList.toggle("active", on);
      b.setAttribute("aria-pressed", String(on));
    }
    $("society-visitors").classList.toggle("hidden", which !== "visitors");
    $("society-vehicles").classList.toggle("hidden", which !== "vehicles");
  }

  function show() {
    showTab(tab);
    renderVisitors();
    renderVehicles();
  }

  // Opened from Add a device with a brand chosen.
  function open(which) {
    tab = which;
    show();
  }

  // For Home: what is at the gate, when signed in (or looking at the sample).
  function homeState() {
    const parts = [];
    if (visitors.signedIn || visitors.sample) parts.push(visitors.pending.length ? `${visitors.pending.length} at the gate` : "Nobody at the gate");
    if (vehicles.signedIn || vehicles.sample) parts.push(`${vehicles.list.length} vehicle${vehicles.list.length === 1 ? "" : "s"}`);
    return parts.length ? { status: parts.join(" · ") + (visitors.sample || vehicles.sample ? " (sample)" : ""), pending: visitors.pending } : null;
  }

  for (const b of $("society-tabs").querySelectorAll(".tab")) b.addEventListener("click", () => showTab(b.dataset.tab));
  signIn("mg", "MyGate", $("mg-note"));
  signIn("pp", "Park+", $("pp-note"));
  $("mg-sample-on").addEventListener("click", () => { visitors.sample = true; sampleVisitors(); renderVisitors(); if (typeof KagazHome !== "undefined") KagazHome.devicesChanged(); });
  $("mg-sample-off").addEventListener("click", () => { visitors.sample = false; visitors.pending = []; visitors.future = []; visitors.activity = []; renderVisitors(); if (typeof KagazHome !== "undefined") KagazHome.devicesChanged(); });
  $("pp-sample-on").addEventListener("click", () => { vehicles.sample = true; sampleVehicles(); renderVehicles(); if (typeof KagazHome !== "undefined") KagazHome.devicesChanged(); });
  $("pp-sample-off").addEventListener("click", () => { vehicles.sample = false; vehicles.list = []; vehicles.moves = []; renderVehicles(); if (typeof KagazHome !== "undefined") KagazHome.devicesChanged(); });
  for (const b of $("mg-future-buttons").querySelectorAll("button")) b.addEventListener("click", () => allowFuture(b.dataset.kind));
  $("pp-add").addEventListener("click", addVehicle);
  for (const c of $("pp-filters").querySelectorAll(".chip")) c.addEventListener("click", () => { vehicles.filter = c.dataset.filter; renderVehicles(); });
  $("pp-alerts").addEventListener("change", () => { vehicles.alerts = $("pp-alerts").checked; });
  // The waiting times tick while someone is at the gate.
  setInterval(() => { if (visitors.pending.length && !$("society").classList.contains("hidden")) renderVisitors(); }, 10000);

  return { show, open, homeState, decide };
})();
