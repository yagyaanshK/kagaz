// Air purifiers (Philips): the purifiers the person added, each with
// whether it is on the network and how it talks, and the ones found
// nearby, not added yet. Finding works; switching a purifier and reading
// its air come once the protocol is checked against a real one, and the
// page says so rather than showing controls that do nothing.
// Uses app.js's globals: $, invoke; ask.js's KagazAsk; brands.js's KagazBrands.

const KagazPurifiers = (() => {
  let data = { purifiers: [] }; // the saved list
  let reachable = new Set(); // ids that answered in the last search
  let found = []; // nearby, not added
  let scanning = false;
  let lastScan = 0;

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

  function ago(seconds) {
    if (!seconds) return "not seen yet";
    const s = Math.max(0, Math.floor(Date.now() / 1000) - seconds);
    if (s < 90) return "seen just now";
    if (s < 3600) return `seen ${Math.round(s / 60)} min ago`;
    if (s < 86400) return `seen ${Math.round(s / 3600)} h ago`;
    return `seen ${Math.round(s / 86400)} d ago`;
  }

  // How it talks, in words.
  function talks(p) {
    return p.protocol === "coap" ? "talks CoAP (purifiers from 2020 on)"
      : p.protocol === "http" ? "talks HTTP (older purifiers)"
        : "how it talks is not known yet";
  }

  // For the Home tile.
  function homeWords(p) {
    return reachable.has(p.id) ? "On the network" : ago(p.last_seen).replace(/^seen/, "Last seen").replace(/^not/, "Not");
  }

  function card(p) {
    const li = el("li", "purifier-card" + (reachable.has(p.id) ? "" : " unreachable"));
    li.dataset.id = p.id;
    li.appendChild(el("div", "name", p.name));
    li.appendChild(el("div", "where", [p.model, p.address].filter(Boolean).join(" · ")));
    const state = el("div", "caps");
    state.appendChild(el("span", reachable.has(p.id) ? "cap-ok" : "muted", reachable.has(p.id) ? "On the network" : ago(p.last_seen)));
    state.append(" · ", el("span", "muted", talks(p)));
    li.appendChild(state);
    const row = el("div", "actions");
    row.appendChild(button("", "Rename", async () => {
      const name = await KagazAsk.name("Name the purifier", p.name, "Rename");
      if (!name || name === p.name) return;
      try {
        data = await invoke("purifier_rename", { id: p.id, name });
      } catch (e) {
        $("purifiers-note").textContent = "Could not rename it: " + e;
      }
      render();
      if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
    }));
    row.appendChild(button("danger-text", "Remove", async () => {
      if (!(await KagazAsk.confirm(`Remove ${p.name} from your purifiers?`, "Remove purifier", "It stays as it is; a search finds it again, and Add brings it back."))) return;
      try {
        data = await invoke("purifier_forget", { id: p.id });
      } catch (e) {
        $("purifiers-note").textContent = "Could not remove it: " + e;
      }
      render();
      if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
    }));
    li.appendChild(row);
    return li;
  }

  function foundCard(f) {
    const li = el("li", "purifier-card");
    li.appendChild(el("div", "name", f.name));
    li.appendChild(el("div", "where", [f.model, f.address].filter(Boolean).join(" · ")));
    li.appendChild(el("div", "caps muted", talks(f)));
    const add = button("add-printer accent", "Add", async () => {
      add.disabled = true;
      try {
        data = await invoke("purifier_add", { found: f, name: null });
        reachable.add(f.id);
        found = found.filter((x) => x.id !== f.id);
      } catch (e) {
        $("purifiers-note").textContent = "Could not add it: " + e;
      }
      render();
      if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
    }, `Add ${f.name} to your purifiers`);
    li.appendChild(add);
    return li;
  }

  function render() {
    const mine = $("purifiers-list");
    mine.replaceChildren(...data.purifiers.map(card));
    $("purifiers-empty").classList.toggle("hidden", data.purifiers.length > 0);
    const list = $("purifiers-found");
    list.replaceChildren(...found.map(foundCard));
    $("purifiers-found-head").classList.toggle("hidden", found.length === 0);
  }

  async function rescan() {
    if (scanning) return;
    scanning = true;
    $("purifiers-rescan").disabled = true;
    const note = $("purifiers-note");
    note.textContent = "Looking for purifiers on the network…";
    note.classList.add("working");
    try {
      const s = await invoke("purifiers_scan", { seconds: 4 });
      data = s.purifiers;
      reachable = new Set(s.reachable);
      found = s.found;
      lastScan = Date.now();
      const n = s.reachable.length + found.length;
      note.textContent = n
        ? found.length ? `${found.length} found. Add one to keep it in your purifiers.` : "Your purifiers answered."
        : data.purifiers.length
          ? "No purifier answered. Is it on, and on this Wi-Fi?"
          : "No purifier answered. Is it on, set up with the Philips Air+ app, and on this Wi-Fi?";
    } catch (e) {
      note.textContent = "Could not search: " + e;
    }
    note.classList.remove("working");
    $("purifiers-rescan").disabled = false;
    scanning = false;
    render();
    if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
  }

  async function show() {
    try {
      data = (await invoke("purifiers_saved")) || data;
    } catch (e) {
      $("purifiers-note").textContent = "Could not read your purifiers: " + e;
    }
    render();
    if (Date.now() - lastScan > 2 * 60 * 1000) rescan();
  }

  $("purifiers-rescan").addEventListener("click", rescan);

  return { show, rescan, homeWords };
})();
