// Doorbell calls through the OneTouch cloud: the account's app line rings
// here. A ring brings up the call card at the top of the Doorbell pane; a
// call shows the caller's camera, plays their voice (levelled) and sends
// the microphone. The devices to call are the Doorbell pane's cards
// (onetouch.js), which call through KagazCalls.call; a call made from a
// card plays inside that card. The picture and voice go through the
// window's player (player.js). Uses app.js's globals: $, invoke, listen,
// selectCategory, startChime, stopChime.

(() => {
  const PANEL = `
      <div id="calls-panel" class="calls-panel">
        <div class="calls-head">
          <p id="calls-line" class="muted small"></p>
          <label class="switch" title="Ring here when someone calls the OneTouch account">
            <input id="calls-enabled" type="checkbox"> <span>Ring me here</span>
          </label>
        </div>
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
        </div>
        <div class="call-body">
          <div class="call-who">
            <span id="call-peer"></span>
            <span id="call-state" class="muted"></span>
          </div>
          <div class="call-actions">
            <button id="call-answer" type="button" class="big ok">Answer</button>
            <button id="call-decline" type="button" class="big">Decline</button>
            <button id="call-open" type="button" class="big accent">Open door</button>
            <button id="call-mute" type="button" class="big">Mute</button>
            <button id="call-hangup" type="button" class="big danger">Hang up</button>
          </div>
          <label class="call-volume" title="How loud their voice plays here">Volume
            <input id="call-volume" type="range" min="0.5" max="3" step="0.1" value="1">
          </label>
          <p id="call-note" class="muted small"></p>
        </div>
      </div>`;
  const DIAL = `
      <details id="ot-dial-box" class="calls-more">
        <summary>Call another number</summary>
        <form id="calls-dial" class="calls-dial">
          <input id="calls-number" type="text" inputmode="numeric" autocomplete="off" placeholder="A OneTouch number, e.g. a family member's app">
          <button type="submit">Call</button>
        </form>
      </details>`;

  $("calls-host").innerHTML = PANEL;
  $("call-slot").innerHTML = CARD;
  $("ot-devices").insertAdjacentHTML("afterend", DIAL);

  let status = null;
  let clockTimer = null;
  let ringing = false;

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
  function place() {
    const card = $("call-card");
    const phase = (status && status.phase) || "idle";
    let slot = null;
    if (phase === "calling" || phase === "talking" || card.dataset.ended) {
      const key = callKey || keyForPeer(status && status.peer);
      if (key) slot = document.querySelector(`.dev-card[data-key="${CSS.escape(key)}"] .dev-slot`);
    }
    slot = slot || $("call-slot");
    if (card.parentElement !== slot) slot.appendChild(card);
  }

  function call(number, key) {
    KagazPlayer.audioContext(); // a click is what lets the page make sound
    callKey = key || null;
    // The relay cannot show the live view and a call at once.
    if (typeof otStopLive === "function") otStopLive();
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
    show("call-hangup", phase === "talking" || phase === "calling");
    $("call-clock").classList.toggle("hidden", phase !== "talking");

    if (phase === "ringing" && !ringing) {
      ringing = true;
      if (typeof selectCategory === "function") selectCategory("doorbell");
      // The caller's tone (Settings). On a phone the ring notification
      // plays it, also when the window is closed; the page stays quiet.
      const peer = (s.peer || "").trim().toLowerCase();
      const dev = (s.devices || []).find((d) => d.name.trim().toLowerCase() === peer);
      const family = typeof otPeople !== "undefined" && otPeople.some((p) => p.name.trim().toLowerCase() === peer);
      const kind = (dev && dev.kind === "apps") || family ? "family" : "doorbell";
      if (typeof startChime === "function" && !/Android/i.test(navigator.userAgent)) startChime(s.peer, kind);
      $("call-video-note").textContent = s.video ? "Answer to see the camera" : "";
      card.dataset.ended = "";
    }
    if (phase !== "ringing" && ringing) {
      ringing = false;
      if (typeof stopChime === "function") stopChime();
    }
    if ((phase === "calling" || phase === "talking") && typeof otStopLive === "function") otStopLive();
    if (phase === "talking") {
      startClock(s.since);
      startAudio();
      startPlayer();
      if (!s.video) $("call-video-note").textContent = "No camera on this call";
    } else {
      stopClock();
    }
    if (phase === "idle" && card.dataset.active === "1") {
      // Keep the card a moment to say how it ended.
      card.dataset.active = "";
      card.dataset.ended = "1";
      stopAudio();
      stopPlayer();
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

  function note(text) {
    $("call-note").textContent = text;
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
    player = stage.player;
    $("call-video").classList.add("has-stage");
    // In full screen the card's buttons are out of sight: the two that
    // matter are on the picture too.
    stage.button("Open door", "Open door", () => $("call-open").click(), "kz-word kz-full-only kz-open");
    stage.button("Hang up", "Hang up", () => $("call-hangup").click(), "kz-word kz-full-only kz-danger");
    // What happened to the camera on this call, said on the card when no
    // picture shows: nothing arrived, or it arrived and could not be shown.
    videoWatch = setInterval(() => {
      if (!player || !status || status.phase !== "talking" || !status.video) return;
      $("call-video-note").textContent = videoNote();
    }, 1000);
  }

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
  let muted = false;
  let micStarting = false;

  function bytesToBase64(u8) {
    let s = "";
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
    return btoa(s);
  }

  async function startAudio() {
    const actx = KagazPlayer.audioContext();
    if (mic || micStarting) return;
    micStarting = true;
    try {
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) throw new Error("this window cannot use the microphone");
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 },
      });
      if (!status || status.phase !== "talking") {
        stream.getTracks().forEach((t) => t.stop());
        return;
      }
      await actx.audioWorklet.addModule("call-mic.js");
      const source = actx.createMediaStreamSource(stream);
      const node = new AudioWorkletNode(actx, "kagaz-mic");
      // Worklets only run while connected to the output; this one is silent.
      const sink = actx.createGain();
      sink.gain.value = 0;
      node.port.onmessage = (e) => {
        if (muted) return;
        const pcm = e.data;
        invoke("calls_audio", { data: bytesToBase64(new Uint8Array(pcm.buffer)) }).catch(() => {});
      };
      source.connect(node);
      node.connect(sink);
      sink.connect(actx.destination);
      mic = { stream, node, source, sink };
      note("");
    } catch (e) {
      note("The microphone is not available, so they cannot hear you: " + (e.message || e));
    } finally {
      micStarting = false;
    }
  }

  function stopAudio() {
    if (mic) {
      mic.stream.getTracks().forEach((t) => t.stop());
      try {
        mic.source.disconnect();
        mic.node.disconnect();
        mic.sink.disconnect();
      } catch (e) {}
      mic = null;
    }
    muted = false;
    $("call-mute").textContent = "Mute";
  }

  // ---------- buttons ----------

  $("calls-enabled").addEventListener("change", (e) => invoke("calls_enable", { enabled: e.target.checked }));
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
      note("Enter the number to call.");
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
  $("call-mute").addEventListener("click", () => {
    muted = !muted;
    $("call-mute").textContent = muted ? "Unmute" : "Mute";
  });

  listen("call-event", (e) => render(e.payload));
  listen("call-video", (e) => {
    videoStats = e.payload;
  });
  invoke("calls_status").then(render).catch(() => {});
})();
