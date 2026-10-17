// Doorbell calls through the OneTouch cloud: the account's app line rings
// here. A ring brings up the call card; a call shows the caller's camera,
// plays their voice and sends the microphone. Each device of the account
// has a Call button. Uses app.js's globals: $, invoke, listen,
// selectCategory, startChime, stopChime.

(() => {
  const PANEL = `
    <div id="calls-panel" class="calls-panel">
      <div class="calls-head">
        <div>
          <h3>Calls</h3>
          <p id="calls-line" class="muted small"></p>
        </div>
        <label class="switch" title="Ring here when someone calls the OneTouch account">
          <input id="calls-enabled" type="checkbox"> <span>Ring me here</span>
        </label>
      </div>
      <div id="calls-retry-row" class="actions hidden">
        <button id="calls-retry" type="button">Try again</button>
      </div>
      <div id="call-card" class="hidden">
        <div class="call-video">
          <canvas id="call-canvas" class="hidden"></canvas>
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
          <p id="call-note" class="muted small"></p>
        </div>
      </div>
      <ul id="calls-devices" class="calls-devices"></ul>
    </div>`;

  const head = document.querySelector("#doorbell .cam-head");
  head.insertAdjacentHTML("afterend", PANEL);

  let status = null;
  let clockTimer = null;
  let ringing = false;

  // ---------- status ----------

  const LINE_TEXT = {
    off: "Off. Turn on to ring here when someone calls your OneTouch account.",
    connecting: "Connecting to OneTouch…",
    up: "Ringing here for OneTouch calls.",
    down: "Not connected",
    signed_out: "Sign in to OneTouch below to take its calls here.",
    expired: "The OneTouch sign-in has expired.",
  };

  function render(s) {
    status = s;
    $("calls-enabled").checked = !!s.enabled;
    const base = LINE_TEXT[s.line] || s.line || "";
    $("calls-line").textContent = s.detail && s.line !== "up" ? `${base} ${s.detail}`.trim() : base;
    $("calls-line").classList.toggle("cap-ok", s.line === "up");
    $("calls-line").classList.toggle("cap-warn", ["down", "expired", "signed_out"].includes(s.line));
    $("calls-retry-row").classList.toggle("hidden", !["down", "expired", "signed_out"].includes(s.line) || !s.enabled);
    renderDevices(s);
    renderCard(s);
  }

  function renderDevices(s) {
    const ul = $("calls-devices");
    ul.innerHTML = "";
    const busy = s.phase && s.phase !== "idle";
    for (const d of s.devices || []) {
      const li = document.createElement("li");
      li.innerHTML = `<span class="name"></span><span class="muted small"></span><button type="button">Call</button>`;
      li.querySelector(".name").textContent = d.name;
      const kind = { door: "door station", indoor: "indoor monitor", guard: "guard phone" }[d.kind] || "device";
      li.querySelector(".small").textContent = `${kind}${d.online ? "" : ", offline"}`;
      const b = li.querySelector("button");
      b.disabled = busy || s.line !== "up";
      b.addEventListener("click", () => {
        audioContext(); // a click is what lets the page make sound
        invoke("calls_call", { number: d.sip }).catch((e) => note(String(e)));
      });
      ul.appendChild(li);
    }
  }

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
      if (typeof startChime === "function") startChime();
      $("call-video-note").textContent = s.video ? "Answer to see the camera" : "";
      card.dataset.ended = "";
    }
    if (phase !== "ringing" && ringing) {
      ringing = false;
      if (typeof stopChime === "function") stopChime();
    }
    if (phase === "talking") {
      startClock(s.since);
      startAudio();
      if (!s.video) $("call-video-note").textContent = "No camera on this call";
    } else {
      stopClock();
    }
    if (phase === "idle" && card.dataset.active === "1") {
      // Keep the card a moment to say how it ended.
      card.dataset.active = "";
      card.dataset.ended = "1";
      stopAudio();
      stopVideo();
      $("call-state").textContent = s.detail || "call ended";
      setTimeout(() => {
        card.dataset.ended = "";
        if (status && status.phase === "idle") card.classList.add("hidden");
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

  // ---------- voice ----------

  let actx = null;
  let playAt = 0;
  let mic = null; // { stream, node, source, sink }
  let muted = false;
  let micStarting = false;

  function audioContext() {
    if (!actx) actx = new (window.AudioContext || window.webkitAudioContext)();
    if (actx.state === "suspended") actx.resume().catch(() => {});
    return actx;
  }

  function playVoice(bytes) {
    if (!actx) return;
    const n = bytes.byteLength >> 1;
    if (!n) return;
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const buf = actx.createBuffer(1, n, 8000);
    const ch = buf.getChannelData(0);
    for (let i = 0; i < n; i++) ch[i] = view.getInt16(i * 2, true) / 32768;
    const src = actx.createBufferSource();
    src.buffer = buf;
    src.connect(actx.destination);
    const now = actx.currentTime;
    // A small cushion against network jitter; catch up when it grows.
    if (playAt < now + 0.02 || playAt > now + 0.4) playAt = now + 0.08;
    src.start(playAt);
    playAt += n / 8000;
  }

  function bytesToBase64(u8) {
    let s = "";
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
    return btoa(s);
  }

  async function startAudio() {
    audioContext();
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
    playAt = 0;
  }

  // ---------- camera ----------

  let decoder = null;
  let canvasCtx = null;

  function nalOf(data, kind) {
    for (let i = 0; i + 4 < data.length; i++) {
      if (data[i] === 0 && data[i + 1] === 0 && data[i + 2] === 0 && data[i + 3] === 1 && (data[i + 4] & 0x1f) === kind) {
        return data.subarray(i + 4, i + 8);
      }
    }
    return null;
  }

  function hex2(n) {
    return n.toString(16).padStart(2, "0").toUpperCase();
  }

  function showVideo(key, ts, data) {
    if (typeof VideoDecoder === "undefined") {
      $("call-video-note").textContent = "This window cannot show the camera picture";
      return;
    }
    if (!decoder || decoder.state === "closed") {
      if (!key) return;
      const sps = nalOf(data, 7);
      if (!sps) return;
      const codec = `avc1.${hex2(sps[1])}${hex2(sps[2])}${hex2(sps[3])}`;
      const canvas = $("call-canvas");
      canvasCtx = canvas.getContext("2d");
      decoder = new VideoDecoder({
        output: (frame) => {
          if (canvas.width !== frame.displayWidth) canvas.width = frame.displayWidth;
          if (canvas.height !== frame.displayHeight) canvas.height = frame.displayHeight;
          canvasCtx.drawImage(frame, 0, 0);
          frame.close();
          canvas.classList.remove("hidden");
          $("call-video-note").textContent = "";
        },
        error: () => {
          decoder = null;
        },
      });
      try {
        decoder.configure({ codec, optimizeForLatency: true });
      } catch (e) {
        decoder = null;
        $("call-video-note").textContent = "This window cannot decode the camera picture";
        return;
      }
    }
    if (decoder.decodeQueueSize > 10 && !key) return; // fall behind gracefully
    try {
      decoder.decode(new EncodedVideoChunk({ type: key ? "key" : "delta", timestamp: Math.round((ts * 1000) / 90), data }));
    } catch (e) {
      decoder = null;
    }
  }

  function stopVideo() {
    if (decoder && decoder.state !== "closed") {
      try {
        decoder.close();
      } catch (e) {}
    }
    decoder = null;
    $("call-canvas").classList.add("hidden");
    $("call-video-note").textContent = "";
  }

  // ---------- media channel ----------

  const channel = new window.__TAURI__.core.Channel();
  channel.onmessage = (msg) => {
    const u8 = msg instanceof ArrayBuffer ? new Uint8Array(msg) : msg instanceof Uint8Array ? msg : Array.isArray(msg) ? new Uint8Array(msg) : null;
    if (!u8 || u8.length < 5) return;
    const kind = u8[0];
    const ts = ((u8[1] << 24) >>> 0) + (u8[2] << 16) + (u8[3] << 8) + u8[4];
    const body = u8.subarray(5);
    if (kind === 1) playVoice(body);
    else showVideo(kind === 2, ts, body);
  };
  invoke("calls_media", { channel }).catch(() => {});

  // ---------- buttons ----------

  $("calls-enabled").addEventListener("change", (e) => invoke("calls_enable", { enabled: e.target.checked }));
  $("calls-retry").addEventListener("click", () => invoke("calls_restart"));
  $("call-answer").addEventListener("click", () => {
    audioContext(); // a click is what lets the page make sound
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
  $("call-mute").addEventListener("click", () => {
    muted = !muted;
    $("call-mute").textContent = muted ? "Unmute" : "Mute";
  });

  listen("call-event", (e) => render(e.payload));
  invoke("calls_status").then(render).catch(() => {});
})();
