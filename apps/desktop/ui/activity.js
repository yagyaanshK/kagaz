// Activity: what happened, newest first, grouped by day. Today it holds the
// doorbell: who rang (with the station's picture of them), the calls
// (answered, missed, made) and rings from door stations on this network;
// other kinds of device add their events here as they arrive. The same
// list shows on the Doorbell page's History tab.
// Uses app.js's globals: $, invoke, selectCategory.

const KagazActivity = (() => {
  const FILTERS = [
    ["all", "All"],
    ["door", "Visitors"],
    ["calls", "Calls"],
    ["missed", "Missed"],
    ["local", "On this network"],
  ];
  // One feed per place on screen: the Activity page and the Doorbell tab.
  function feed() {
    return { entries: [], pages: { door: 0, calls: 0 }, more: { door: true, calls: true }, note: "", loading: false };
  }
  const feeds = { page: feed(), doorbell: feed() };
  let filter = "all";

  // ---------- time ----------

  // The cloud gives "YYYY-MM-DD HH:MM:SS" in the home's time, or seconds.
  function when(at) {
    if (at === null || at === undefined || at === "") return null;
    if (typeof at === "number" || /^\d{9,13}$/.test(String(at))) {
      const n = Number(at);
      return new Date(n > 1e12 ? n : n * 1000);
    }
    const m = String(at).match(/^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})(?::(\d{2}))?/);
    if (m) return new Date(+m[1], +m[2] - 1, +m[3], +m[4], +m[5], +(m[6] || 0));
    const d = new Date(at);
    return isNaN(d) ? null : d;
  }
  function dayLabel(d) {
    if (!d) return "Earlier";
    const today = new Date();
    const start = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const days = Math.round((start(today) - start(d)) / 86400000);
    if (days === 0) return "Today";
    if (days === 1) return "Yesterday";
    return d.toLocaleDateString(undefined, { weekday: days < 7 ? "long" : undefined, day: "numeric", month: "long", year: d.getFullYear() === today.getFullYear() ? undefined : "numeric" });
  }
  const timeText = (d, raw) => (d ? d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) : String(raw || ""));

  // ---------- loading ----------

  async function loadCloud(f, log) {
    if (!f.more[log]) return;
    const page = f.pages[log] + 1;
    const r = await invoke("onetouch_history", { log, page });
    f.pages[log] = page;
    f.more[log] = r.more;
    for (const e of r.entries) f.entries.push({ ...e, source: log, date: when(e.at) });
  }

  async function loadLocal(f) {
    try {
      const st = await invoke("doorbell_status");
      const stations = await invoke("doorbell_stations").catch(() => ({ stations: [] }));
      const name = (a) => (stations.stations.find((s) => s.address === a) || {}).name;
      for (const r of st.recent || []) {
        f.entries.push({
          id: `local-${r.at}-${r.from}`, kind: "local", source: "local", at: r.at, date: when(r.at),
          device: name(r.from) || r.caller || r.from, detail: r.outcome ? r.outcome[0].toUpperCase() + r.outcome.slice(1) : "Rang", direction: "", picture: "", thumbnail: "", video: "", unread: false,
        });
      }
    } catch (_) {}
  }

  // The first page (reset) or the next one of each cloud feed.
  async function load(f, reset, withLocal) {
    if (f.loading) return;
    f.loading = true;
    if (reset) Object.assign(f, feed(), { loading: true });
    f.note = "";
    try {
      const results = await Promise.allSettled([loadCloud(f, "door"), loadCloud(f, "calls")]);
      const failed = results.find((r) => r.status === "rejected");
      if (failed) {
        const why = String(failed.reason);
        f.note = /sign in|expired/i.test(why)
          ? "Sign in to OneTouch (Doorbell) to see who rang and the calls."
          : `Could not read the doorbell's history: ${why}`;
        f.more.door = f.more.calls = false;
      }
      if (reset && withLocal) await loadLocal(f);
    } finally {
      f.loading = false;
    }
    f.entries.sort((a, b) => (b.date ? b.date.getTime() : 0) - (a.date ? a.date.getTime() : 0));
  }

  // ---------- drawing ----------

  const ICON = {
    door: '<svg viewBox="0 0 24 24"><path d="M6 8a6 6 0 0 1 12 0v5l2 3H4l2-3zM10 19a2 2 0 0 0 4 0"/></svg>',
    missed: '<svg viewBox="0 0 24 24"><path d="M3 5l6 6 4-4 8 8M21 9v6h-6"/></svg>',
    answered: '<svg viewBox="0 0 24 24"><path d="M21 3l-8 8M14 3h7v7M5 4h3l2 5-2 1a11 11 0 0 0 6 6l1-2 5 2v3a2 2 0 0 1-2 2A17 17 0 0 1 3 6a2 2 0 0 1 2-2z"/></svg>',
    outgoing: '<svg viewBox="0 0 24 24"><path d="M14 10l7-7M15 3h6v6M5 4h3l2 5-2 1a11 11 0 0 0 6 6l1-2 5 2v3a2 2 0 0 1-2 2A17 17 0 0 1 3 6a2 2 0 0 1 2-2z"/></svg>',
    local: '<svg viewBox="0 0 24 24"><path d="M6 3h12v18H6zM10 8h4M12 13a1.5 1.5 0 1 0 0 .01M9 18h6"/></svg>',
  };

  function shown(e) {
    switch (filter) {
      case "door": return e.source === "door";
      case "calls": return e.source === "calls";
      case "missed": return e.direction === "missed";
      case "local": return e.source === "local";
      default: return true;
    }
  }

  function item(e) {
    const el = document.createElement("button");
    el.type = "button";
    el.className = "act-item" + (e.unread ? " unread" : "") + (e.direction === "missed" ? " missed" : "");
    const pic = document.createElement("span");
    pic.className = "act-pic";
    if (e.thumbnail) {
      const img = document.createElement("img");
      img.src = e.thumbnail;
      img.alt = "";
      img.loading = "lazy";
      img.referrerPolicy = "no-referrer";
      img.addEventListener("error", () => { pic.innerHTML = ICON.door; pic.classList.add("icon"); });
      pic.appendChild(img);
      if (e.video) pic.insertAdjacentHTML("beforeend", '<span class="act-play" aria-hidden="true">▶</span>');
    } else {
      pic.classList.add("icon");
      pic.innerHTML = ICON[e.direction] || ICON[e.source] || ICON.door;
    }
    const text = document.createElement("span");
    text.className = "act-text";
    const who = document.createElement("strong");
    who.textContent = e.device || "Someone";
    const what = document.createElement("span");
    what.className = "muted small";
    what.textContent = e.detail || "";
    text.append(who, what);
    const time = document.createElement("span");
    time.className = "act-time muted small";
    time.textContent = timeText(e.date, e.at);
    el.append(pic, text, time);
    if (e.picture || e.video) el.addEventListener("click", () => view(e));
    else el.disabled = true;
    return el;
  }

  function render(container, f) {
    container.innerHTML = "";
    let day = null;
    let count = 0;
    for (const e of f.entries) {
      if (!shown(e)) continue;
      const label = dayLabel(e.date);
      if (label !== day) {
        day = label;
        const h = document.createElement("h3");
        h.className = "act-day";
        h.textContent = label;
        container.appendChild(h);
      }
      container.appendChild(item(e));
      count++;
    }
    return count;
  }

  // ---------- the picture viewer ----------

  function view(e) {
    $("viewer-title").textContent = [e.device, timeText(e.date, e.at)].filter(Boolean).join(" · ");
    const img = $("viewer-img");
    const video = $("viewer-video");
    img.classList.toggle("hidden", !!e.video);
    video.classList.toggle("hidden", !e.video);
    if (e.video) {
      video.src = e.video;
      video.play().catch(() => {});
    } else {
      img.src = e.picture;
    }
    $("viewer").classList.remove("hidden");
  }
  function closeViewer() {
    const video = $("viewer-video");
    video.pause();
    video.removeAttribute("src");
    video.load();
    $("viewer-img").removeAttribute("src");
    $("viewer").classList.add("hidden");
  }
  $("viewer-close").addEventListener("click", closeViewer);
  $("viewer").addEventListener("click", (ev) => { if (ev.target.id === "viewer") closeViewer(); });
  document.addEventListener("keydown", (ev) => { if (ev.key === "Escape" && !$("viewer").classList.contains("hidden")) closeViewer(); });

  // ---------- the Activity page ----------

  function drawFilters() {
    const box = $("activity-filters");
    box.innerHTML = "";
    for (const [id, label] of FILTERS) {
      const c = document.createElement("button");
      c.type = "button";
      c.className = "chip" + (filter === id ? " active" : "");
      c.textContent = label;
      c.addEventListener("click", () => { filter = id; drawPage(); });
      box.appendChild(c);
    }
  }

  function drawPage() {
    const f = feeds.page;
    drawFilters();
    const n = render($("activity-list"), f);
    $("activity-note").textContent = f.loading ? "Loading…" : f.note || (n ? "" : "Nothing here yet.");
    $("activity-more").classList.toggle("hidden", !(f.more.door || f.more.calls) || f.loading);
  }

  async function show() {
    feeds.page.loading = false;
    $("activity-note").textContent = "Loading…";
    await load(feeds.page, true, true);
    drawPage();
  }
  $("activity-refresh").addEventListener("click", show);
  $("activity-more").addEventListener("click", async () => { await load(feeds.page, false, false); drawPage(); });

  // ---------- the Doorbell page's History tab ----------

  async function doorbell(reset = true) {
    const f = feeds.doorbell;
    const saved = filter;
    filter = "all";
    $("ot-history").innerHTML = '<p class="muted">Loading…</p>';
    await load(f, reset, false);
    const n = render($("ot-history"), f);
    filter = saved;
    if (!n || f.note) {
      const p = document.createElement("p");
      p.className = "muted";
      p.textContent = f.note || "No rings or calls yet.";
      $("ot-history").prepend(p);
    }
    $("ot-history-more").classList.toggle("hidden", !(f.more.door || f.more.calls));
  }
  $("ot-history-more").addEventListener("click", () => doorbell(false));

  return { show, doorbell, when };
})();
