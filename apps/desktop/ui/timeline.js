// A seek bar for one day of camera recordings, used for the whole grid, a
// group and each camera.
//
//   - It shows a window of the day: 8 AM to 8 PM unless zoomed.
//   - A time scale under the bar labels the window, finer as you zoom.
//   - Scrolling over the bar zooms around the pointer; shift + scroll (or a
//     sideways scroll) moves the window.
//   - Pointing shows the time under the pointer; holding and dragging (a
//     mouse or a finger) shows where the cursor will drop, and letting go
//     seeks there. Two fingers pinch to zoom.
//   - From the keyboard (Tab to the bar): Left and Right move the cursor,
//     + and - zoom.
//   - Recordings are drawn in the bar; detections the camera reported sit on
//     a lane above it (motion orange, person blue). Clicking one seeks to
//     its start.
//   - Where a camera restarted there is a red line; where its clock may be
//     wrong afterwards the bar is striped, and where it was corrected tinted.
//
// The caller supplies the data and decides what a seek does:
//   makeTimeline(el, { dayStart, spans, detections, cursor, onSeek, clock,
//                      bands, marks })
// each a function, called whenever the bar redraws (`draw()`), so the bar
// always shows current data.

const TL_STEPS = [10, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800];
const TL_MIN = 60;          // the narrowest window: one minute
const TL_DAY = 86400;

function makeTimeline(el, opts) {
  el.classList.add("tl");
  el.innerHTML =
    '<div class="tl-events"></div>' +
    '<div class="tl-track"><div class="tl-spans"></div><div class="tl-marks"></div><div class="tl-pos hidden"></div>' +
    '<div class="tl-ghost hidden"><span class="tl-tip"></span></div></div>' +
    '<div class="tl-scale"></div>';
  const events = el.querySelector(".tl-events");
  const track = el.querySelector(".tl-track");
  const spans = el.querySelector(".tl-spans");
  const marks = el.querySelector(".tl-marks");
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
    marks.innerHTML = "";
    const lo = view.day + view.from, hi = lo + view.len;
    for (const b of (opts.bands && opts.bands()) || []) {
      if (b.end <= lo || b.start >= hi) continue;
      const a = Math.max(b.start, lo), z = Math.min(b.end, hi);
      const band = document.createElement("div");
      band.className = `tl-band tl-band-${b.kind}`;
      band.style.left = `${pct(a)}%`;
      band.style.width = `${Math.max(0.2, ((z - a) / view.len) * 100)}%`;
      band.title = b.title;
      marks.appendChild(band);
    }
    for (const m of (opts.marks && opts.marks()) || []) {
      if (m.at < lo || m.at >= hi) continue;
      const line = document.createElement("div");
      line.className = "tl-restart";
      line.style.left = `${pct(m.at)}%`;
      line.title = m.title;
      marks.appendChild(line);
    }
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

  // Zoom by `factor` around the time at `clientX` (or the window's middle).
  function zoom(factor, clientX) {
    const r = track.getBoundingClientRect();
    const f = clientX === undefined ? 0.5 : Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    const at = view.from + f * view.len;
    view.len *= factor;
    clampView();
    view.from = at - f * view.len;
    clampView();
    view.follow = false; // the user is looking somewhere on purpose (for a while)
    view.touched = Date.now();
    view.key = "";
    draw();
  }

  // Mouse, pen or finger: drag to choose, let go to seek; two fingers zoom.
  const fingers = new Map();
  let pinch = null;
  track.addEventListener("pointermove", (ev) => {
    if (fingers.has(ev.pointerId)) fingers.set(ev.pointerId, ev.clientX);
    if (pinch && fingers.size === 2) {
      const [a, b] = [...fingers.values()];
      const span = Math.max(8, Math.abs(a - b));
      zoom(pinch / span, (a + b) / 2);
      pinch = span;
      return;
    }
    if (ev.pointerType === "mouse" || dragging) showGhost(ev.clientX);
  });
  track.addEventListener("pointerleave", () => { if (!dragging) ghost.classList.add("hidden"); });
  track.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0 || view.day === null) return;
    ev.preventDefault();
    track.setPointerCapture(ev.pointerId);
    fingers.set(ev.pointerId, ev.clientX);
    if (fingers.size === 2) {
      // A second finger: a pinch, not a seek.
      dragging = false;
      ghost.classList.add("hidden");
      const [a, b] = [...fingers.values()];
      pinch = Math.max(8, Math.abs(a - b));
      return;
    }
    dragging = true;
    showGhost(ev.clientX);
  });
  const lift = (ev, seek) => {
    fingers.delete(ev.pointerId);
    if (pinch) {
      if (!fingers.size) pinch = null;
      return;
    }
    if (!dragging) return;
    dragging = false;
    ghost.classList.add("hidden");
    view.follow = true;
    if (seek) opts.onSeek(Math.floor(timeAt(ev.clientX)));
  };
  track.addEventListener("pointerup", (ev) => lift(ev, true));
  track.addEventListener("pointercancel", (ev) => lift(ev, false));

  track.tabIndex = 0;
  track.setAttribute("role", "slider");
  track.setAttribute("aria-label", opts.label || "Recordings: Left and Right move, + and - zoom");
  track.addEventListener("keydown", (ev) => {
    if (view.day === null) return;
    const at = opts.cursor();
    const step = Math.max(1, Math.round(view.len / 48));
    if (ev.key === "ArrowLeft" || ev.key === "ArrowRight") {
      const from = at === null || at === undefined ? view.day + view.from : at;
      opts.onSeek(Math.floor(from + (ev.key === "ArrowLeft" ? -step : step)));
    } else if (ev.key === "+" || ev.key === "=") zoom(0.8);
    else if (ev.key === "-" || ev.key === "_") zoom(1.25);
    else return;
    ev.preventDefault();
  });
  el.addEventListener("wheel", (ev) => {
    if (view.day === null) return;
    ev.preventDefault();
    const sideways = ev.shiftKey || Math.abs(ev.deltaX) > Math.abs(ev.deltaY);
    if (sideways) {
      const d = ev.shiftKey ? ev.deltaY : ev.deltaX;
      view.from += (d / 100) * view.len * 0.15;
    } else {
      zoom(ev.deltaY > 0 ? 1.25 : 0.8, ev.clientX);
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

// A camera's day as a ruler under a fixed line, as phone camera apps show
// it: the recordings slide under the line in the middle, which marks the
// moment playing. Drag the ruler and let go to play from the line; tap a
// moment to play from there; two fingers (or the mouse wheel) zoom; shift
// + wheel moves along. Left and Right step, + and - zoom from the
// keyboard. Grey is nothing recorded; recordings, events and detections
// are coloured as in makeTimeline.
//
// The same options as makeTimeline, plus `idle()`: where to rest when
// nothing plays (the newest recording, say).
function makeRuler(el, opts) {
  el.classList.add("ruler");
  el.innerHTML = '<canvas class="ruler-canvas"></canvas><div class="ruler-line"></div><div class="ruler-at"></div>';
  const canvas = el.querySelector("canvas");
  const at = el.querySelector(".ruler-at");
  const fmt = opts.clock || ((t) => String(t));
  // The moment under the line, and seconds per CSS pixel.
  const view = { centre: null, spp: 30, day: null };
  let drag = null; // { x, centre, moved }
  let held = 0; // until when the person's own position holds (ms)
  const fingers = new Map();
  let pinch = 0;

  const clamp = () => {
    view.spp = Math.min(TL_DAY / 200, Math.max(0.25, view.spp));
    if (view.day !== null && view.centre !== null) view.centre = Math.min(view.day + TL_DAY, Math.max(view.day, view.centre));
  };
  const colour = (name, fallback) => getComputedStyle(el).getPropertyValue(name).trim() || fallback;

  function draw() {
    const day = opts.dayStart();
    if (day === null || day === undefined) return;
    if (view.day !== day) { view.day = day; view.centre = null; }
    const playing = opts.cursor();
    if (!drag && Date.now() > held) {
      const rest = playing ?? (opts.idle && opts.idle());
      if (rest !== null && rest !== undefined) view.centre = rest;
    }
    if (view.centre === null) view.centre = day + 12 * 3600;
    clamp();
    const w = el.clientWidth, h = el.clientHeight;
    if (!w || !h) return;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
    }
    const g = canvas.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, h);
    const lo = view.centre - (w / 2) * view.spp, hi = view.centre + (w / 2) * view.spp;
    const x = (t) => (t - lo) / view.spp;
    const band = { top: 22, height: h - 30 };
    // The day itself in grey; before and after it, nothing.
    g.fillStyle = colour("--line", "#ddd");
    const d0 = Math.max(lo, day), d1 = Math.min(hi, day + TL_DAY);
    if (d1 > d0) g.fillRect(x(d0), band.top, x(d1) - x(d0), band.height);
    for (const b of (opts.bands && opts.bands()) || []) {
      if (b.end <= lo || b.start >= hi) continue;
      g.fillStyle = b.kind === "fixed" ? "rgba(255,210,74,0.45)" : "rgba(232,80,60,0.35)";
      g.fillRect(x(b.start), band.top, Math.max(1, x(b.end) - x(b.start)), band.height);
    }
    const accent = colour("--accent", "#2f6f4e");
    for (const s of opts.spans() || []) {
      if (s.end <= lo || s.start >= hi) continue;
      g.fillStyle = s.event ? "#e8892f" : accent;
      g.globalAlpha = s.event ? 0.75 : 0.85;
      g.fillRect(x(s.start), band.top, Math.max(1, x(s.end) - x(s.start)), band.height);
    }
    g.globalAlpha = 1;
    // Detections: a strip along the bottom of the band.
    for (const d of opts.detections() || []) {
      if (d.end < lo || d.start >= hi) continue;
      g.fillStyle = d.label === "person" ? "#2f74e0" : d.label === "motion" ? "#c26a12" : "#b48ce0";
      g.fillRect(x(d.start), band.top + band.height - 8, Math.max(3, x(Math.max(d.end, d.start + 1)) - x(d.start)), 8);
    }
    g.fillStyle = "#e8503c";
    for (const m of (opts.marks && opts.marks()) || []) {
      if (m.at < lo || m.at >= hi) continue;
      g.fillRect(x(m.at) - 1, band.top - 3, 2, band.height + 6);
    }
    // The scale: a label about every 90 px, small ticks between.
    const step = TL_STEPS.find((s) => s >= view.spp * 90) || TL_STEPS[TL_STEPS.length - 1];
    g.fillStyle = colour("--muted", "#888");
    g.font = "11px system-ui, sans-serif";
    g.textAlign = "center";
    g.textBaseline = "top";
    for (let t = Math.ceil(lo / step) * step; t <= hi; t += step) {
      if (t < day || t > day + TL_DAY) continue;
      g.fillRect(Math.round(x(t)), band.top - 5, 1, 5);
      g.fillText(rulerLabel(fmt(t), step), x(t), 3);
    }
    at.textContent = fmt(Math.floor(view.centre));
    el.classList.toggle("ruler-moving", !!drag);
  }

  // "9 AM", "9:30 AM" or "9:30:10 AM", as fine as the scale needs.
  function rulerLabel(full, step) {
    const m = /^(\d\d):(\d\d):(\d\d) (AM|PM)$/.exec(full);
    if (!m) return full;
    const hour = String(Number(m[1]));
    if (step < 60) return `${hour}:${m[2]}:${m[3]}`;
    if (step < 3600 || m[2] !== "00") return `${hour}:${m[2]}`;
    return `${hour} ${m[4]}`;
  }

  function zoom(factor) {
    view.spp *= factor;
    held = Date.now() + 1500;
    clamp();
    draw();
  }

  let wheelSeek = null;
  function seekSoon() {
    clearTimeout(wheelSeek);
    wheelSeek = setTimeout(() => opts.onSeek(Math.floor(view.centre)), 500);
  }

  canvas.addEventListener("pointerdown", (ev) => {
    if (view.day === null) return;
    canvas.setPointerCapture(ev.pointerId);
    fingers.set(ev.pointerId, ev.clientX);
    if (fingers.size === 2) {
      const [a, b] = [...fingers.values()];
      pinch = Math.max(8, Math.abs(a - b));
      drag = null;
      return;
    }
    drag = { x: ev.clientX, centre: view.centre, moved: false };
  });
  canvas.addEventListener("pointermove", (ev) => {
    if (!fingers.has(ev.pointerId)) return;
    fingers.set(ev.pointerId, ev.clientX);
    if (pinch && fingers.size === 2) {
      const [a, b] = [...fingers.values()];
      const span = Math.max(8, Math.abs(a - b));
      zoom(pinch / span);
      pinch = span;
      return;
    }
    if (!drag) return;
    const dx = ev.clientX - drag.x;
    if (Math.abs(dx) > 4) drag.moved = true;
    view.centre = drag.centre - dx * view.spp;
    clamp();
    draw();
  });
  const lift = (ev, act) => {
    fingers.delete(ev.pointerId);
    if (pinch) {
      if (!fingers.size) pinch = 0;
      return;
    }
    if (!drag) return;
    const d = drag;
    drag = null;
    if (!act) { draw(); return; }
    if (!d.moved) {
      // A tap: play from the moment tapped.
      const r = canvas.getBoundingClientRect();
      view.centre += (ev.clientX - r.left - r.width / 2) * view.spp;
      clamp();
    }
    held = Date.now() + 4000;
    draw();
    opts.onSeek(Math.floor(view.centre));
  };
  canvas.addEventListener("pointerup", (ev) => lift(ev, true));
  canvas.addEventListener("pointercancel", (ev) => lift(ev, false));
  el.addEventListener("wheel", (ev) => {
    if (view.day === null) return;
    ev.preventDefault();
    const sideways = ev.shiftKey || Math.abs(ev.deltaX) > Math.abs(ev.deltaY);
    if (sideways) {
      view.centre += ((ev.shiftKey ? ev.deltaY : ev.deltaX) / 2) * view.spp;
      held = Date.now() + 4000;
      clamp();
      draw();
      seekSoon();
    } else {
      zoom(ev.deltaY > 0 ? 1.25 : 0.8);
    }
  }, { passive: false });
  canvas.tabIndex = 0;
  canvas.setAttribute("role", "slider");
  canvas.setAttribute("aria-label", opts.label || "Recordings: Left and Right move, + and - zoom");
  canvas.addEventListener("keydown", (ev) => {
    if (view.day === null) return;
    const step = Math.max(1, Math.round(view.spp * 40));
    if (ev.key === "ArrowLeft" || ev.key === "ArrowRight") {
      view.centre += ev.key === "ArrowLeft" ? -step : step;
      held = Date.now() + 4000;
      clamp();
      draw();
      seekSoon();
    } else if (ev.key === "+" || ev.key === "=") zoom(0.8);
    else if (ev.key === "-" || ev.key === "_") zoom(1.25);
    else return;
    ev.preventDefault();
  });
  // Smooth while it plays; nothing to do once the ruler is gone.
  const timer = setInterval(() => {
    if (!el.isConnected) { clearInterval(timer); return; }
    if (!document.hidden) draw();
  }, 250);

  return { draw, reset() { view.day = null; draw(); } };
}
