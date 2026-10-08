// A seek bar for one day of camera recordings, used for the whole grid, a
// group and each camera.
//
//   - It shows a window of the day: 8 AM to 8 PM unless zoomed.
//   - A time scale under the bar labels the window, finer as you zoom.
//   - Scrolling over the bar zooms around the pointer; shift + scroll (or a
//     sideways scroll) moves the window.
//   - Pointing shows the time under the pointer; holding and dragging shows
//     where the cursor will drop, and letting go seeks there.
//   - Recordings are drawn in the bar; detections the camera reported sit on
//     a lane above it (motion orange, person blue). Clicking one seeks to
//     its start.
//
// The caller supplies the data and decides what a seek does:
//   makeTimeline(el, { dayStart, spans, detections, cursor, onSeek, clock })
// each a function, called whenever the bar redraws (`draw()`), so the bar
// always shows current data.

const TL_STEPS = [10, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800];
const TL_MIN = 60;          // the narrowest window: one minute
const TL_DAY = 86400;

function makeTimeline(el, opts) {
  el.classList.add("tl");
  el.innerHTML =
    '<div class="tl-events"></div>' +
    '<div class="tl-track"><div class="tl-spans"></div><div class="tl-pos hidden"></div>' +
    '<div class="tl-ghost hidden"><span class="tl-tip"></span></div></div>' +
    '<div class="tl-scale"></div>';
  const events = el.querySelector(".tl-events");
  const track = el.querySelector(".tl-track");
  const spans = el.querySelector(".tl-spans");
  const pos = el.querySelector(".tl-pos");
  const ghost = el.querySelector(".tl-ghost");
  const tip = el.querySelector(".tl-tip");
  const scale = el.querySelector(".tl-scale");

  // The window, relative to the day's start: [from, from + len).
  const view = { from: 8 * 3600, len: 12 * 3600, day: null, key: "", follow: true, touched: 0 };
  let dragging = false;

  const fmt = opts.clock || ((t) => String(t));
  const clampView = () => {
    view.len = Math.min(TL_DAY, Math.max(TL_MIN, view.len));
    view.from = Math.min(TL_DAY - view.len, Math.max(0, view.from));
  };
  const timeAt = (clientX) => {
    const r = track.getBoundingClientRect();
    const f = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    return view.day + view.from + f * view.len;
  };
  const pct = (t) => ((t - view.day - view.from) / view.len) * 100;

  function drawScale() {
    const width = Math.max(1, track.getBoundingClientRect().width);
    // About one label per 90 px.
    const want = view.len / Math.max(1, width / 90);
    const step = TL_STEPS.find((s) => s >= want) || TL_STEPS[TL_STEPS.length - 1];
    scale.innerHTML = "";
    const first = Math.ceil(view.from / step) * step;
    for (let s = first; s <= view.from + view.len; s += step) {
      const tick = document.createElement("span");
      tick.className = "tl-tick";
      tick.style.left = `${((s - view.from) / view.len) * 100}%`;
      tick.textContent = shortClock(view.day + s, step);
      scale.appendChild(tick);
    }
  }

  // Labels as precise as the scale needs: "9 AM", "9:30 AM", "9:30:10 AM".
  function shortClock(t, step) {
    const full = fmt(Math.floor(t)); // "09:30:10 AM"
    const m = /^(\d\d):(\d\d):(\d\d) (AM|PM)$/.exec(full);
    if (!m) return full;
    const h = String(Number(m[1]));
    if (step < 60) return `${h}:${m[2]}:${m[3]} ${m[4]}`;
    if (step < 3600 || m[2] !== "00") return `${h}:${m[2]} ${m[4]}`;
    return `${h} ${m[4]}`;
  }

  function drawData() {
    spans.innerHTML = "";
    events.innerHTML = "";
    const lo = view.day + view.from, hi = lo + view.len;
    for (const sp of opts.spans() || []) {
      if (sp.end <= lo || sp.start >= hi) continue;
      const a = Math.max(sp.start, lo), b = Math.min(sp.end, hi);
      const seg = document.createElement("div");
      seg.className = "tl-span" + (sp.event ? " tl-event-rec" : "");
      seg.style.left = `${pct(a)}%`;
      seg.style.width = `${Math.max(0.2, ((b - a) / view.len) * 100)}%`;
      spans.appendChild(seg);
    }
    for (const d of opts.detections() || []) {
      if (d.end < lo || d.start >= hi) continue;
      const a = Math.max(d.start, lo), b = Math.min(Math.max(d.end, d.start + 1), hi);
      const mark = document.createElement("div");
      mark.className = `tl-det tl-${d.label}`;
      mark.style.left = `${pct(a)}%`;
      mark.style.width = `${Math.max(0.35, ((b - a) / view.len) * 100)}%`;
      mark.title = `${d.label}: ${fmt(d.start)} – ${fmt(d.end)}${d.camera ? " (" + d.camera + ")" : ""}`;
      mark.addEventListener("click", (ev) => { ev.stopPropagation(); opts.onSeek(d.start); });
      events.appendChild(mark);
    }
  }

  function draw() {
    const day = opts.dayStart();
    if (day === null || day === undefined) return;
    if (view.day !== day) { view.day = day; view.from = 8 * 3600; view.len = 12 * 3600; view.follow = true; }
    const key = `${view.day}:${view.from}:${view.len}:${Math.round(track.getBoundingClientRect().width)}:${opts.dataKey ? opts.dataKey() : ""}`;
    if (key !== view.key) { view.key = key; drawScale(); drawData(); }
    const at = opts.cursor();
    if (at === null || at === undefined) { pos.classList.add("hidden"); return; }
    // Back to following the playing time 15 s after the last scroll.
    if (!view.follow && Date.now() - view.touched > 15000) view.follow = true;
    // Keep a playing cursor in sight, unless the user is looking elsewhere.
    if (view.follow && !dragging && (at < view.day + view.from || at > view.day + view.from + view.len)) {
      view.from = at - view.day - view.len * 0.2;
      clampView();
      view.key = "";
      return draw();
    }
    const p = pct(at);
    pos.classList.toggle("hidden", p < 0 || p > 100);
    pos.style.left = `${p}%`;
  }

  function showGhost(clientX) {
    const t = timeAt(clientX);
    ghost.classList.remove("hidden");
    ghost.style.left = `${pct(t)}%`;
    tip.textContent = fmt(Math.floor(t));
    ghost.classList.toggle("tl-drag", dragging);
  }

  track.addEventListener("mousemove", (ev) => showGhost(ev.clientX));
  track.addEventListener("mouseleave", () => { if (!dragging) ghost.classList.add("hidden"); });
  track.addEventListener("mousedown", (ev) => {
    if (ev.button !== 0 || view.day === null) return;
    ev.preventDefault();
    dragging = true;
    showGhost(ev.clientX);
    const move = (e) => showGhost(e.clientX);
    const up = (e) => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      dragging = false;
      ghost.classList.add("hidden");
      view.follow = true;
      opts.onSeek(Math.floor(timeAt(e.clientX)));
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  el.addEventListener("wheel", (ev) => {
    if (view.day === null) return;
    ev.preventDefault();
    const sideways = ev.shiftKey || Math.abs(ev.deltaX) > Math.abs(ev.deltaY);
    if (sideways) {
      const d = ev.shiftKey ? ev.deltaY : ev.deltaX;
      view.from += (d / 100) * view.len * 0.15;
    } else {
      // Zoom around the time under the pointer.
      const at = timeAt(ev.clientX) - view.day;
      const r = track.getBoundingClientRect();
      const f = Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width));
      view.len *= ev.deltaY > 0 ? 1.25 : 0.8;
      clampView();
      view.from = at - f * view.len;
    }
    clampView();
    view.follow = false; // the user is looking somewhere on purpose (for a while)
    view.touched = Date.now();
    view.key = "";
    draw();
    showGhost(ev.clientX);
  }, { passive: false });

  return { draw, reset() { view.day = null; view.key = ""; draw(); } };
}
