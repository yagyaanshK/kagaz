// A camera's own screen, as phone camera apps have it: tapping a camera in
// the list opens it on its own. The picture runs across the top, with its
// sound, quality and full-screen buttons over it (they hide after a few
// seconds; a tap brings them back). Under it sit big buttons: talk (where
// the camera has a speaker Kagaz can reach), screenshot, record, and the
// way to its recordings and back to live. Its recordings play on the
// same screen, with a day at a time on a ruler that slides under a fixed
// line. Turning the phone on its side (or the full-screen button) fills
// the screen, with talk, screenshot and record down the side.
//
// It is the camera grid showing one camera (app.js renders it): the
// screen is one cell, so live view, playback, sound, talk and retries all
// work as they do in the grid. Uses app.js's globals.

let camFocus = null; // { id, listMode, at }: the camera on its own screen

const CP_ICON = {
  back: '<svg viewBox="0 0 24 24"><path d="M15 5l-7 7 7 7"/></svg>',
  talk: '<svg viewBox="0 0 24 24"><path d="M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3zM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/></svg>',
  shot: '<svg viewBox="0 0 24 24"><path d="M3.5 8h3.5l2-3h6l2 3h3.5v11h-17z"/><circle cx="12" cy="13" r="3.5"/></svg>',
  rec: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="7.5"/><circle cx="12" cy="12" r="3.5" class="fill"/></svg>',
  past: '<svg viewBox="0 0 24 24"><path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3M4.5 4v3.5H8"/><path d="M12 8v4.5l3 1.8"/></svg>',
  live: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="2.5" class="fill"/><path d="M7.8 7.8a6 6 0 0 0 0 8.4M16.2 7.8a6 6 0 0 1 0 8.4M5 5a10 10 0 0 0 0 14M19 5a10 10 0 0 1 0 14"/></svg>',
  pause: '<svg viewBox="0 0 24 24"><path d="M9 6v12M15 6v12"/></svg>',
  play: '<svg viewBox="0 0 24 24"><path d="M8 5.5v13l10.5-6.5z" class="fill"/></svg>',
  prev: '<svg viewBox="0 0 24 24"><path d="M14.5 6l-6 6 6 6"/></svg>',
  next: '<svg viewBox="0 0 24 24"><path d="M9.5 6l6 6-6 6"/></svg>',
};

// ---- opening and closing ----

function openCamera(cam) {
  if (rearranging || camFocus) return;
  const tile = $("cam-grid").querySelector(`.cam-cell[data-id="${CSS.escape(cam.device_id)}"] .cam-tile`);
  // From the recordings of every camera: this one goes on from where it is.
  const at = isPlayback() && tile ? tileNow(tile) : null;
  camFocus = { id: cam.device_id, listMode: $("cam-when").value, at };
  KagazNav.push("cam-page", closeCamera);
  renderCameraGrid();
  $("main").scrollTop = 0;
}

function closeCamera() {
  if (!camFocus) return;
  if (cpRec) cpStopRecord();
  const mode = camFocus.listMode;
  camFocus = null;
  KagazNav.pop("cam-page");
  document.querySelectorAll("#cam-grid .kz-full").forEach((el) => KagazPlayer.fullscreen(el, false));
  $("cam-when").value = mode;
  renderCameraGrid();
}

function cpRoot() { return $("cam-grid").querySelector(".cp"); }
function cpTile() { const r = cpRoot(); return r && r.querySelector(".cam-tile"); }

// Live, or the recordings, on the same screen.
function cpMode(mode) {
  if ($("cam-when").value === mode) return;
  if (cpRec) cpStopRecord();
  if (camTalk) stopTalk();
  $("cam-when").value = mode;
  renderCameraGrid();
}

// ---- building the screen ----

function cpButton(cls, icon, label, onClick) {
  const b = document.createElement("button");
  b.type = "button";
  b.className = `cp-btn ${cls}`;
  b.innerHTML = `<span class="cp-icon">${icon}</span><span class="cp-label">${label}</span>`;
  b.setAttribute("aria-label", label);
  b.addEventListener("click", (ev) => { ev.stopPropagation(); onClick(); });
  return b;
}

function buildCameraPage(cam) {
  const playback = isPlayback();
  const root = document.createElement("div");
  root.className = "cam-cell cp big" + (playback ? " cp-past" : "");
  root.dataset.id = cam.device_id;

  const head = document.createElement("div");
  head.className = "cp-head";
  const back = document.createElement("button");
  back.type = "button";
  back.className = "cp-back icon";
  back.innerHTML = CP_ICON.back;
  back.title = "Back to the cameras";
  back.setAttribute("aria-label", back.title);
  back.addEventListener("click", closeCamera);
  const title = document.createElement("h2");
  title.textContent = camName(cam);
  const more = document.createElement("button");
  more.type = "button";
  more.className = "cp-more icon";
  more.textContent = "⋮";
  more.title = `More for ${camName(cam)}`;
  more.setAttribute("aria-label", more.title);
  more.setAttribute("aria-haspopup", "menu");
  head.append(back, title, more);
  root.appendChild(head);

  const tile = buildTile(cam);
  tile.classList.add("big");
  decorateTile(tile, cam);
  root.appendChild(tile);

  // The big buttons under the picture.
  const actions = document.createElement("div");
  actions.className = "cp-actions";
  if (cam.brand === "xiaomi" && !playback) {
    actions.appendChild(cpButton("cp-talk", CP_ICON.talk, "Talk", () => cpTalk(tile, cam)));
  }
  actions.appendChild(cpButton("cp-shot", CP_ICON.shot, "Screenshot", () => cpScreenshot(tile, cam)));
  actions.appendChild(cpButton("cp-rec", CP_ICON.rec, "Record", () => cpToggleRecord(tile, cam)));
  actions.appendChild(playback
    ? cpButton("cp-live", CP_ICON.live, "Live", () => cpMode("live"))
    : cpButton("cp-past", CP_ICON.past, "Recordings", () => cpMode("playback")));
  root.appendChild(actions);

  const toast = document.createElement("div");
  toast.className = "cp-toast hidden";
  toast.setAttribute("role", "status");
  root.appendChild(toast);

  if (playback) root.appendChild(buildPast(cam, tile));
  const forms = document.createElement("div");
  forms.className = "cp-forms";
  root.appendChild(forms);

  more.addEventListener("click", (ev) => { ev.stopPropagation(); cpMenu(more, cam, tile, forms); });
  cpRefresh();
  return root;
}

// The recordings: the day, the ruler, pause and speed.
function buildPast(cam, tile) {
  const box = document.createElement("div");
  box.className = "cp-rec-box";
  const days = document.createElement("div");
  days.className = "cp-days";
  const prev = document.createElement("button");
  prev.type = "button";
  prev.className = "icon cp-day-prev";
  prev.innerHTML = CP_ICON.prev;
  prev.title = "The day before";
  prev.setAttribute("aria-label", prev.title);
  prev.addEventListener("click", () => cpStepDay(-1));
  const label = document.createElement("span");
  label.className = "cp-day";
  const next = document.createElement("button");
  next.type = "button";
  next.className = "icon cp-day-next";
  next.innerHTML = CP_ICON.next;
  next.title = "The day after";
  next.setAttribute("aria-label", next.title);
  next.addEventListener("click", () => cpStepDay(1));
  days.append(prev, label, next);
  box.appendChild(days);

  const bar = document.createElement("div");
  bar.className = "pb-tl cp-ruler";
  box.appendChild(bar);
  const id = cam.device_id;
  bar._tl = makeRuler(bar, {
    dayStart: () => seekDayStart(),
    spans: () => tlSpans([id]),
    detections: () => tlDetections([id]),
    bands: () => tlBands([id]),
    marks: () => tlMarks([id]),
    cursor: () => tileNow(tile),
    idle: () => { const sp = spansOf(id); return sp && sp.length ? sp[sp.length - 1].end : null; },
    onSeek: (at) => startSynced([tile.closest(".cam-cell")], at, tile._sync ? tile._sync.speed : 1),
    clock: (t) => clockText(t),
    label: `${camName(cam)}'s recordings: Left and Right move, + and - zoom`,
  });

  const row = document.createElement("div");
  row.className = "cp-play-row";
  const pause = document.createElement("button");
  pause.type = "button";
  pause.className = "icon cp-pause";
  pause.innerHTML = CP_ICON.pause;
  pause.title = "Pause";
  pause.setAttribute("aria-label", "Pause");
  pause.addEventListener("click", () => { togglePause([tile.closest(".cam-cell")]); cpRefresh(); });
  const at = document.createElement("span");
  at.className = "pb-at";
  at.textContent = "—";
  row.append(pause, at);
  // The camera plays its own card at its own pace (Xiaomi): no speeds.
  if (cam.brand !== "xiaomi") {
    const speeds = document.createElement("span");
    speeds.className = "cp-speeds";
    speeds.setAttribute("role", "group");
    speeds.setAttribute("aria-label", "Speed");
    for (const s of [1, 2, 4, 8]) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "chip cp-speed";
      b.dataset.speed = String(s);
      b.textContent = `${s}×`;
      b.title = s === 1 ? "Normal speed, with sound" : `${s} times as fast, without sound`;
      b.addEventListener("click", () => { speedCells([tile.closest(".cam-cell")], s); cpRefresh(); });
      speeds.appendChild(b);
    }
    row.appendChild(speeds);
  }
  box.appendChild(row);
  const note = document.createElement("p");
  note.className = "muted small pb-note";
  box.appendChild(note);
  const key = document.createElement("p");
  key.className = "tl-legend";
  key.innerHTML = '<i class="tl-key tl-key-rec"></i>recording <i class="tl-key tl-key-motion"></i>motion <i class="tl-key tl-key-person"></i>person';
  box.appendChild(key);
  return box;
}

// The ⋮ menu: the camera's settings; for a Tapo camera's recordings,
// saving a stretch and correcting its clock.
function cpMenu(button, cam, tile, forms) {
  const open = document.querySelector(".cp-menu");
  if (open) { open.remove(); return; }
  const menu = document.createElement("div");
  menu.className = "cp-menu cam-settings menu";
  menu.setAttribute("role", "menu");
  const item = (text, act) => {
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("role", "menuitem");
    b.textContent = text;
    b.addEventListener("click", (ev) => { ev.stopPropagation(); menu.remove(); act(); });
    menu.appendChild(b);
  };
  item("Camera settings…", () => { if (camFocus && isPlayback()) camFocus.at = tileNow(tile); cameraSettings(cam); });
  if (isPlayback() && cam.brand !== "xiaomi") {
    item("Save a stretch as MP4…", () => downloadForm(forms, cam, tile));
    item("Correct the clock…", () => clockForm(forms, cam, tile));
  }
  item("Where files are saved…", () => selectCategory("settings"));
  button.parentElement.appendChild(menu);
  menu.querySelector("button").focus();
  const away = (ev) => {
    if (menu.contains(ev.target)) return;
    menu.remove();
    document.removeEventListener("click", away, true);
  };
  setTimeout(() => document.addEventListener("click", away, true), 0);
  menu.addEventListener("keydown", (ev) => { if (ev.key === "Escape") { ev.stopPropagation(); menu.remove(); button.focus(); } });
}

// What the screen adds to its picture: a badge (live, the time played, or
// recording), the zoom, and in full screen a way out and the big buttons
// down the side.
function decorateTile(tile, cam) {
  const badge = document.createElement("span");
  badge.className = "cam-badge";
  tile.appendChild(badge);
  const zoomLabel = document.createElement("span");
  zoomLabel.className = "cam-zoom-label hidden";
  tile.appendChild(zoomLabel);
  const exit = document.createElement("button");
  exit.type = "button";
  exit.className = "cam-exit";
  exit.innerHTML = CP_ICON.back;
  exit.title = "Leave full screen";
  exit.setAttribute("aria-label", exit.title);
  exit.addEventListener("click", (ev) => { ev.stopPropagation(); KagazPlayer.fullscreen(tile, false); });
  tile.appendChild(exit);
  const side = document.createElement("div");
  side.className = "cam-side";
  if (cam.brand === "xiaomi" && !isPlayback()) side.appendChild(cpButton("cp-talk", CP_ICON.talk, "Talk", () => cpTalk(tile, cam)));
  side.appendChild(cpButton("cp-shot", CP_ICON.shot, "Screenshot", () => cpScreenshot(tile, cam)));
  side.appendChild(cpButton("cp-rec", CP_ICON.rec, "Record", () => cpToggleRecord(tile, cam)));
  tile.appendChild(side);
  zoomable(tile, zoomLabel);
  const ctrl = tile.querySelector(".cam-ctrl");
  if (ctrl) ctrl.addEventListener("click", (ev) => ev.stopPropagation());
}

// The controls over the picture show for a while after a tap.
function tilePeek(tile) {
  const on = !tile.classList.contains("peek");
  tile.classList.toggle("peek", on);
  clearTimeout(tile._peekTimer);
  if (on) tile._peekTimer = setTimeout(() => tile.classList.remove("peek"), 4000);
}

// ---- the state of the buttons ----

function cpRefresh() {
  const root = cpRoot();
  if (!root) return;
  const tile = root.querySelector(".cam-tile");
  const talking = !!(camTalk && camTalk.tile === tile);
  root.querySelectorAll(".cp-talk").forEach((b) => {
    b.classList.toggle("on", talking);
    b.setAttribute("aria-pressed", String(talking));
    b.querySelector(".cp-label").textContent = talking ? "Hang up" : "Talk";
  });
  root.querySelectorAll(".cp-rec").forEach((b) => {
    b.classList.toggle("on", !!cpRec);
    b.setAttribute("aria-pressed", String(!!cpRec));
    b.querySelector(".cp-label").textContent = cpRec ? "Stop" : "Record";
  });
  const paused = cellsPaused([root]);
  const pause = root.querySelector(".cp-pause");
  if (pause) {
    pause.innerHTML = paused ? CP_ICON.play : CP_ICON.pause;
    pause.title = paused ? "Play" : "Pause";
    pause.setAttribute("aria-label", pause.title);
  }
  // The day: known once the camera has said which days it has.
  const day = root.querySelector(".cp-day");
  if (day) {
    const iso = $("pb-date").value;
    day.textContent = cpDayText(iso);
    root.querySelector(".cp-day-next").disabled = !iso || iso >= cpTodayIso();
  }
  const speed = tile && tile._sync ? tile._sync.speed : 1;
  root.querySelectorAll(".cp-speed").forEach((b) => {
    const on = Number(b.dataset.speed) === speed;
    b.classList.toggle("active", on);
    b.setAttribute("aria-pressed", String(on));
  });
  cpBadge();
}

function cpBadge() {
  const tile = cpTile();
  if (!tile) return;
  const badge = tile.querySelector(".cam-badge");
  if (!badge) return;
  if (cpRec) {
    const s = Math.floor((Date.now() - cpRec.since) / 1000);
    badge.textContent = `REC ${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
    badge.className = "cam-badge rec";
  } else if (isPlayback()) {
    const at = tileNow(tile);
    badge.textContent = at === null ? "" : clockText(Math.floor(at));
    badge.className = "cam-badge past";
  } else {
    badge.textContent = "LIVE";
    badge.className = "cam-badge live";
  }
  badge.classList.toggle("hidden", !badge.textContent);
}
setInterval(() => { if (camFocus) cpBadge(); }, 500);

// ---- talk ----

function cpTalk(tile, cam) {
  if (camTalk && camTalk.tile === tile) stopTalk(); else startTalk(tile, cam);
  cpRefresh();
}

// ---- screenshot ----

// "Front door 2026-10-24 10.15.30": the camera's name and the moment (in
// recordings, the moment played, on the camera's clock).
function cpFileName(cam, tile) {
  const p2 = (n) => String(n).padStart(2, "0");
  const at = isPlayback() ? tileNow(tile) : null;
  let stamp;
  if (at !== null) {
    const d = new Date((Math.floor(at) + pbOffset * 60) * 1000);
    stamp = `${d.getUTCFullYear()}-${p2(d.getUTCMonth() + 1)}-${p2(d.getUTCDate())} ${p2(d.getUTCHours())}.${p2(d.getUTCMinutes())}.${p2(d.getUTCSeconds())}`;
  } else {
    const d = new Date();
    stamp = `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())} ${p2(d.getHours())}.${p2(d.getMinutes())}.${p2(d.getSeconds())}`;
  }
  return `${camName(cam)} ${stamp}`;
}

async function cpScreenshot(tile, cam) {
  const p = tile._player;
  const v = tile.querySelector("video");
  let canvas = null;
  if (p && p.stats.shown) canvas = p.canvas;
  else if (v && v.videoWidth) {
    canvas = document.createElement("canvas");
    canvas.width = v.videoWidth;
    canvas.height = v.videoHeight;
    canvas.getContext("2d").drawImage(v, 0, 0);
  }
  if (!canvas) { cpToast("There is no picture to save yet."); return; }
  tile.classList.remove("flash");
  void tile.offsetWidth;
  tile.classList.add("flash");
  try {
    const blob = await new Promise((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.92));
    if (!blob) throw new Error("the picture could not be made");
    const data = await new Promise((resolve, reject) => {
      const r = new FileReader();
      r.onload = () => resolve(String(r.result).replace(/^data:[^,]*,/, ""));
      r.onerror = () => reject(r.error);
      r.readAsDataURL(blob);
    });
    const where = await invoke("save_picture", { name: `${cpFileName(cam, tile)}.jpg`, data });
    cpToast(`Screenshot saved: ${where}`, where);
  } catch (e) {
    cpToast(`The screenshot was not saved: ${(e && e.message) || e}`);
  }
}

// ---- recording ----

let cpRec = null; // { watch, since }

async function cpToggleRecord(tile, cam) {
  if (cpRec) { cpStopRecord(); return; }
  const h = tile._watchHandle;
  if (!h) {
    cpToast(tile._playback && tile._playback.kind === "file"
      ? "This part is a downloaded clip, already a file on this device."
      : "Recording starts once the picture plays.");
    return;
  }
  try {
    await invoke("camera_record_start", { watch: h.id, name: cpFileName(cam, tile) });
    cpRec = { watch: h.id, since: Date.now() };
  } catch (e) {
    cpToast(`Recording did not start: ${e}`);
  }
  cpRefresh();
}

async function cpStopRecord() {
  const r = cpRec;
  if (!r) return;
  cpRec = null;
  cpRefresh();
  try {
    cpSaved(await invoke("camera_record_stop", { watch: r.watch }));
  } catch (e) {
    // Already kept when its picture stopped (recording-saved says so).
  }
}

function cpSaved(saved) {
  const s = Math.round(saved.seconds);
  cpToast(`Recording saved (${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}): ${saved.location}`, saved.path);
}

// The picture stopped while recording (another quality, the screen
// closed, the app sent to the background): what was recorded is kept.
listen("recording-saved", (e) => {
  if (cpRec && cpRec.watch === e.payload.watch) { cpRec = null; cpRefresh(); }
  cpSaved(e.payload);
});
listen("recording-failed", (e) => {
  const [watch, why] = e.payload;
  if (cpRec && cpRec.watch === watch) { cpRec = null; cpRefresh(); }
  cpToast(`The recording was not saved: ${why}`);
});

// A line under the buttons; with a file, a way to show it.
function cpToast(text, path) {
  const t = cpRoot() && cpRoot().querySelector(".cp-toast");
  if (!t) { $("cam-message").textContent = text; return; }
  t.innerHTML = "";
  const span = document.createElement("span");
  span.textContent = text;
  t.appendChild(span);
  if (path) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "link";
    b.textContent = "Show";
    b.addEventListener("click", () => invoke("show_in_folder", { path }).catch((e) => cpToast("" + e)));
    t.appendChild(b);
  }
  t.classList.remove("hidden");
  clearTimeout(t._timer);
  t._timer = setTimeout(() => t.classList.add("hidden"), 10000);
}

// ---- days ----

// The cameras' today, on their clock.
function cpTodayIso() {
  return new Date(Date.now() + pbOffset * 60000).toISOString().slice(0, 10);
}

function cpDayText(iso) {
  if (!iso) return "";
  const today = cpTodayIso();
  if (iso === today) return "Today";
  const y = new Date(Date.parse(today + "T00:00:00Z") - 86400000).toISOString().slice(0, 10);
  if (iso === y) return "Yesterday";
  const d = new Date(iso + "T00:00:00Z");
  return d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short", timeZone: "UTC" });
}

// The day before or after; through the days the camera has footage for
// when it says which.
function cpStepDay(dir) {
  const cur = $("pb-date").value;
  if (!cur) return;
  const days = [...pbDatesWithFootage].sort();
  let next = null;
  if (days.length) {
    next = dir < 0 ? days.filter((d) => d < cur).pop() : days.find((d) => d > cur);
  }
  if (!next) {
    const d = new Date(Date.parse(cur + "T00:00:00Z") + dir * 86400000).toISOString().slice(0, 10);
    if (dir > 0 && d > cpTodayIso()) return;
    next = d;
  }
  if (camFocus) camFocus.at = null;
  setPickerDate(next);
  renderCameraGrid();
}

// The day's list is in: play. From where the list left off, else the
// newest footage today (the last minute), else the day's first.
function cpStartPlayback() {
  const root = cpRoot();
  if (!root || !camFocus) return;
  const id = camFocus.id;
  const day = seekDayStart();
  let at = camFocus.at;
  camFocus.at = null;
  if (at === null || at === undefined || day === null || at < day || at >= day + 86400) {
    const spans = spansOf(id);
    const today = $("pb-date").value === cpTodayIso();
    if (spans && spans.length) {
      const last = spans[spans.length - 1];
      at = today ? Math.max(last.start, last.end - 60) : spans[0].start;
    } else {
      at = today ? Date.now() / 1000 - 120 : (day ?? Date.now() / 1000) + 10 * 3600;
    }
  }
  startSynced([root], at, 1);
  cpRefresh();
}

// ---- zoom: two fingers or the wheel; double tap for twice the size ----

function zoomable(tile, label) {
  const z = { s: 1, x: 0, y: 0 };
  const fingers = new Map();
  let pinch = null; // { span, s, mid, x, y }
  let pan = null; // { x, y, zx, zy }
  let lastTap = { t: 0, x: 0, y: 0 };
  const apply = () => {
    const r = tile.getBoundingClientRect();
    z.s = Math.min(6, Math.max(1, z.s));
    const mx = (r.width * (z.s - 1)) / 2, my = (r.height * (z.s - 1)) / 2;
    z.x = Math.min(mx, Math.max(-mx, z.x));
    z.y = Math.min(my, Math.max(-my, z.y));
    tile.style.setProperty("--zs", String(z.s));
    tile.style.setProperty("--zx", `${z.x}px`);
    tile.style.setProperty("--zy", `${z.y}px`);
    label.textContent = `${z.s.toFixed(1)}×`;
    label.classList.toggle("hidden", z.s <= 1.01);
    tile.classList.toggle("zoomed", z.s > 1.01);
  };
  // Zoom to `s` keeping the point (cx, cy) of the tile where it is.
  const zoomAt = (s, cx, cy) => {
    const r = tile.getBoundingClientRect();
    const px = cx - r.left - r.width / 2, py = cy - r.top - r.height / 2;
    const k = Math.min(6, Math.max(1, s)) / z.s;
    z.x = px - (px - z.x) * k;
    z.y = py - (py - z.y) * k;
    z.s *= k;
    apply();
  };
  const onControl = (ev) => ev.target.closest("button, select, input, a, .cam-ctrl, .cam-side");
  tile.addEventListener("pointerdown", (ev) => {
    if (onControl(ev)) return;
    fingers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    if (fingers.size === 2) {
      const [a, b] = [...fingers.values()];
      pinch = { span: Math.hypot(a.x - b.x, a.y - b.y) || 1, s: z.s };
      pan = null;
      tile._noClick = true;
    } else if (fingers.size === 1) {
      pan = { x: ev.clientX, y: ev.clientY, zx: z.x, zy: z.y };
    }
  });
  tile.addEventListener("pointermove", (ev) => {
    if (!fingers.has(ev.pointerId)) return;
    fingers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    if (pinch && fingers.size === 2) {
      const [a, b] = [...fingers.values()];
      const span = Math.hypot(a.x - b.x, a.y - b.y) || 1;
      zoomAt((pinch.s * span) / pinch.span, (a.x + b.x) / 2, (a.y + b.y) / 2);
      return;
    }
    if (pan && z.s > 1.01) {
      const dx = ev.clientX - pan.x, dy = ev.clientY - pan.y;
      if (Math.abs(dx) + Math.abs(dy) > 6) tile._noClick = true;
      z.x = pan.zx + dx;
      z.y = pan.zy + dy;
      apply();
    }
  });
  const up = (ev) => {
    if (!fingers.has(ev.pointerId)) return;
    fingers.delete(ev.pointerId);
    if (fingers.size < 2) pinch = null;
    if (fingers.size === 0) pan = null;
    // A double tap: twice the size there, or back to the whole picture.
    if (ev.type === "pointerup" && !tile._noClick) {
      const now = Date.now();
      if (now - lastTap.t < 320 && Math.hypot(ev.clientX - lastTap.x, ev.clientY - lastTap.y) < 30) {
        if (z.s > 1.01) { z.s = 1; z.x = 0; z.y = 0; apply(); } else zoomAt(2, ev.clientX, ev.clientY);
        lastTap = { t: 0, x: 0, y: 0 };
      } else lastTap = { t: now, x: ev.clientX, y: ev.clientY };
    }
  };
  tile.addEventListener("pointerup", up);
  tile.addEventListener("pointercancel", up);
  tile.addEventListener("wheel", (ev) => {
    if (onControl(ev)) return;
    ev.preventDefault();
    zoomAt(z.s * (ev.deltaY > 0 ? 0.8 : 1.25), ev.clientX, ev.clientY);
  }, { passive: false });
  apply();
}

// ---- a phone turned on its side fills the screen, and upright again leaves ----

const cpLandscape = window.matchMedia("(orientation: landscape)");
cpLandscape.addEventListener("change", () => {
  if (!ON_PHONE || !camFocus || currentView !== "cameras") return;
  const tile = cpTile();
  if (!tile) return;
  const full = tile.classList.contains("kz-full");
  if (cpLandscape.matches && !full) tile.querySelector(".cam-full")?.click();
  else if (!cpLandscape.matches && full) KagazPlayer.fullscreen(tile, false);
});
