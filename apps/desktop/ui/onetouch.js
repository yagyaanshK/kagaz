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
$('ot-menu').addEventListener('click', (ev) => { ev.stopPropagation(); $('ot-actions').classList.toggle('hidden'); });
document.addEventListener('click', (ev) => {
  if (!ev.target.closest('#ot-menu-box')) $('ot-actions').classList.add('hidden');
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
  $('ot-signin').classList.toggle('hidden', signed);
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
  for (const [id, on] of [['ot-live-tab', otTab === 'live'], ['ot-recordings-tab', otTab === 'recordings']]) {
    $(id).setAttribute('aria-pressed', String(on)); $(id).classList.toggle('active', on);
  }
  for (const id of ['ot-reconnect','ot-forget','ot-remember','ot-signin','ot-signout','ot-refresh','ot-login-submit','ot-login-cancel','ot-live-tab','ot-recordings-tab','ot-recordings-refresh','ot-provider','ot-legacy-device','ot-username','ot-password','ot-password-toggle']) $(id).disabled = otBusy;
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
const OT_ICON = {
  door: '<svg viewBox="0 0 24 24"><path d="M6 3h12v18H6zM10 8h4M12 13a1.5 1.5 0 1 0 0 .01M9 18h6"/></svg>',
  indoor: '<svg viewBox="0 0 24 24"><path d="M3 5h18v11H3zM8 20h8M12 16v4"/></svg>',
  guard: '<svg viewBox="0 0 24 24"><path d="M12 3l8 3v6c0 4.5-3.5 8-8 9-4.5-1-8-4.5-8-9V6z"/></svg>',
  apps: '<svg viewBox="0 0 24 24"><path d="M8 3h8v18H8zM11 18h2"/></svg>',
  family: '<svg viewBox="0 0 24 24"><path d="M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM2 21v-1a7 7 0 0 1 14 0v1M17 11a3 3 0 1 0 0-6M19 21v-1a5 5 0 0 0-3-4.6"/></svg>',
  other: '<svg viewBox="0 0 24 24"><path d="M4 4h16v16H4z"/></svg>',
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
    const el = document.createElement("div");
    el.className = "dev-card";
    el.dataset.key = c.key;
    el.dataset.name = c.name.trim().toLowerCase();
    el.classList.toggle("open", open);
    el.classList.toggle("watching", !!watching);
    el.classList.toggle("offline", !c.online);

    const head = document.createElement("button");
    head.type = "button";
    head.className = "dev-head";
    head.setAttribute("aria-expanded", String(open));
    const icon = document.createElement("span");
    icon.className = "dev-icon";
    icon.innerHTML = OT_ICON[c.kind] || OT_ICON.other;
    const text = document.createElement("span");
    text.className = "dev-text";
    const name = document.createElement("strong");
    name.textContent = c.name;
    const sub = document.createElement("span");
    sub.className = "muted small";
    const canWatch = !!(c.live && c.live.can_monitor);
    sub.textContent = [OT_KIND[c.kind] || OT_KIND.other, c.online ? "" : "offline", canWatch ? "live view" : ""].filter(Boolean).join(" · ");
    text.append(name, sub);
    const go = document.createElement("span");
    go.className = "dev-go";
    go.setAttribute("aria-hidden", "true");
    go.textContent = open ? "✕" : canWatch ? "▶" : "›";
    head.append(icon, text, go);
    head.disabled = otBusy;
    head.title = open ? "Close" : canWatch ? "Watch live" : "Open";
    head.addEventListener("click", () => (open ? otClose() : otOpenCard(c)));
    el.append(head);

    const slot = document.createElement("div");
    slot.className = "dev-slot";
    el.append(slot);
    if (watching) slot.append(otLive.stage.el);

    if (open) {
      const actions = document.createElement("div");
      actions.className = "dev-actions";
      const note = document.createElement("p");
      note.className = "muted small dev-note";
      if (c.call && !busy) {
        const call = document.createElement("button");
        call.type = "button";
        call.className = "accent big";
        call.textContent = c.kind === "apps" ? "Ring them" : c.kind === "family" ? `Call ${c.name.split(" ")[0]}` : "Call";
        call.disabled = !lineUp || busy;
        call.addEventListener("click", () => window.KagazCalls.call(c.call.sip, c.key));
        actions.append(call);
        if (!lineUp) note.textContent = calls && calls.line === "off" ? "Turn on “Ring me here” above to make calls." : "Calls connect once the call line is up (see above).";
      }
      if (watching) {
        note.textContent = [note.textContent, "Live view is picture only: the door sends its sound in a call."].filter(Boolean).join(" ");
      } else if (!canWatch) {
        note.textContent = [note.textContent,
          c.live ? "OneTouch does not allow live view of this device."
          : c.kind === "apps" ? "Every phone and tablet signed in with your own login shares one number, so they ring together. Family members with their own login are listed separately and ring on their own."
          : c.kind === "family" && !c.call ? "OneTouch did not give a call number for this member yet."
          : ""].filter(Boolean).join(" ");
      }
      actions.append(note);
      el.append(actions);
    }
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

// Home: open a device's card by its name once the account is loaded.
async function otOpenByName(name) {
  const want = (name || "").trim().toLowerCase();
  for (let i = 0; i < 40 && !(otStatus && otStatus.devices); i++) await new Promise((r) => setTimeout(r, 150));
  const card = otCards().find((c) => c.name.trim().toLowerCase() === want);
  if (card) await otOpenCard(card);
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
async function otLoadPeople() {
  if (!otStatus?.logged_in) { otPeople = []; return; }
  try { otPeople = await invoke('onetouch_people'); } catch (_) { otPeople = []; }
  otRenderDevices();
}
$('ot-signin').addEventListener('click', () => {
  if (otStatus?.remembered) {
    $('ot-username').value = otStatus.remembered.username;
    $('ot-provider').value = otStatus.remembered.provider;
    $('ot-legacy-device').value = otStatus.remembered.legacy_device || '';
    $('ot-provider').dispatchEvent(new Event('change'));
  }
  otPasswordVisible(false); $('ot-login').classList.remove('hidden'); $('ot-message').textContent = ''; $('ot-username').focus();
  KagazNav.push('ot-login', () => $('ot-login-cancel').click());
});
$('ot-login-cancel').addEventListener('click', () => {
  $('ot-password').value = ''; otPasswordVisible(false); $('ot-login').classList.add('hidden'); $('ot-message').textContent = '';
  KagazNav.pop('ot-login');
});
$('ot-login').addEventListener('submit', async e => {
  e.preventDefault();
  const username = $('ot-username').value.trim(); const password = $('ot-password').value;
  if (!username || !password) { $('ot-message').textContent = 'Enter your OneTouch username and password.'; return; }
  otPasswordVisible(false);
  await otRun(async () => {
    try { otStatus = await invoke('onetouch_login', { username, password, provider: $('ot-provider').value, legacyDevice: $('ot-legacy-device').value.trim(), remember: otStatus?.remember_supported !== false && $('ot-remember').checked }); }
    finally { $('ot-password').value = ''; otPasswordVisible(false); }
    otInitialized = true; otTab = 'live'; otPage = 1;
    $('ot-login').classList.add('hidden'); $('ot-message').textContent = otStatus?.notice || ''; KagazNav.pop('ot-login');
  }, `Signing in to ${$('ot-provider').selectedOptions[0].textContent}…`);
});
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
}, 'Reconnecting to OneTouch…'));
$('ot-forget').addEventListener('click', () => otRun(async () => {
  otStatus = await invoke('onetouch_forget');
  $('ot-password').value = ''; $('ot-username').value = ''; otPasswordVisible(false);
  $('ot-message').textContent = 'Saved sign-in removed.';
}));
$('ot-refresh').addEventListener('click', () => otRun(async () => {
  otStatus = await invoke('onetouch_refresh'); otInitialized = true; $('ot-message').textContent = '';
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
  const stage = new KagazPlayer.Stage(document.createDocumentFragment(), { title: device.name, badge: "LIVE", sound: false, label: `doorbell ${device.name}` });
  stage.setNote("Connecting…");
  const live = { key: card.key, deviceId: device.id, stage, handle: null, timer: null };
  otLive = live;
  otRenderDevices();
  // The picture says what is happening until the first picture shows.
  live.timer = setInterval(() => {
    if (otLive !== live) return;
    if (live.stage.player.stats.shown === 0) stage.setNote(live.stage.player.note());
  }, 500);
  stage.player.onended = (why) => {
    if (otLive !== live) return;
    clearInterval(live.timer);
    stage.setIdle(true);
    stage.setBadge("");
    stage.setNote(why ? `Live view ended: ${why}. Tap ▶ to watch again.` : "Live view ended.");
  };
  $('ot-message').textContent = '';
  try {
    live.handle = await KagazPlayer.watch(stage.player, 'onetouch_watch', { deviceId: device.id });
    if (otLive !== live) live.handle.stop();
  } catch (e) {
    if (otLive === live) { clearInterval(live.timer); stage.setIdle(true); stage.setBadge(""); stage.setNote(String(e)); }
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
