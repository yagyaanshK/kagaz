// Doorbell calls through the OneTouch cloud: the account's app line rings
// here. A ring brings up the call card at the top of the Doorbell pane; a
// call shows the caller's camera, plays their voice (levelled) and sends
// the microphone, and this device's camera when the other side takes video
// (call-camera.js): on from the start with a person's own phone or monitor,
// off with a door station or the guard (shared devices) until turned on,
// and when the other person's camera comes on, Kagaz offers to turn its
// own on too. The devices to call are the Doorbell pane's cards
// (onetouch.js), which call through KagazCalls.call; a call made from a
// card plays inside that card. The picture and voice go through the
// window's player (player.js). Uses app.js's globals: $, invoke, listen,
// selectCategory, startChime, stopChime.

(() => {
  // The call's buttons: a symbol, with its word beside it where there is
  // room (on a phone the symbol alone; the word stays its name for
  // screen readers).
  const ICON = {
    phone: '<path d="M6.5 3.5h3l1.5 4.5-2 1.3a11 11 0 0 0 5.7 5.7l1.3-2 4.5 1.5v3a2 2 0 0 1-2 2A16.5 16.5 0 0 1 4.5 5.5a2 2 0 0 1 2-2z"/>',
    mic: '<path d="M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3zM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/>',
    micOff: '<path d="M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3zM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21M4 4l16 16"/>',
    speaker: '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z"/><path d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11"/>',
    camera: '<rect x="3" y="7" width="12.5" height="10" rx="2"/><path d="M15.5 10.5 21 7.5v9l-5.5-3"/>',
    cameraOff: '<rect x="3" y="7" width="12.5" height="10" rx="2"/><path d="M15.5 10.5 21 7.5v9l-5.5-3M3 4l18 16"/>',
    unlock: '<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7.5a4 4 0 0 1 7.7-1.5M12 15v2"/>',
  };
  const svg = (name) => `<svg viewBox="0 0 24 24" aria-hidden="true">${ICON[name]}</svg>`;
  function btn(id, cls, icon, label, extra = "") {
    const down = icon === "phone down";
    return `<button id="${id}" type="button" class="${cls}" aria-label="${label}" title="${label}" ${extra}>`
      + `<span class="ci${down ? " ci-down" : ""}">${svg(down ? "phone" : icon)}</span><span class="cl">${label}</span></button>`;
  }
  // A toggle's symbol, word and state.
  function setToggle(id, on, icon, label, title) {
    const b = $(id);
    b.classList.toggle("on", on);
    b.setAttribute("aria-pressed", String(on));
    b.querySelector(".ci").innerHTML = svg(icon);
    b.querySelector(".cl").textContent = label;
    b.title = title;
  }

  const PANEL = `
      <div id="calls-panel" class="calls-panel">
        <div class="calls-head">
          <p id="calls-line" class="muted small"></p>
          <label class="switch" title="Ring here when someone calls the OneTouch account">
            <input id="calls-enabled" type="checkbox"> <span>Ring me here</span>
          </label>
        </div>
        <p id="calls-note" class="small cap-warn" role="alert"></p>
        <div id="calls-retry-row" class="actions hidden">
          <button id="calls-retry" type="button">Try again</button>
        </div>
        <div id="call-slot"></div>
      </div>`;
  const CARD = `
      <div id="call-card" class="hidden">
        <div id="call-video" class="call-video">
          <div id="call-video-note" class="call-video-note"></div>
          <span id="call-clock" class="call-clock hidden">0:00</span>
          <video id="call-self" class="call-self hidden" muted playsinline aria-label="Your camera"></video>
        </div>
        <div class="call-body">
          <div class="call-who">
            <span id="call-peer"></span>
            <span id="call-state" class="muted"></span>
          </div>
          <div class="call-actions">
            ${btn("call-decline", "big cbtn danger", "phone down", "Decline")}
            ${btn("call-answer", "big cbtn ok", "phone", "Answer")}
            ${btn("call-mute", "big cbtn", "mic", "Mute", 'aria-pressed="false"')}
            ${btn("call-speaker", "big cbtn hidden on", "speaker", "Speaker", 'aria-pressed="true"')}
            ${btn("call-camera", "big cbtn", "cameraOff", "Camera", 'aria-pressed="false"')}
            ${btn("call-open", "big cbtn accent", "unlock", "Open door")}
            ${btn("call-hangup", "big cbtn danger", "phone down", "Hang up")}
          </div>
          <label class="call-volume" title="How loud their voice plays here">Volume
            <input id="call-volume" type="range" min="0.5" max="3" step="0.1" value="1">
          </label>
          <div id="call-ask" class="call-ask hidden" role="alert">
            <span id="call-ask-text"></span>
            <div class="actions">
              <button id="call-ask-yes" type="button" class="accent">Turn my camera on</button>
              <button id="call-ask-no" type="button">Not now</button>
            </div>
          </div>
          <p id="call-note" class="muted small"></p>
        </div>
      </div>`;
  const DIAL = `
      <details id="ot-dial-box" class="calls-more">
        <summary>Call another number</summary>
        <form id="calls-dial" class="calls-dial">
          <label for="calls-number" class="small">Number</label>
          <input id="calls-number" type="text" inputmode="numeric" autocomplete="off" placeholder="e.g. a family member's app">
          <button type="submit">Call</button>
        </form>
        <p id="calls-dial-note" class="small cap-warn" role="alert"></p>
      </details>`;

  $("calls-host").innerHTML = PANEL;
  $("call-slot").innerHTML = CARD;
  $("ot-devices").insertAdjacentHTML("afterend", DIAL);

  let status = null;
  let clockTimer = null;
  let ringing = false;
  let focused = false; // the page went to this call because its notification was opened

  // ---------- status ----------

  const LINE_TEXT = {
    off: "Off. Turn on to ring here when someone calls your OneTouch account.",
    connecting: "Connecting to OneTouch…",
    up: "Ringing here for OneTouch calls.",
    down: "Not connected",
    signed_out: "Sign in to OneTouch to take its calls here.",
    expired: "The OneTouch sign-in has expired.",
  };

  function render(s) {
    status = s;
    $("calls-enabled").checked = !!s.enabled;
    // Ringing here needs the account: nothing to turn on while signed out.
    $("calls-enabled").disabled = s.line === "signed_out" && !s.enabled;
    const base = LINE_TEXT[s.line] || s.line || "";
    // The detail adds to the state's words; for "signed out" it only repeats them.
    $("calls-line").textContent = s.detail && !["up", "signed_out"].includes(s.line) ? `${base} ${s.detail}`.trim() : base;
    $("calls-line").classList.toggle("cap-ok", s.line === "up");
    $("calls-line").classList.toggle("cap-warn", ["down", "expired", "signed_out"].includes(s.line));
    $("calls-retry-row").classList.toggle("hidden", !["down", "expired", "signed_out"].includes(s.line) || !s.enabled);
    $("calls-dial").querySelector("button").disabled = s.line !== "up" || (s.phase && s.phase !== "idle");
    renderCard(s);
    // The device cards show Call buttons by the line's state, and hold the call.
    if (typeof otRenderDevices === "function") otRenderDevices();
    else place();
  }

  // ---------- where the call card goes ----------

  let callKey = null; // the card a call was made from

  function keyForPeer(peer) {
    const name = (peer || "").trim().toLowerCase();
    if (!name) return null;
    const el = document.querySelector(`.dev-card[data-name="${CSS.escape(name)}"]`);
    return el ? el.dataset.key : null;
  }

  // A ring shows at the top of the pane; a call made or answered plays in
  // its device's card when there is one.
  // On a phone a call covers the screen (style.css): it hangs from the
  // page itself, so no card it was made from can show through or clip it.
  const PHONE_LAYOUT = window.matchMedia("(max-width: 599px), (pointer: coarse) and (max-height: 500px)");
  PHONE_LAYOUT.addEventListener("change", () => place());

  function place() {
    const card = $("call-card");
    const phase = (status && status.phase) || "idle";
    const shown = phase !== "idle" || !!card.dataset.ended;
    document.body.classList.toggle("in-call", shown && PHONE_LAYOUT.matches);
    if (shown && PHONE_LAYOUT.matches) {
      if (card.parentElement !== document.body) document.body.appendChild(card);
      return;
    }
    let slot = null;
    if (phase === "calling" || phase === "talking" || card.dataset.ended) {
      // Only into a card that is open (full width): a closed one is a small
      // grid cell, too narrow for the picture and the call's buttons.
      const key = callKey || keyForPeer(status && status.peer);
      if (key) slot = document.querySelector(`.dev-card.open[data-key="${CSS.escape(key)}"] .dev-slot`);
    }
    slot = slot || $("call-slot");
    if (card.parentElement !== slot) slot.appendChild(card);
  }

  function call(number, key) {
    KagazPlayer.audioContext(); // a click is what lets the page make sound
    callKey = key || null;
    // The relay cannot show the live view and a call at once.
    if (typeof otStopLive === "function") otStopLive();
    $("calls-dial-note").textContent = "";
    invoke("calls_call", { number }).catch((e) => {
      callKey = null;
      note(String(e));
    });
  }

  window.KagazCalls = { status: () => status, call, place };

  function renderCard(s) {
    const phase = s.phase || "idle";
    const card = $("call-card");
    const active = phase !== "idle";
    card.classList.toggle("hidden", !active && !card.dataset.ended);
    card.classList.toggle("ringing", phase === "ringing");
    $("call-peer").textContent = s.peer || "";
    $("call-state").textContent =
      phase === "ringing" ? "is calling" : phase === "calling" ? s.detail === "ringing there" ? "ringing…" : "calling…" : phase === "talking" ? "" : s.detail || "";
    show("call-answer", phase === "ringing");
    show("call-decline", phase === "ringing");
    show("call-open", phase === "talking" && !!s.door_dtmf);
    show("call-mute", phase === "talking");
    show("call-speaker", phase === "talking" && ON_PHONE);
    show("call-camera", phase === "talking" && !!s.send_video);
    show("call-hangup", phase === "talking" || phase === "calling");
    $("call-clock").classList.toggle("hidden", phase !== "talking");

    if (phase === "ringing" && !ringing) {
      ringing = true;
      // Ring mode brings the ring up; silent mode leaves the screen alone
      // (its notification opens the ring when tapped: s.focus below).
      if (!s.silent && typeof selectCategory === "function") selectCategory("doorbell");
      // The caller's tone (Settings). On a phone the ring notification
      // plays it, also when the window is closed; the page stays quiet.
      const peer = (s.peer || "").trim().toLowerCase();
      const dev = (s.devices || []).find((d) => d.name.trim().toLowerCase() === peer);
      const family = typeof otPeople !== "undefined" && otPeople.some((p) => p.name.trim().toLowerCase() === peer);
      const kind = (dev && dev.kind === "apps") || family ? "family" : "doorbell";
      if (!s.silent && typeof startChime === "function" && !/Android/i.test(navigator.userAgent)) startChime(s.peer, kind);
      $("call-video-note").textContent = s.video ? "Answer to see the camera" : "";
      card.dataset.ended = "";
    }
    // The caller's live view while it rings, and after answering until the
    // call's own picture shows.
    if (phase === "ringing" || (phase === "talking" && !(player && player.stats.shown))) startPreview();
    else stopPreview();
    // The ring's notification was opened: show the ring (once per call).
    if (s.focus && (phase === "ringing" || phase === "talking") && !focused) {
      focused = true;
      if (typeof selectCategory === "function") selectCategory("doorbell");
    }
    if (phase === "idle") focused = false;
    if (phase !== "ringing" && ringing) {
      ringing = false;
      if (typeof stopChime === "function") stopChime();
    }
    if ((phase === "calling" || phase === "talking") && typeof otStopLive === "function") otStopLive();
    if (phase === "talking") {
      startClock(s.since);
      startAudio();
      startPlayer();
      // A person's own device: video both ways from the start.
      if (s.send_video && !camDecided) {
        camDecided = true;
        if (personal(s)) startCamera();
      }
      if (!s.send_video && (cam || camStarting)) stopCamera();
      if (!s.video && !preview) $("call-video-note").textContent = "No camera on this call";
    } else {
      stopClock();
    }
    if (phase === "idle" && card.dataset.active === "1") {
      // Keep the card a moment to say how it ended.
      card.dataset.active = "";
      card.dataset.ended = "1";
      stopAudio();
      stopPlayer();
      stopCamera();
      camDecided = false;
      camAsked = false;
      $("call-ask").classList.add("hidden");
      $("call-state").textContent = s.detail || "call ended";
      setTimeout(() => {
        card.dataset.ended = "";
        if (status && status.phase === "idle") {
          card.classList.add("hidden");
          callKey = null;
          place();
        }
      }, 4000);
    }
    if (active) card.dataset.active = "1";
  }

  function show(id, on) {
    $(id).classList.toggle("hidden", !on);
  }

  // Under the call when it shows; else at the top of the pane, where it is seen.
  function note(text) {
    const shown = !$("call-card").classList.contains("hidden");
    $("call-note").textContent = shown ? text : "";
    $("calls-note").textContent = shown ? "" : text;
  }

  function startClock(since) {
    if (clockTimer) return;
    const tick = () => {
      const secs = Math.max(0, Math.floor(Date.now() / 1000) - since);
      $("call-clock").textContent = `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}`;
    };
    tick();
    clockTimer = setInterval(tick, 1000);
  }

  function stopClock() {
    clearInterval(clockTimer);
    clockTimer = null;
  }

  // ---------- their picture and voice ----------

  let stage = null; // the call's picture and voice, while talking
  let player = null;
  let videoWatch = null;
  let videoStats = null; // how the call's video is doing, from the line (every 2 s)
  let userVolume = 1;
  try {
    userVolume = Number(localStorage.getItem("kagaz-call-volume")) || 1;
  } catch (e) {}
  $("call-volume").value = userVolume;

  function startPlayer() {
    if (player) return;
    stage = new KagazPlayer.Stage($("call-video"), { title: (status && status.peer) || "", badge: "CALL", sound: true, level: true, volume: userVolume, label: "call" });
    stage.el.classList.add("call-stage");
    player = stage.player;
    $("call-video").classList.add("has-stage");
    // In full screen the card's buttons are out of sight: the two that
    // matter are on the picture too.
    stage.button("Open door", "Open door", () => $("call-open").click(), "kz-word kz-full-only kz-open");
    stage.button("Hang up", "Hang up", () => $("call-hangup").click(), "kz-word kz-full-only kz-danger");
    stage.button("Camera", "Your camera on or off", () => $("call-camera").click(), "kz-word kz-full-only");
    // What happened to the camera on this call, said on the card when no
    // picture shows: nothing arrived, or it arrived and could not be shown.
    videoWatch = setInterval(() => {
      if (!player || !status || status.phase !== "talking") return;
      // The call's own picture is up: the live view has done its job.
      if (player.stats.shown && preview) stopPreview();
      if (preview && preview.stage.player.stats.shown) {
        $("call-video-note").textContent = "";
        return;
      }
      if (status.video) $("call-video-note").textContent = videoNote();
      // Their camera is on and ours is off: offer to turn ours on (a
      // person's own device; a door station's camera is always on).
      if (player.stats.shown && status.send_video && !cam && !camStarting && !camAsked && personal(status)) {
        camAsked = true;
        $("call-ask-text").textContent = `${status.peer || "They"} turned on their camera. Turn yours on too?`;
        $("call-ask").classList.remove("hidden");
      }
    }, 1000);
    if (preview) addCallButtons(preview);
  }

  // Open door and Hang up over a picture, for full screen.
  function addCallButtons(st) {
    if (st.buttons) return;
    st.buttons = true;
    st.stage.button("Open door", "Open door", () => $("call-open").click(), "kz-word kz-full-only kz-open");
    st.stage.button("Hang up", "Hang up", () => $("call-hangup").click(), "kz-word kz-full-only kz-danger");
  }

  // ---------- the caller's live view ----------
  // As in the OneTouch app, the caller's picture shows as soon as it
  // rings: the account's live view of that device (a call's own video
  // starts only once answered). After answering it stays until the call's
  // picture shows, and goes if the relay ends it. Out of sight it stops,
  // and starts again on return.

  let preview = null; // { stage, handle, buttons }
  let previewRetry = null;

  function liveDeviceOf(peer) {
    if (typeof otCards !== "function") return null;
    const name = (peer || "").trim().toLowerCase();
    const card = otCards().find((c) => c.name.trim().toLowerCase() === name && c.live && c.live.can_monitor);
    return card ? card.live : null;
  }

  async function startPreview() {
    if (preview || document.hidden || !status) return;
    const device = liveDeviceOf(status.peer);
    if (!device) {
      // The account's devices may still be loading (the window just opened
      // for this ring): look again shortly.
      if (!previewRetry) previewRetry = setTimeout(() => {
        previewRetry = null;
        if (status && (status.phase === "ringing" || status.phase === "talking")) startPreview();
      }, 1000);
      return;
    }
    if (typeof otStopLive === "function") otStopLive(); // one live view at a time
    const st = new KagazPlayer.Stage($("call-video"), { title: device.name, badge: "LIVE", sound: false, label: `ring ${device.name}` });
    st.el.classList.add("call-preview");
    st.setNote("Connecting to the camera\u2026");
    const p = { stage: st, handle: null, buttons: false };
    preview = p;
    $("call-video").classList.add("has-stage", "previewing");
    $("call-video-note").textContent = "";
    if (status.phase === "talking") addCallButtons(p);
    st.player.onended = () => {
      if (preview !== p) return;
      stopPreview();
      // Ringing: say so on the card; talking: the call's picture takes over.
      if (status && status.phase === "ringing" && status.video) $("call-video-note").textContent = "Answer to see the camera";
    };
    try {
      p.handle = await KagazPlayer.watch(st.player, "onetouch_watch", { deviceId: device.id });
      if (preview !== p) p.handle.stop();
    } catch (e) {
      if (preview === p) st.player.onended();
    }
  }

  function stopPreview() {
    clearTimeout(previewRetry);
    previewRetry = null;
    if (!preview) return;
    const p = preview;
    preview = null;
    if (p.handle) p.handle.stop();
    p.stage.stop();
    $("call-video").classList.remove("previewing");
    if (!stage) $("call-video").classList.remove("has-stage");
  }

  // On a phone the ring's picture stops while Kagaz is out of sight; a
  // computer keeps showing it (app.js, out of sight).
  document.addEventListener("visibilitychange", () => {
    if (!ON_PHONE) return;
    if (document.hidden) stopPreview();
    else if (status) render(status);
  });

  // Why no picture shows, in words: the page's own view once pictures
  // reach it, else what the line saw on the network.
  function videoNote() {
    if (player.stats.shown > 0) return "";
    if (player.stats.received) return player.note();
    const v = videoStats;
    const who = (status && status.peer) || "the caller";
    if (!v || v.seconds < 2) return "Waiting for the camera\u2026";
    const asked = v.asked ? ` Asked for a picture ${v.asked} time${v.asked === 1 ? "" : "s"}.` : "";
    if (!v.packets) return `No video from ${who} has reached this device.${asked}`;
    if (v.other_payload >= v.packets) return `Video arrives (${v.packets} packets) in a format Kagaz does not read.`;
    if (v.damaged && !v.pictures) return `Video arrives (${v.packets} packets), but each full picture lost part on the way (${v.damaged}).${asked}`;
    return `Video arrives (${v.packets} packets), but no full picture yet.${asked}`;
  }

  function stopPlayer() {
    clearInterval(videoWatch);
    videoWatch = null;
    if (stage) stage.stop();
    stage = null;
    player = null;
    videoStats = null;
    $("call-video").classList.remove("has-stage");
    $("call-video-note").textContent = "";
  }

  // The media channel stays open across calls; packets go to the current player.
  const channel = new window.__TAURI__.core.Channel();
  channel.onmessage = (msg) => {
    if (player) player.feed(msg);
  };
  invoke("calls_media", { channel }).catch(() => {});

  // ---------- the microphone ----------

  let mic = null; // { stream, node, source, sink }
  // On a phone the call plays on the loudspeaker, as a video call does;
  // the person can switch to the earpiece for privacy.
  let loud = true;
  let speakerSet = false; // this call asked for the loudspeaker or earpiece
  function applySpeaker() {
    if (!ON_PHONE) return;
    speakerSet = true;
    invoke("phone_speaker", { on: loud }).catch(() => {});
    setToggle("call-speaker", loud, "speaker", "Speaker", loud ? "On the loudspeaker; tap for the earpiece" : "On the earpiece; tap for the loudspeaker");
  }
  let muted = false;
  let micStarting = false;

  function bytesToBase64(u8) {
    let s = "";
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
    return btoa(s);
  }

  // The microphone, asked again when it is busy (another app letting go
  // of it takes a moment).
  async function openMic() {
    let last = null;
    for (let attempt = 0; attempt < 4; attempt++) {
      try {
        return await navigator.mediaDevices.getUserMedia({
          audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 },
        });
      } catch (e) {
        last = e;
        if (e && e.name === "NotAllowedError") throw e;
        await new Promise((r) => setTimeout(r, 700));
      }
    }
    throw last;
  }

  // Whether the microphone really sends: when its sound stops coming, or
  // comes as exact silence (what Android hands an app while another one,
  // a WhatsApp call say, has the microphone; a real microphone always
  // hears a little), it is opened again, after Kagaz takes the call audio
  // back. Said on the call while it happens.
  let micHeard = 0;
  let micSilentSince = 0;
  let micWatch = null;
  let micRestarts = 0;
  let micRestarting = false;

  function watchMic() {
    clearInterval(micWatch);
    micHeard = Date.now();
    micSilentSince = 0;
    micWatch = setInterval(() => {
      if (!mic || !status || status.phase !== "talking") return;
      const now = Date.now();
      const track = mic.stream.getAudioTracks()[0];
      const why = !track || track.readyState === "ended" ? "it stopped"
        : now - micHeard > 3000 ? "no sound came from it"
        : micSilentSince && now - micSilentSince > 3000 ? "Android gave it only silence"
        : "";
      if (why) restartMic(why);
      else if (micRestarts && !micSilentSince) {
        micRestarts = 0;
        note("");
      }
    }, 1000);
  }

  async function restartMic(why) {
    if (micRestarting) return;
    micRestarting = true;
    micRestarts++;
    invoke("page_log", { line: `call microphone: ${why}; opening it again (${micRestarts})` }).catch(() => {});
    note(micRestarts > 2
      ? "They may not hear you: another app may be using the microphone, or it is switched off in the phone's quick settings. Kagaz keeps trying."
      : "Reconnecting the microphone\u2026");
    releaseMic();
    // Not too often: a microphone that stays silent is tried every few seconds.
    await new Promise((r) => setTimeout(r, micRestarts > 2 ? 4000 : 300));
    micRestarting = false;
    if (status && status.phase === "talking") startAudio();
  }

  function releaseMic() {
    if (!mic) return;
    mic.stream.getTracks().forEach((t) => t.stop());
    try {
      mic.source.disconnect();
      mic.node.disconnect();
      mic.sink.disconnect();
    } catch (e) {}
    mic = null;
  }

  async function startAudio() {
    const actx = KagazPlayer.audioContext();
    if (mic || micStarting) return;
    micStarting = true;
    // The phone's call audio is Kagaz's for the call, also over another
    // app's call (KagazPlugin's CallAudio); the loudspeaker from the moment
    // the call is answered (its Loudspeaker holds it there).
    if (ON_PHONE) invoke("phone_call_audio", { on: true }).catch(() => {});
    applySpeaker();
    try {
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) throw new Error("this window cannot use the microphone");
      const stream = await openMic();
      if (!status || status.phase !== "talking") {
        stream.getTracks().forEach((t) => t.stop());
        return;
      }
      await actx.audioWorklet.addModule("call-mic.js");
      await actx.resume().catch(() => {});
      const source = actx.createMediaStreamSource(stream);
      const node = new AudioWorkletNode(actx, "kagaz-mic");
      // Worklets only run while connected to the output; this one is silent.
      const sink = actx.createGain();
      sink.gain.value = 0;
      node.port.onmessage = (e) => {
        const pcm = e.data;
        micHeard = Date.now();
        if (pcm.some((v) => v !== 0)) micSilentSince = 0;
        else if (!micSilentSince) micSilentSince = micHeard;
        if (muted) return;
        invoke("calls_audio", { data: bytesToBase64(new Uint8Array(pcm.buffer)) }).catch(() => {});
      };
      source.connect(node);
      node.connect(sink);
      sink.connect(actx.destination);
      mic = { stream, node, source, sink };
      watchMic();
      // The microphone put the phone in call mode, which picks the earpiece.
      applySpeaker();
      if (!micRestarts) note("");
    } catch (e) {
      if (e && e.name === "NotAllowedError") {
        const where = /Android/i.test(navigator.userAgent)
          ? "Settings › Apps › Kagaz › Permissions"
          : "the system's privacy settings";
        note(`Kagaz may not use the microphone, so they cannot hear you. Allow it in ${where}, then call again.`);
      } else {
        note("The microphone is not available, so they cannot hear you: " + (e.message || e));
      }
    } finally {
      micStarting = false;
    }
  }

  function stopAudio() {
    clearInterval(micWatch);
    micWatch = null;
    micRestarts = 0;
    releaseMic();
    if (ON_PHONE) invoke("phone_call_audio", { on: false }).catch(() => {});
    // The phone's own choice of where sound plays again.
    if (speakerSet) invoke("phone_speaker", { on: false }).catch(() => {});
    speakerSet = false;
    loud = true;
    muted = false;
    setToggle("call-mute", false, "mic", "Mute", "Mute your microphone");
  }

  // ---------- our camera ----------

  let cam = null; // KagazCallCamera handle while it films
  let camStarting = false;
  let camDecided = false; // this call's camera was set on or off at its start
  let camAsked = false; // this call offered to turn the camera on

  // A person's own phone, computer or indoor monitor (video both ways, as
  // a video call), not a device the building shares: a door station or
  // the guard's phone (the camera stays off until turned on).
  function personal(s) {
    const peer = ((s && s.peer) || "").trim().toLowerCase();
    if (typeof otPeople !== "undefined" && otPeople.some((p) => p.name.trim().toLowerCase() === peer)) return true;
    const dev = ((s && s.devices) || []).find((d) => d.name.trim().toLowerCase() === peer);
    return !(dev && (dev.kind === "door" || dev.kind === "guard"));
  }

  function renderCamera() {
    const on = !!cam || camStarting;
    setToggle("call-camera", on, on ? "camera" : "cameraOff", "Camera", on ? "Your camera is on; tap to turn it off" : "Your camera is off; tap to turn it on");
    $("call-self").classList.toggle("hidden", !cam);
  }

  async function startCamera() {
    if (cam || camStarting) return;
    camStarting = true;
    $("call-ask").classList.add("hidden");
    renderCamera();
    try {
      const handle = await KagazCallCamera.start($("call-self"), {
        onError: (e) => {
          cam = null;
          renderCamera();
          note("The camera stopped: " + (e.message || e));
        },
      });
      if (!status || status.phase !== "talking" || !status.send_video) handle.stop();
      else cam = handle;
    } catch (e) {
      if (e && e.name === "NotAllowedError") {
        const where = /Android/i.test(navigator.userAgent) ? "Settings › Apps › Kagaz › Permissions" : "the system's privacy settings";
        note(`Kagaz may not use the camera, so they cannot see you. Allow it in ${where}, then try again.`);
      } else if (e && (e.name === "NotFoundError" || e.name === "OverconstrainedError")) {
        note("This device has no camera they can see you with.");
      } else {
        note("The camera did not start, so they cannot see you: " + ((e && e.message) || e));
      }
    } finally {
      camStarting = false;
      renderCamera();
    }
  }

  function stopCamera() {
    if (cam) cam.stop();
    cam = null;
    renderCamera();
  }

  listen("call-keyframe", () => {
    if (cam) cam.keyframe();
  });
  $("call-camera").addEventListener("click", () => (cam || camStarting ? stopCamera() : startCamera()));
  $("call-ask-yes").addEventListener("click", () => startCamera());
  $("call-ask-no").addEventListener("click", () => $("call-ask").classList.add("hidden"));

  // ---------- buttons ----------

  $("calls-enabled").addEventListener("change", async (e) => {
    const on = e.target.checked;
    try {
      await invoke("calls_enable", { enabled: on });
      $("calls-note").textContent = "";
    } catch (err) {
      e.target.checked = !on;
      $("calls-note").textContent = `Could not turn ringing ${on ? "on" : "off"}: ${err}`;
    }
  });
  $("calls-retry").addEventListener("click", () => invoke("calls_restart"));
  $("call-answer").addEventListener("click", () => {
    KagazPlayer.audioContext(); // a click is what lets the page make sound
    invoke("calls_answer").catch((e) => note(String(e)));
  });
  $("call-decline").addEventListener("click", () => invoke("calls_decline").catch((e) => note(String(e))));
  $("call-hangup").addEventListener("click", () => invoke("calls_hangup").catch((e) => note(String(e))));
  $("call-open").addEventListener("click", async () => {
    try {
      await invoke("calls_press", { key: (status && status.door_dtmf) || "#" });
      note("Door open sent.");
    } catch (e) {
      note(String(e));
    }
  });
  $("calls-dial").addEventListener("submit", (e) => {
    e.preventDefault();
    const number = $("calls-number").value.replace(/\s+/g, "");
    if (!/^[0-9A-Za-z*#+]+$/.test(number)) {
      $("calls-dial-note").textContent = "Enter the number to call: digits, * or #.";
      $("calls-number").focus();
      return;
    }
    call(number, null);
  });
  $("call-volume").addEventListener("input", (e) => {
    userVolume = Number(e.target.value) || 1;
    if (player) player.volume = userVolume;
    try {
      localStorage.setItem("kagaz-call-volume", String(userVolume));
    } catch (err) {}
  });
  $("call-speaker").addEventListener("click", () => {
    loud = !loud;
    applySpeaker();
  });
  $("call-mute").addEventListener("click", () => {
    muted = !muted;
    setToggle("call-mute", muted, muted ? "micOff" : "mic", muted ? "Unmute" : "Mute", muted ? "Your microphone is off; tap to turn it on" : "Mute your microphone");
  });

  listen("call-event", (e) => render(e.payload));
  listen("call-video", (e) => {
    videoStats = e.payload;
  });
  invoke("calls_status").then(render).catch(() => {});
})();
