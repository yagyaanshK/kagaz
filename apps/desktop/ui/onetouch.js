// OneTouch account/video UI. Account credentials stay in Rust; this receives
// device metadata and only the selected recording's temporary media URL.
// Each device is one card for both watching and calling; live view and
// calls play inside the card with the window's player (player.js).
let otStatus = null;
let otBusy = false;
let otTab = 'live';
let otPage = 1;
let otMore = false;
let otPlayback = 0;
let otTimer = null;
let otInitialized = false;
let otLive = null; // { key, deviceId, stage, handle, timer }

function otPasswordVisible(visible) {
  $('ot-password').type = visible ? 'text' : 'password';
  $('ot-password-toggle').textContent = visible ? 'Hide' : 'Show';
  $('ot-password-toggle').setAttribute('aria-label', visible ? 'Hide password' : 'Show password');
}
$('ot-password-toggle').addEventListener('click', () => {
  otPasswordVisible($('ot-password').type === 'password');
});

// The rarely used account actions sit behind one ⋯ button.
// Back, Escape or a click elsewhere closes it.
function otMenu(open) {
  const was = !$('ot-actions').classList.contains('hidden');
  if (open === was) return;
  $('ot-actions').classList.toggle('hidden', !open);
  $('ot-menu').setAttribute('aria-expanded', String(open));
  if (open) KagazNav.push('ot-menu', () => otMenu(false));
  else KagazNav.pop('ot-menu');
}
$('ot-menu').addEventListener('click', (ev) => { ev.stopPropagation(); otMenu($('ot-actions').classList.contains('hidden')); });
document.addEventListener('click', (ev) => {
  if (!ev.target.closest('#ot-menu-box')) otMenu(false);
});
document.addEventListener('keydown', (ev) => {
  if (ev.key === 'Escape' && !$('ot-actions').classList.contains('hidden')) { otMenu(false); $('ot-menu').focus(); }
});

function otRender() {
  const signed = !!otStatus?.logged_in;
  const saved = otStatus?.remembered;
  const canRemember = otStatus?.remember_supported !== false;
  $('ot-reconnect').classList.toggle('hidden', signed || !saved);
  $('ot-forget').classList.toggle('hidden', !saved && !otStatus?.notice);
  $('ot-remembered').classList.toggle('hidden', !saved || signed);
  $('ot-remembered').textContent = saved ? `Saved sign-in: ${saved.username}.` : '';
  $('ot-signin').textContent = saved ? 'Use another account' : 'Sign in';
  $('ot-remember-label').classList.toggle('hidden', !canRemember);
  $('ot-remember-help').classList.toggle('hidden', !canRemember);
  // While the form is open its own Sign in is the one to press.
  $('ot-signin').classList.toggle('hidden', signed || !$('ot-login').classList.contains('hidden'));
  $('ot-signout').classList.toggle('hidden', !signed);
  $('ot-refresh').classList.toggle('hidden', !signed);
  $('ot-menu').classList.toggle('hidden', !signed && !saved);
  $('ot-connected').classList.toggle('hidden', !signed);
  $('ot-service-name').textContent = signed ? otStatus.service_name : 'OneTouch';
  $('ot-account-name').textContent = signed ? `Signed in as ${otStatus.username}` : 'Live video and recordings through your OneTouch account.';
  otRenderDevices();
  if (signed && !(otStatus.devices || []).length) {
    const note = document.createElement('p'); note.className = 'muted'; note.textContent = 'No doorbells loaded. Try Refresh devices in the \u22EF menu.'; $('ot-devices').append(note);
  }
  $('ot-devices').classList.toggle('hidden', otTab !== 'live');
  if ($('ot-dial-box')) $('ot-dial-box').classList.toggle('hidden', otTab !== 'live');
  $('ot-recordings-pane').classList.toggle('hidden', otTab !== 'recordings');
  $('ot-family-pane').classList.toggle('hidden', otTab !== 'family');
  if (otTab === 'family') otRenderFamily();
  for (const [id, on] of [['ot-live-tab', otTab === 'live'], ['ot-recordings-tab', otTab === 'recordings'], ['ot-family-tab', otTab === 'family']]) {
    $(id).setAttribute('aria-pressed', String(on)); $(id).classList.toggle('active', on);
  }
  for (const id of ['ot-reconnect','ot-forget','ot-remember','ot-signin','ot-signout','ot-refresh','ot-login-submit','ot-login-cancel','ot-live-tab','ot-recordings-tab','ot-family-tab','ot-recordings-refresh','ot-provider','ot-legacy-device','ot-username','ot-password','ot-password-toggle']) $(id).disabled = otBusy;
  $('ot-prev').disabled = otBusy || otPage <= 1;
  $('ot-next').disabled = otBusy || !otMore;
  document.querySelectorAll('#ot-recordings button').forEach(b => { b.disabled = otBusy; });
}

// ---------- the account's devices: one list for watching and calling ----------
// The cloud describes each device twice: once for live view (its id, may
// it be watched) and once for calls (its number, its door key). Both carry
// the same name, so each device is one card. Tapping a card opens it: its
// live view starts inside it when it has one, and its Call button appears
// under the picture. The call itself (calls.js) then plays in the same
// card. Calls arrive whether or not a card is open.

const OT_KIND = { door: "Door station", indoor: "Indoor monitor", guard: "Guard phone", apps: "They ring all at once", family: "Family member", other: "Device" };
const OT_LIVE_KIND = { "0": "door", "1": "door", "2": "indoor", "3": "guard" };
const OT_ORDER = { door: 0, indoor: 1, guard: 2, other: 3, family: 4, apps: 5 };
let otPeople = []; // the home's family members (their own logins)
let otPeopleState = { phase: 'idle', error: '' }; // idle | loading | ok | error
const OT_ICON = {
  door: KagazIcons.svg("doorbell"),
  indoor: KagazIcons.svg("indoor"),
  guard: KagazIcons.svg("guard"),
  apps: KagazIcons.svg("mobile"),
  family: KagazIcons.svg("family"),
  other: KagazIcons.svg("device"),
};
// Small glyphs for the per-device action buttons (OneTouch shows a row of these).
const OT_ACT = {
  live: KagazIcons.svg("eye"),
  call: KagazIcons.svg("phone"),
};
let otOpen = null; // the key of the open card

function otCallStatus() {
  return (window.KagazCalls && window.KagazCalls.status()) || null;
}

// Every device of the account, once, door stations first.
function otCards() {
  const norm = (n) => (n || "").trim().toLowerCase();
  const cards = [];
  const byName = new Map();
  for (const d of otStatus?.devices || []) {
    const card = { key: `live:${d.id}`, name: d.name, live: d, call: null, kind: OT_LIVE_KIND[d.kind] || "other", online: d.status !== "0" };
    cards.push(card);
    byName.set(norm(d.name), card);
  }
  // Family members, each with their own number (when the cloud gives it).
  for (const p of otPeople) {
    cards.push({ key: `person:${p.id || p.name}`, name: p.name, live: null, call: p.sip ? { sip: p.sip, name: p.name, kind: "family" } : null, kind: "family", online: true, person: true });
  }
  const calls = otCallStatus();
  for (let d of (calls && calls.devices) || []) {
    // The account's own number rings every phone signed in with this login.
    if (d.kind === "apps") d = { ...d, name: "Phones on my login" };
    const card = byName.get(norm(d.name));
    if (card && !card.call) {
      card.call = d;
      card.kind = d.kind || card.kind;
      card.online = d.online;
    } else {
      cards.push({ key: `call:${d.sip}`, name: d.name, live: null, call: d, kind: d.kind || "other", online: d.online });
    }
  }
  return cards
    .map((c, i) => ({ c, i }))
    .sort((a, b) => (OT_ORDER[a.c.kind] ?? 3) - (OT_ORDER[b.c.kind] ?? 3) || a.i - b.i)
    .map(({ c }) => c);
}

function otRenderDevices() {
  const list = $("ot-devices");
  // A call may be playing inside a card: keep it out of the rebuild
  // (calls.js puts it back where it belongs).
  const callCard = $("call-card");
  if (callCard && $("call-slot") && list.contains(callCard)) $("call-slot").appendChild(callCard);
  list.replaceChildren();
  const cards = otCards();
  if (otOpen && !cards.some((c) => c.key === otOpen)) otOpen = null;
  const calls = otCallStatus();
  const lineUp = calls && calls.line === "up";
  const busy = calls && calls.phase && calls.phase !== "idle";
  for (const c of cards) {
    const open = otOpen === c.key;
    const watching = otLive && otLive.key === c.key;
    const canWatch = !!(c.live && c.live.can_monitor);
    const activeCall = busy && open; // the call card nests in this card's slot
    const el = document.createElement("div");
    el.className = "dev-card";
    el.dataset.key = c.key;
    el.dataset.name = c.name.trim().toLowerCase();
    el.classList.toggle("open", open);
    el.classList.toggle("watching", !!watching);
    el.classList.toggle("offline", !c.online);

    // Head: icon, name, status — a device row, like OneTouch's. Not a button;
    // the actions below drive everything, so nothing toggles by surprise.
    const head = document.createElement("div");
    head.className = "dev-head";
    const icon = document.createElement("span");
    icon.className = "dev-icon";
    icon.innerHTML = OT_ICON[c.kind] || OT_ICON.other;
    const text = document.createElement("span");
    text.className = "dev-text";
    const name = document.createElement("strong");
    name.textContent = c.name;
    const sub = document.createElement("span");
    sub.className = "muted small";
    sub.textContent = [OT_KIND[c.kind] || OT_KIND.other, c.online ? "" : "offline"].filter(Boolean).join(" · ");
    text.append(name, sub);
    const dot = document.createElement("span");
    dot.className = "dev-dot" + (c.online ? " on" : "");
    dot.setAttribute("aria-hidden", "true");
    head.append(icon, text, dot);
    el.append(head);

    // Inline live stage (or the running call card), shown in the slot.
    const slot = document.createElement("div");
    slot.className = "dev-slot";
    el.append(slot);
    if (watching) slot.append(otLive.stage.el);

    // Persistent action row: Live · Call, the two things Kagaz can do for a
    // OneTouch device (opening a door happens inside a call, via its button).
    if (!activeCall && (canWatch || c.call)) {
      const actions = document.createElement("div");
      actions.className = "dev-actions";
      if (canWatch) {
        const live = document.createElement("button");
        live.type = "button";
        live.className = "dev-btn" + (watching ? " on" : "");
        live.innerHTML = OT_ACT.live + `<span>${watching ? "Stop" : "Live"}</span>`;
        live.disabled = otBusy;
        live.title = watching ? "Stop live view" : "Watch live";
        live.addEventListener("click", () => (watching ? otClose() : otOpenCard(c)));
        actions.append(live);
      }
      if (c.call) {
        const call = document.createElement("button");
        call.type = "button";
        call.className = "dev-btn call";
        call.innerHTML = OT_ACT.call + `<span>${c.kind === "apps" ? "Ring" : "Call"}</span>`;
        call.disabled = !lineUp || busy || otBusy;
        call.title = c.kind === "apps" ? "Ring the phones on your login" : `Call ${c.name}`;
        call.addEventListener("click", async () => { await otFocusCard(c); window.KagazCalls.call(c.call.sip, c.key); });
        actions.append(call);
      }
      el.append(actions);
    }

    // A short reason line, only when something needs explaining.
    const note = document.createElement("p");
    note.className = "muted small dev-note";
    if (watching) note.textContent = "Live view is picture only; the door sends its sound in a call.";
    else if (c.call && !lineUp && !busy) note.textContent = calls && calls.line === "off" ? "Turn on “Ring me here” above to call." : "Calls connect once the call line is up.";
    else if (!canWatch && !c.call) {
      note.textContent = c.live ? "OneTouch does not allow live view of this device."
        : c.kind === "apps" ? "Every phone signed in with your login shares one number, so they ring together. Family members with their own login are listed on their own."
        : c.kind === "family" ? "OneTouch did not give a call number for this member yet."
        : "";
    }
    if (note.textContent) el.append(note);

    list.append(el);
  }
  if (window.KagazCalls) window.KagazCalls.place();
}

async function otOpenCard(card) {
  KagazPlayer.audioContext(); // a tap is what lets the page make sound later
  await otStopLive();
  otOpen = card.key;
  KagazNav.push("ot-card", otClose);
  if (card.live && card.live.can_monitor) await otWatch(card);
  else otRenderDevices();
  // The open card comes to the top, so its picture has the screen.
  const el = document.querySelector(`.dev-card[data-key="${CSS.escape(card.key)}"]`);
  if (el) el.scrollIntoView({ block: "start", behavior: "smooth" });
}

// Make a card the open (full-width) one without starting live, so a call
// placed into it has room. Used by the Call button.
async function otFocusCard(card) {
  if (otOpen === card.key) return;
  await otStopLive();
  otOpen = card.key;
  KagazNav.push("ot-card", otClose);
  otRenderDevices();
  const el = document.querySelector(`.dev-card[data-key="${CSS.escape(card.key)}"]`);
  if (el) el.scrollIntoView({ block: "start", behavior: "smooth" });
}

// Home: open a device's card by its name once the account is loaded.
async function otOpenByName(name) {
  const want = (name || "").trim().toLowerCase();
  for (let i = 0; i < 40 && !(otStatus && otStatus.devices); i++) await new Promise((r) => setTimeout(r, 150));
  const card = otCards().find((c) => c.name.trim().toLowerCase() === want);
  if (card) await otOpenCard(card);
}

// Home: call a device by its name (its card opens, and the call plays in it).
async function otCallByName(name) {
  const want = (name || "").trim().toLowerCase();
  for (let i = 0; i < 40 && !(otStatus && otStatus.devices); i++) await new Promise((r) => setTimeout(r, 150));
  const card = otCards().find((c) => c.name.trim().toLowerCase() === want);
  if (!card || !card.call || !window.KagazCalls) return otOpenByName(name);
  await otFocusCard(card);
  window.KagazCalls.call(card.call.sip, card.key);
}

async function otClose() {
  otOpen = null;
  KagazNav.pop("ot-card");
  await otStopLive();
  otRenderDevices();
}

async function otRun(work, message = '') {
  if (otBusy) return;
  otBusy = true; $('ot-message').textContent = message; otRender();
  try { await work(); }
  catch (e) {
    $('ot-message').textContent = String(e);
    // Expiration is resolved in Rust; immediately restore the sign-in action.
    try { otStatus = await invoke('onetouch_status'); } catch (_) {}
    if (!otStatus?.logged_in) await stopOneTouch();
  } finally { otBusy = false; otRender(); }
}
async function refreshOneTouch() {
  await otRun(async () => {
    otStatus = await invoke('onetouch_status');
    if (otStatus.logged_in && !otInitialized) {
      $('ot-message').textContent = 'Loading your OneTouch devices…';
      otStatus = await invoke('onetouch_refresh'); otInitialized = true;
    }
    $('ot-message').textContent = otStatus?.notice || '';
  });
  otLoadPeople();
}
// Fetched fresh after every sign-in, reconnect and refresh, and whenever the Family tab opens.
// A failure is shown as one (with Try again), never as an empty family.
let otPeopleSeq = 0;
async function otLoadPeople() {
  if (!otStatus?.logged_in) { otPeople = []; otPeopleState = { phase: 'idle', error: '' }; otRenderFamily(); return; }
  const seq = ++otPeopleSeq;
  otPeopleState = { phase: 'loading', error: '' }; otRenderFamily();
  try {
    const list = await invoke('onetouch_people');
    if (seq !== otPeopleSeq) return;
    otPeople = Array.isArray(list) ? list : []; otPeopleState = { phase: 'ok', error: '' };
  } catch (e) {
    if (seq !== otPeopleSeq) return;
    otPeopleState = { phase: 'error', error: String((e && e.message) || e) };
  }
  otRenderFamily();
  otRenderDevices();
}
$('ot-signin').addEventListener('click', () => {
  if (otStatus?.remembered) {
    $('ot-username').value = otStatus.remembered.username;
    $('ot-provider').value = otStatus.remembered.provider;
    $('ot-legacy-device').value = otStatus.remembered.legacy_device || '';
    $('ot-provider').dispatchEvent(new Event('change'));
  }
  otPasswordVisible(false); $('ot-login').classList.remove('hidden'); $('ot-signin').classList.add('hidden'); $('ot-message').textContent = ''; $('ot-username').focus();
  KagazNav.push('ot-login', () => $('ot-login-cancel').click());
});
$('ot-login-cancel').addEventListener('click', () => {
  $('ot-password').value = ''; otPasswordVisible(false); $('ot-login').classList.add('hidden'); $('ot-message').textContent = '';
  $('ot-signin').classList.toggle('hidden', !!otStatus?.logged_in);
  KagazNav.pop('ot-login');
});
$('ot-login').addEventListener('submit', async e => {
  e.preventDefault();
  const username = $('ot-username').value.trim(); const password = $('ot-password').value;
  if (!username || !password) {
    $('ot-message').textContent = !username ? 'Enter your OneTouch username.' : 'Enter your OneTouch password.';
    $(!username ? 'ot-username' : 'ot-password').focus();
    return;
  }
  otPasswordVisible(false);
  await otRun(async () => {
    try { otStatus = await invoke('onetouch_login', { username, password, provider: $('ot-provider').value, legacyDevice: $('ot-legacy-device').value.trim(), remember: otStatus?.remember_supported !== false && $('ot-remember').checked }); }
    finally { $('ot-password').value = ''; otPasswordVisible(false); }
    otInitialized = true; otTab = 'live'; otPage = 1;
    $('ot-login').classList.add('hidden'); $('ot-message').textContent = otStatus?.notice || ''; KagazNav.pop('ot-login');
    otLoadPeople();
  }, `Signing in to ${$('ot-provider').selectedOptions[0].textContent}…`);
});

// ---- Sign in by scanning the OneTouch QR code (welcome email or phone app) ----
// The camera (or a chosen photo) is read in the window; Rust decodes the code.
// A OneTouch code is encrypted with a key inside the vendor's own app, so Kagaz
// cannot complete sign-in from it offline — it says so and points at the
// username and password printed in the same email. A plain-credentials code
// signs in straight away.
let otQrStream = null, otQrTimer = null, otQrBusy = false, otQrDone = false;

function otQrNote(t) { const n = $('ot-qr-note'); if (n) n.textContent = t; }

async function otQrOpen() {
  otQrDone = false;
  const dlg = $('ot-qr-dialog');
  if (!dlg.open) dlg.showModal();
  otQrNote('Point the camera at the code.');
  try {
    otQrStream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' } });
    const v = $('ot-qr-video');
    v.srcObject = otQrStream; await v.play();
    dlg.classList.add('has-camera');
    otQrTick();
  } catch (_) {
    dlg.classList.remove('has-camera');
    otQrNote('No camera available here. Choose a photo of the QR code instead.');
  }
}

function otQrStop() {
  if (otQrTimer) { clearTimeout(otQrTimer); otQrTimer = null; }
  if (otQrStream) { otQrStream.getTracks().forEach((t) => t.stop()); otQrStream = null; }
  const v = $('ot-qr-video'); if (v) v.srcObject = null;
}

function otQrClose() {
  otQrStop();
  const dlg = $('ot-qr-dialog');
  if (dlg.open) dlg.close();
}

// Draw a source (video frame or image) to the canvas, reduced so the longest
// side is at most 640, and return its grayscale bytes for the decoder.
function otQrGray(source, sw, sh) {
  const max = 640, scale = Math.min(1, max / Math.max(sw, sh));
  const w = Math.max(1, Math.round(sw * scale)), h = Math.max(1, Math.round(sh * scale));
  const c = $('ot-qr-canvas'); c.width = w; c.height = h;
  const ctx = c.getContext('2d', { willReadFrequently: true });
  ctx.drawImage(source, 0, 0, w, h);
  const rgba = ctx.getImageData(0, 0, w, h).data;
  const gray = new Uint8Array(w * h);
  for (let i = 0, j = 0; j < gray.length; i += 4, j++) {
    gray[j] = (rgba[i] * 77 + rgba[i + 1] * 150 + rgba[i + 2] * 29) >> 8;
  }
  let bin = '';
  for (let k = 0; k < gray.length; k++) bin += String.fromCharCode(gray[k]);
  return { width: w, height: h, gray: btoa(bin) };
}

async function otQrDecode(frame) {
  try { return await invoke('onetouch_qr_decode', frame); }
  catch (_) { return { kind: 'none' }; }
}

function otQrHandle(res) {
  if (!res || res.kind === 'none') return false;
  if (res.kind === 'credentials') {
    otQrDone = true; otQrClose();
    $('ot-username').value = res.username; $('ot-password').value = res.password;
    $('ot-login').requestSubmit();
    return true;
  }
  if (res.kind === 'encrypted') {
    otQrDone = true; otQrStop();
    otQrNote('This is a OneTouch sign-in code. It is encrypted, so sign in with the username and password printed in the same welcome email.');
    return true;
  }
  // Some other code: show a little of it and keep scanning.
  otQrNote('That code is not a OneTouch sign-in code. Try the QR from your welcome email.');
  return false;
}

async function otQrTick() {
  if (otQrDone || !otQrStream) return;
  const v = $('ot-qr-video');
  if (v && v.videoWidth && !otQrBusy) {
    otQrBusy = true;
    const done = otQrHandle(await otQrDecode(otQrGray(v, v.videoWidth, v.videoHeight)));
    otQrBusy = false;
    if (done) return;
  }
  otQrTimer = setTimeout(otQrTick, 450);
}

async function otQrFromFile(file) {
  if (!file) return;
  otQrStop();
  otQrNote('Reading the picture…');
  const img = new Image();
  img.onload = async () => {
    const res = await otQrDecode(otQrGray(img, img.naturalWidth, img.naturalHeight));
    if (!otQrHandle(res)) otQrNote('No QR code found in that picture. Try a clearer, closer photo.');
    URL.revokeObjectURL(img.src);
  };
  img.onerror = () => otQrNote('That image could not be opened.');
  img.src = URL.createObjectURL(file);
}

if ($('ot-qr-open')) {
  $('ot-qr-open').addEventListener('click', otQrOpen);
  $('ot-qr-cancel').addEventListener('click', otQrClose);
  $('ot-qr-close').addEventListener('click', otQrClose);
  $('ot-qr-file').addEventListener('change', (e) => otQrFromFile(e.target.files && e.target.files[0]));
  $('ot-qr-dialog').addEventListener('close', otQrStop);
}

$('ot-signout').addEventListener('click', () => otRun(async () => {
  await stopOneTouch();
  await invoke('onetouch_logout'); otStatus = await invoke('onetouch_status'); otInitialized = false;
  $('ot-recordings').replaceChildren(); $('ot-message').textContent = ''; $('ot-password').value = ''; otPasswordVisible(false);
}));
$('ot-reconnect').addEventListener('click', () => otRun(async () => {
  await stopOneTouch();
  otStatus = await invoke('onetouch_reconnect');
  otInitialized = true; otTab = 'live'; otPage = 1;
  $('ot-login').classList.add('hidden');
  $('ot-password').value = ''; otPasswordVisible(false);
  $('ot-message').textContent = otStatus.notice || '';
  otLoadPeople();
}, 'Reconnecting to OneTouch…'));
$('ot-forget').addEventListener('click', () => otRun(async () => {
  otStatus = await invoke('onetouch_forget');
  $('ot-password').value = ''; $('ot-username').value = ''; otPasswordVisible(false);
  $('ot-message').textContent = 'Saved sign-in removed.';
}));
$('ot-refresh').addEventListener('click', () => otRun(async () => {
  otStatus = await invoke('onetouch_refresh'); otInitialized = true; $('ot-message').textContent = '';
  otLoadPeople();
}, 'Refreshing OneTouch devices…'));

// Stop the live view (the card stays open).
async function otStopLive() {
  if (!otLive) return;
  const live = otLive;
  otLive = null;
  clearInterval(live.timer);
  if (live.handle) live.handle.stop();
  live.stage.stop();
  otRenderDevices();
}

// Stop whatever plays: the live view in a card, or a recording.
async function stopOneTouch() {
  otPlayback++;
  clearTimeout(otTimer); otTimer = null;
  await otStopLive();
  const v = $('ot-video'); v.pause(); v.removeAttribute('src'); v.load();
  $('ot-picture').removeAttribute('src'); $('ot-player').classList.add('hidden');
  try { await invoke('onetouch_stop'); } catch (_) {}
}
function otShowPlayer(title, picture = false) {
  $('ot-player').classList.remove('hidden'); $('ot-playing-title').textContent = title;
  $('ot-video').classList.toggle('hidden', picture); $('ot-picture').classList.toggle('hidden', !picture);
  $('ot-player-note').textContent = picture ? 'Loading snapshot…' : 'Connecting…';
}
async function otPlayVideo(url, generation) {
  if (generation !== otPlayback) return;
  const v = $('ot-video'); v.src = url;
  otTimer = setTimeout(() => {
    if (generation === otPlayback && v.readyState < 2) $('ot-player-note').textContent = 'No video received yet. Close and try again, or check that live view works in OneTouch Smart Cloud.';
  }, 20000);
  // A stalled stream can leave play() pending indefinitely. Keep navigation
  // and retry controls usable while media events report playback progress.
  v.play().catch(e => {
    if (generation !== otPlayback) return;
    $('ot-player-note').textContent = e.name === 'NotAllowedError' ? 'Press Play to start video.' : 'Could not play this video. Refresh and try again.';
  });
}

// Live view inside the device's card: Rust speaks to the relay, the page
// decodes the pictures (player.js). The relay sends no sound for live
// view; the door's sound comes in a call.
async function otWatch(card) {
  const device = card.live;
  // LIVE once a picture shows, not while it is still connecting.
  const stage = new KagazPlayer.Stage(document.createDocumentFragment(), { title: device.name, badge: "", sound: false, label: `doorbell ${device.name}` });
  stage.setNote("Connecting…");
  const live = { key: card.key, deviceId: device.id, stage, handle: null, timer: null };
  otLive = live;
  otRenderDevices();
  // The picture says what is happening until the first picture shows.
  live.timer = setInterval(() => {
    if (otLive !== live) return;
    if (live.stage.player.stats.shown === 0) stage.setNote(live.stage.player.note());
    else stage.setBadge("LIVE");
  }, 500);
  stage.player.onended = (why) => {
    if (otLive !== live) return;
    clearInterval(live.timer);
    stage.setIdle(true);
    stage.setBadge("");
    stage.setNote(why ? `Live view ended: ${why}.` : "Live view ended.");
    watchAgain();
  };
  let again = null;
  const watchAgain = () => {
    if (again) return;
    again = stage.button("Watch again", "Watch again", () => {
      if (otLive === live) otWatch(card);
    }, "kz-word");
  };
  $('ot-message').textContent = '';
  try {
    live.handle = await KagazPlayer.watch(stage.player, 'onetouch_watch', { deviceId: device.id });
    if (otLive !== live) live.handle.stop();
  } catch (e) {
    if (otLive === live) { clearInterval(live.timer); stage.setIdle(true); stage.setBadge(""); stage.setNote(String(e)); watchAgain(); }
  }
}
async function otLoadRecordings(page) {
  await stopOneTouch();
  const list = await invoke('onetouch_recordings', { page });
  otPage = page; otMore = list.more; $('ot-page').textContent = `Page ${page}`;
  const container = $('ot-recordings'); container.replaceChildren();
  for (const item of list.entries) {
    const row = document.createElement('div'); row.className = 'ot-recording';
    const label = document.createElement('span');
    // Preserve the cloud's timestamp string; do not assume its timezone.
    label.textContent = [item.name, item.at, item.has_video ? 'Video' : 'Snapshot'].filter(Boolean).join(' · ');
    row.append(label);
    for (const [picture, available, text] of [[false, item.has_video, 'Play'], [true, item.has_picture, 'View snapshot']]) {
      if (!available) continue;
      const button = document.createElement('button'); button.type = 'button'; button.textContent = text;
      button.addEventListener('click', () => otRun(async () => {
        await stopOneTouch(); const generation = otPlayback;
        const url = await invoke('onetouch_recording_media', { id: item.id, picture });
        if (generation !== otPlayback) return;
        otShowPlayer([item.name, item.at].filter(Boolean).join(' · '), picture);
        $('ot-player').scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        if (picture) $('ot-picture').src = url; else await otPlayVideo(url, generation);
      }));
      row.append(button);
    }
    container.append(row);
  }
  $('ot-message').textContent = list.message || (list.entries.length ? '' : 'No cloud recordings or snapshots on this page. Footage stored only on an indoor monitor is not included.');
}
$('ot-live-tab').addEventListener('click', () => otRun(async () => { await stopOneTouch(); otTab = 'live'; }));
$('ot-family-tab').addEventListener('click', async () => { await otRun(async () => { await stopOneTouch(); otTab = 'family'; }); otLoadPeople(); });

// ---- the family: the members with their own login, and inviting one ----
const OT_FAMILY_MAX = 10;
function otRenderFamily() {
  const list = $('ot-family-list');
  if (!list) return;
  list.replaceChildren();
  if (otPeopleState.phase === 'loading' && !otPeople.length) {
    $('ot-family-count').textContent = 'Loading family members…';
    $('ot-family-invite').disabled = true;
    return;
  }
  if (otPeopleState.phase === 'error') {
    $('ot-family-count').textContent = `Could not load the family members: ${otPeopleState.error}`;
    const retry = document.createElement('button'); retry.type = 'button'; retry.textContent = 'Try again';
    retry.addEventListener('click', otLoadPeople);
    list.appendChild(retry);
    $('ot-family-invite').disabled = true;
    return;
  }
  for (const p of otPeople) {
    const row = document.createElement('div'); row.className = 'act-item';
    const pic = document.createElement('span'); pic.className = 'act-pic icon'; pic.innerHTML = OT_ICON.family;
    const text = document.createElement('span'); text.className = 'act-text';
    const name = document.createElement('strong'); name.textContent = p.name;
    const sub = document.createElement('span'); sub.className = 'muted small';
    sub.textContent = [p.email, p.sip ? `number ${p.sip}` : ''].filter(Boolean).join(' · ') || 'Their own OneTouch login';
    text.append(name, sub);
    const remove = document.createElement('button'); remove.type = 'button'; remove.className = 'small danger-text'; remove.textContent = 'Remove';
    remove.setAttribute('aria-label', `Remove ${p.name} from the family`);
    remove.addEventListener('click', async () => {
      if (!(await KagazAsk.confirm(`Remove ${p.name}?`, 'Remove', 'They lose these devices in their OneTouch app; their own account stays.'))) return;
      try {
        await invoke('onetouch_family_remove', { id: p.id || '' });
        await otLoadPeople(); otRenderFamily();
        $('ot-family-note').textContent = `${p.name} was removed.`;
      } catch (e) {
        $('ot-family-note').textContent = otFamilyFailed(e, 'remove');
      }
    });
    row.append(pic, text, remove);
    list.appendChild(row);
  }
  const n = otPeople.length;
  $('ot-family-count').textContent = n ? `${n} of ${OT_FAMILY_MAX} members.` : 'No family members yet.';
  $('ot-family-invite').disabled = n >= OT_FAMILY_MAX;
  $('ot-family-note').textContent = n >= OT_FAMILY_MAX ? `OneTouch allows ${OT_FAMILY_MAX} members; remove one to invite another.` : '';
}
// Inviting and removing go through OneTouch's own cloud, with its words on failure.
function otFamilyFailed(e, what) {
  const msg = String(e && e.message || e);
  return /^OneTouch did not/.test(msg) ? msg : `Could not ${what} the member: ${msg}`;
}
// The invite form follows OneTouch's own "New Family Member" form, field for field:
// First Name, Last Name, Email, Remark, Country / Region, Mobile Number. It is sent
// from inside the dialog, so a refusal shows under the form with everything kept.
$('ot-family-invite').addEventListener('click', async () => {
  const done = await KagazAsk.ask((dlg, finish) => {
    dlg.classList.add('member-form');
    const form = document.createElement('form'); form.method = 'dialog'; form.noValidate = true;
    const title = document.createElement('p'); title.className = 'ask-title'; title.textContent = 'New family member';
    const errs = {};
    const field = (key, label, input, hint) => {
      const wrap = document.createElement('div'); wrap.className = 'mf-field';
      const l = document.createElement('label'); l.textContent = label; l.htmlFor = input.id;
      const err = document.createElement('p'); err.className = 'mf-error'; err.id = `${input.id}-err`; err.setAttribute('role', 'alert');
      input.setAttribute('aria-describedby', err.id);
      wrap.append(l, input);
      if (hint) wrap.appendChild(hint);
      wrap.appendChild(err);
      errs[key] = (msg) => { err.textContent = msg || ''; input.toggleAttribute('aria-invalid', !!msg); };
      return wrap;
    };
    const text = (id, ac, max, extra = {}) => { const i = document.createElement('input'); i.id = id; i.type = extra.type || 'text'; i.autocomplete = ac; i.maxLength = max; if (extra.inputmode) i.inputMode = extra.inputmode; return i; };
    const first = text('mf-first', 'given-name', 64);
    const last = text('mf-last', 'family-name', 64);
    const email = text('mf-email', 'email', 128, { type: 'email', inputmode: 'email' });
    const remark = document.createElement('textarea'); remark.id = 'mf-remark'; remark.maxLength = 255; remark.rows = 2;
    const count = document.createElement('span'); count.className = 'mf-count muted small'; count.textContent = '0/255';
    remark.addEventListener('input', () => { count.textContent = `${remark.value.length}/255`; });
    const country = KagazCountries.select('mf-country'); country.autocomplete = 'tel-country-code';
    const mobile = text('mf-mobile', 'tel-national', 20, { type: 'tel', inputmode: 'tel' });
    const note = document.createElement('p'); note.className = 'muted small'; note.textContent = 'OneTouch makes them a login on this home and sends it to them: by email, or by text message when only a mobile number is given.';
    const formErr = document.createElement('p'); formErr.className = 'mf-error'; formErr.setAttribute('role', 'alert');
    const row = document.createElement('div'); row.className = 'actions';
    const cancel = document.createElement('button'); cancel.type = 'button'; cancel.textContent = 'Cancel'; cancel.addEventListener('click', () => finish(null));
    const submit = document.createElement('button'); submit.type = 'submit'; submit.className = 'accent'; submit.textContent = 'Invite';
    row.append(cancel, submit);
    form.append(title,
      field('first', 'First name', first),
      field('last', 'Last name', last),
      field('email', 'Email', email),
      field('remark', 'Remark (optional)', remark, count),
      field('country', 'Country / Region', country),
      field('mobile', 'Mobile number', mobile),
      note, formErr, row);
    // The same checks as OneTouch's form, shown under each field.
    const check = () => {
      for (const k in errs) errs[k]('');
      const em = email.value.trim(), mob = mobile.value.replace(/\D/g, '');
      let firstBad = null;
      const bad = (k, el, msg) => { errs[k](msg); firstBad = firstBad || el; };
      if (!first.value.trim()) bad('first', first, 'Please enter the first name.');
      if (!last.value.trim()) bad('last', last, 'Please enter the last name.');
      if (em && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(em)) bad('email', email, 'That does not look like an email address.');
      if (!em && !mob) bad('email', email, 'Give an email address or a mobile number.');
      if (mobile.value.trim() && (mob.length < 6 || mob.length > 15)) bad('mobile', mobile, 'That does not look like a mobile number.');
      if (mob && !country.value) bad('country', country, 'Choose the country or region of the mobile number.');
      if (firstBad) firstBad.focus();
      return !firstBad;
    };
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      formErr.textContent = '';
      if (!check()) return;
      const member = {
        first_name: first.value.trim(), last_name: last.value.trim(), email: email.value.trim(), remark: remark.value.trim(),
        phone_code: mobile.value.trim() ? KagazCountries.codeOf(country.value) : '', mobile: mobile.value.trim(),
      };
      submit.disabled = cancel.disabled = true; submit.textContent = 'Inviting…';
      try {
        const r = await invoke('onetouch_family_invite', { member });
        if (member.mobile) KagazCountries.remember(country.value);
        finish({ member, r });
      } catch (err) {
        formErr.textContent = otFamilyFailed(err, 'invite');
        submit.disabled = cancel.disabled = false; submit.textContent = 'Invite';
      }
    });
    dlg.appendChild(form);
    setTimeout(() => first.focus(), 0);
  });
  if (!done) return;
  const { member, r } = done;
  const who = `${member.first_name} ${member.last_name}`.trim();
  await otLoadPeople();
  $('ot-family-note').textContent = r && r.emailed === false
    ? `${who} was added; OneTouch sent their login by text message.`
    : `${who} was added; OneTouch emailed their login to ${member.email}.`;
});
$('ot-recordings-tab').addEventListener('click', () => otRun(async () => {
  otTab = 'recordings';
  await stopOneTouch();
  // An older OTIPVDP account has its own list; Smart Cloud shows the history.
  const legacy = otStatus?.provider === 'otipvdp';
  $('ot-legacy-history').classList.toggle('hidden', !legacy);
  $('ot-history').classList.toggle('hidden', legacy);
  if (legacy) await otLoadRecordings(1);
  else KagazHistory.show();
}, ''));
$('ot-recordings-refresh').addEventListener('click', () => otRun(() => otLoadRecordings(otPage), 'Refreshing recordings…'));
$('ot-prev').addEventListener('click', () => otRun(() => otLoadRecordings(Math.max(1, otPage - 1)), 'Loading recordings…'));
$('ot-next').addEventListener('click', () => otRun(() => otLoadRecordings(otPage + 1), 'Loading recordings…'));
$('ot-stop').addEventListener('click', stopOneTouch);
$('ot-video').addEventListener('playing', () => { clearTimeout(otTimer); $('ot-player-note').textContent = ''; $('ot-message').textContent = ''; });
$('ot-video').addEventListener('error', () => {
  if ($('ot-video').getAttribute('src')) $('ot-player-note').textContent = 'Video is unavailable or the link has expired. Refresh and try again.';
});
$('ot-picture').addEventListener('load', () => { $('ot-player-note').textContent = ''; });
$('ot-picture').addEventListener('error', () => { if ($('ot-picture').getAttribute('src')) $('ot-player-note').textContent = 'Snapshot unavailable. Refresh the recordings list and try again.'; });

$('ot-provider').addEventListener('change', () => {
  $('ot-legacy-settings').classList.toggle('hidden', $('ot-provider').value !== 'otipvdp');
  $('ot-session-note').classList.toggle('hidden', $('ot-provider').value !== 'smart_cloud');
  $('ot-password').value = ''; otPasswordVisible(false); $('ot-message').textContent = '';
});
