// OneTouch account/video UI. Account credentials stay in Rust; this receives
// device metadata and only the selected recording's temporary media URL.
let otStatus = null;
let otBusy = false;
let otTab = 'live';
let otPage = 1;
let otMore = false;
let otPlayback = 0;
let otTimer = null;
let otInitialized = false;
let otLivePlayer = null;

function otPasswordVisible(visible) {
  $('ot-password').type = visible ? 'text' : 'password';
  $('ot-password-toggle').textContent = visible ? 'Hide' : 'Show';
  $('ot-password-toggle').setAttribute('aria-label', visible ? 'Hide password' : 'Show password');
}
$('ot-password-toggle').addEventListener('click', () => {
  otPasswordVisible($('ot-password').type === 'password');
});

function otRender() {
  const signed = !!otStatus?.logged_in;
  const saved = otStatus?.remembered;
  const canRemember = otStatus?.remember_supported !== false;
  $('ot-reconnect').classList.toggle('hidden', signed || !saved);
  $('ot-forget').classList.toggle('hidden', !saved && !otStatus?.notice);
  $('ot-remembered').classList.toggle('hidden', !saved);
  $('ot-remembered').textContent = saved ? `Saved sign-in: ${saved.username}. Reconnect happens only when you click it.` : '';
  $('ot-signin').textContent = saved ? 'Use another account' : 'Sign in';
  $('ot-remember-label').classList.toggle('hidden', !canRemember);
  $('ot-remember-help').classList.toggle('hidden', !canRemember);
  $('ot-signin').classList.toggle('hidden', signed);
  $('ot-signout').classList.toggle('hidden', !signed);
  $('ot-refresh').classList.toggle('hidden', !signed);
  $('ot-connected').classList.toggle('hidden', !signed);
  $('ot-service-name').textContent = signed ? otStatus.service_name : 'OneTouch';
  $('ot-account-name').textContent = signed ? `Signed in as ${otStatus.username}` : 'Live video and recordings through your OneTouch account.';
  const list = $('ot-devices');
  list.replaceChildren();
  for (const device of otStatus?.devices || []) {
    const card = document.createElement('div'); card.className = 'ot-device';
    const name = document.createElement('strong'); name.textContent = device.name;
    const note = document.createElement('span'); note.className = 'muted small';
    note.textContent = device.can_monitor ? 'Live view available' : 'Live monitoring is not enabled for this device';
    const button = document.createElement('button'); button.type = 'button'; button.textContent = 'Watch live';
    button.disabled = otBusy || !device.can_monitor;
    button.addEventListener('click', () => otWatch(device));
    card.append(name, note, button); list.append(card);
  }
  if (signed && !(otStatus.devices || []).length) {
    const note = document.createElement('p'); note.className = 'muted'; note.textContent = 'No doorbells loaded. Try Refresh devices.'; list.append(note);
  }
  $('ot-devices').classList.toggle('hidden', otTab !== 'live');
  $('ot-recordings-pane').classList.toggle('hidden', otTab !== 'recordings');
  $('ot-live-tab').setAttribute('aria-pressed', String(otTab === 'live'));
  $('ot-recordings-tab').setAttribute('aria-pressed', String(otTab === 'recordings'));
  for (const id of ['ot-reconnect','ot-forget','ot-remember','ot-signin','ot-signout','ot-refresh','ot-login-submit','ot-login-cancel','ot-live-tab','ot-recordings-tab','ot-recordings-refresh','ot-provider','ot-legacy-device','ot-username','ot-password','ot-password-toggle']) $(id).disabled = otBusy;
  $('ot-prev').disabled = otBusy || otPage <= 1;
  $('ot-next').disabled = otBusy || !otMore;
  document.querySelectorAll('#ot-recordings button').forEach(b => { b.disabled = otBusy; });
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
}
$('ot-signin').addEventListener('click', () => {
  if (otStatus?.remembered) {
    $('ot-username').value = otStatus.remembered.username;
    $('ot-provider').value = otStatus.remembered.provider;
    $('ot-legacy-device').value = otStatus.remembered.legacy_device || '';
    $('ot-provider').dispatchEvent(new Event('change'));
  }
  otPasswordVisible(false); $('ot-login').classList.remove('hidden'); $('ot-message').textContent = ''; $('ot-username').focus();
});
$('ot-login-cancel').addEventListener('click', () => {
  $('ot-password').value = ''; otPasswordVisible(false); $('ot-login').classList.add('hidden'); $('ot-message').textContent = '';
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
    $('ot-login').classList.add('hidden'); $('ot-message').textContent = otStatus?.notice || '';
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

async function stopOneTouch() {
  otPlayback++;
  clearTimeout(otTimer); otTimer = null;
  if (otLivePlayer) {
    clearTimeout(otLivePlayer.reconnectTID);
    clearTimeout(otLivePlayer.disconnectTID);
    otLivePlayer.ondisconnect();
    otLivePlayer.remove(); otLivePlayer = null;
  }
  $('ot-live-video').classList.add('hidden');
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
// Use the engine's pinned MSE player for live H.264. WebKit rejects this
// relay's raw MPEG-TS input even when a standalone decoder can play it.
async function otPlayLive(url, generation) {
  const source = new URL(url);
  if (source.protocol !== 'ws:' || source.hostname !== '127.0.0.1') throw new Error('Invalid local video connection.');
  if (!customElements.get('onetouch-live-player')) {
    const { VideoRTC } = await import(`http://${source.host}/video-rtc.js`);
    if (!customElements.get('onetouch-live-player')) customElements.define('onetouch-live-player', class extends VideoRTC {});
  }
  if (generation !== otPlayback) return;
  const player = document.createElement('onetouch-live-player');
  player.mode = 'mse';
  player.visibilityCheck = false;
  player.DISCONNECT_TIMEOUT = 0;
  player.src = url;
  otLivePlayer = player;
  $('ot-video').classList.add('hidden');
  $('ot-live-video').classList.remove('hidden');
  $('ot-live-video').replaceChildren(player);
  const v = player.video;
  let lastTime = -1, lastProgress = Date.now();
  const current = () => generation === otPlayback && otLivePlayer === player;
  v.addEventListener('playing', () => {
    if (current()) { $('ot-player-note').textContent = ''; lastProgress = Date.now(); }
  });
  v.addEventListener('error', () => {
    if (current()) $('ot-player-note').textContent = 'Live video playback failed. Close video and try again.';
  });
  otTimer = setInterval(() => {
    if (!current() || v.error) return;
    if (v.currentTime > lastTime && v.readyState >= 2) {
      lastTime = v.currentTime; lastProgress = Date.now(); $('ot-player-note').textContent = '';
    } else if (v.paused && v.readyState >= 2) {
      lastProgress = Date.now();
    } else if (Date.now() - lastProgress > 20000) {
      $('ot-player-note').textContent = 'No video received. Close video and try again.';
    }
  }, 1000);
}
async function otWatch(device) {
  await otRun(async () => {
    await stopOneTouch(); const generation = otPlayback;
    otShowPlayer(device.name);
    const url = await invoke('onetouch_live', { deviceId: device.id });
    if (generation !== otPlayback) { await invoke('onetouch_stop'); return; }
    $('ot-message').textContent = '';
    await otPlayLive(url, generation);
  }, 'Opening live video…');
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
        if (picture) $('ot-picture').src = url; else await otPlayVideo(url, generation);
      }));
      row.append(button);
    }
    container.append(row);
  }
  $('ot-message').textContent = list.message || (list.entries.length ? '' : 'No cloud recordings or snapshots on this page. Footage stored only on an indoor monitor is not included.');
}
$('ot-live-tab').addEventListener('click', () => otRun(async () => { await stopOneTouch(); otTab = 'live'; }));
$('ot-recordings-tab').addEventListener('click', () => otRun(async () => { otTab = 'recordings'; await otLoadRecordings(1); }, 'Loading recordings…'));
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
