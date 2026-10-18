// The window's own player: pictures and sound arrive from Rust as small
// packets over a channel, and the page decodes them itself with the
// browser's built-in H.264 decoder (WebCodecs) and plays the sound through
// WebAudio. The same player serves camera tiles, recordings, the doorbell's
// live view and calls, on every platform, with no helper program.
//
// A packet: one byte of kind (1 sound, 2 keyframe, 3 picture, 4 the end),
// four bytes of timestamp (90 kHz), then 8 kHz 16-bit samples, Annex-B
// video, or a UTF-8 reason.

const KagazPlayer = (() => {
  let shared = null;

  // One audio context for the page; it must be created or resumed from a
  // click, so callers do this in their button handlers.
  function audioContext() {
    if (!shared) shared = new (window.AudioContext || window.webkitAudioContext)();
    if (shared.state === "suspended") shared.resume().catch(() => {});
    return shared;
  }

  const hex2 = (n) => n.toString(16).padStart(2, "0").toUpperCase();

  // One line to whoever listens (app.js sends it to the stream log).
  function report(line) {
    try {
      if (KagazPlayer.report) KagazPlayer.report(line);
    } catch (e) {}
  }

  // The NAL units of an Annex-B picture, without their start codes.
  function nals(d) {
    const out = [];
    let i = 0, s = -1;
    while (i + 3 <= d.length) {
      if (d[i] === 0 && d[i + 1] === 0 && d[i + 2] === 1) {
        if (s >= 0) {
          let e = i;
          if (e > s && d[e - 1] === 0) e--;
          if (e > s) out.push(d.subarray(s, e));
        }
        i += 3;
        s = i;
      } else i++;
    }
    if (s >= 0 && s < d.length) out.push(d.subarray(s));
    return out;
  }

  // Decoders take the MP4 shape (WebKit only that): units with 4-byte
  // lengths, and the parameter sets apart as an avcC description.
  function avcc(units) {
    let n = 0;
    for (const u of units) n += 4 + u.length;
    const o = new Uint8Array(n);
    let p = 0;
    for (const u of units) {
      o[p++] = u.length >>> 24;
      o[p++] = (u.length >> 16) & 255;
      o[p++] = (u.length >> 8) & 255;
      o[p++] = u.length & 255;
      o.set(u, p);
      p += u.length;
    }
    return o;
  }

  function avcC(sps, pps) {
    return new Uint8Array([1, sps[1], sps[2], sps[3], 0xff, 0xe1, sps.length >> 8, sps.length & 255, ...sps, 1, pps.length >> 8, pps.length & 255, ...pps]);
  }

  // Levelled speech, as in a call: an automatic gain lifts quiet voices
  // towards a steady loudness (not the hiss between words), a compressor
  // holds loud bursts down, and the volume sits on top.
  const TARGET_RMS = 0.12;
  const NOISE_RMS = 0.006;
  const MAX_GAIN = 8;

  class Player {
    // root: the element the picture fills. sound: play the audio packets.
    // paced: a recording, shown by its timestamps (live shows at once).
    constructor(root, { sound = false, paced = false, volume = 1, level = true, label = "player" } = {}) {
      this.root = root;
      this.label = label;
      this.paced = paced;
      this.level = level;
      this.box = document.createElement("div");
      this.box.className = "kz-player";
      this.canvas = document.createElement("canvas");
      this.canvas.className = "kz-canvas";
      this.box.appendChild(this.canvas);
      root.appendChild(this.box);
      this.ctx = null;
      this.decoder = null;
      this.codec = "";
      this.sps = null;
      this.pps = null;
      this.params = "";
      this.stats = { received: 0, keys: 0, shown: 0, audio: 0, problem: "" };
      this.onended = null; // (reason) => {}
      this.onframe = null; // () => {}, on every picture shown
      this.onsize = null; // (width, height) => {}, when the picture's size is known or changes
      this.ended = false;
      // What a <video> would say, for code that watches progress.
      this.currentTime = 0;
      this.readyState = 0;
      this.paused = false;
      this.error = null;
      this.firstTs = null;
      this.lastTs = null;
      // Paced playback: decoded frames wait their turn.
      this.queue = [];
      this.clockStart = null; // performance.now() when the first frame was due
      this.raf = null;
      this.pendingChunks = [];
      // Sound.
      this._sound = sound;
      this._volume = volume;
      this.voice = null;
      this.speech = TARGET_RMS;
      this.playAt = 0;
      this.audioFirstTs = null;
      this.audioStart = 0;
    }

    get sound() {
      return this._sound;
    }
    set sound(on) {
      this._sound = !!on;
      if (!on && this.voice) this.voice.out.gain.setTargetAtTime(0, audioContext().currentTime, 0.05);
      if (on && this.voice) this.voice.out.gain.setTargetAtTime(this._volume, audioContext().currentTime, 0.05);
    }
    get volume() {
      return this._volume;
    }
    set volume(v) {
      this._volume = Math.max(0, Math.min(4, Number(v) || 0));
      if (this.voice && this._sound) this.voice.out.gain.setTargetAtTime(this._volume, audioContext().currentTime, 0.05);
    }

    // One packet from the channel.
    feed(msg) {
      if (this.ended) return;
      const u8 = msg instanceof ArrayBuffer ? new Uint8Array(msg) : msg instanceof Uint8Array ? msg : Array.isArray(msg) ? new Uint8Array(msg) : null;
      if (!u8 || u8.length < 5) return;
      const kind = u8[0];
      const ts = ((u8[1] << 24) >>> 0) + (u8[2] << 16) + (u8[3] << 8) + u8[4];
      const body = u8.subarray(5);
      if (kind === 1) this.audio(body, ts);
      else if (kind === 2 || kind === 3) this.video(kind === 2, ts, body);
      else if (kind === 4) this.end(new TextDecoder().decode(body));
    }

    end(reason) {
      if (this.ended) return;
      this.ended = true;
      report(`${this.label}: ended${reason ? ": " + reason : ""} after ${this.stats.shown} pictures`);
      if (this.onended) this.onended(reason || "");
    }

    // ---------- pictures ----------

    video(key, ts, data) {
      this.stats.received++;
      if (key) this.stats.keys++;
      if (typeof VideoDecoder === "undefined") {
        this.problem("this window has no video decoder (WebCodecs)");
        return;
      }
      const slices = [];
      let sps = null, pps = null;
      for (const u of nals(data)) {
        const t = u[0] & 0x1f;
        if (t === 7) sps = u;
        else if (t === 8) pps = u;
        else if (t !== 9) slices.push(u);
      }
      if (sps) this.sps = sps;
      if (pps) this.pps = pps;
      const params = this.sps && this.pps ? `${this.sps.join(",")}/${this.pps.join(",")}` : "";
      // A decoder is set up on a keyframe, and again when the stream's
      // parameters change.
      if (!this.decoder || this.decoder.state === "closed" || (key && params !== this.params)) {
        if (!key) return;
        if (!params) {
          this.problem("the first full picture came without its stream settings (SPS, PPS)");
          return;
        }
        if (this.decoder && this.decoder.state !== "closed") {
          try {
            this.decoder.close();
          } catch (e) {}
        }
        this.codec = `avc1.${hex2(this.sps[1])}${hex2(this.sps[2])}${hex2(this.sps[3])}`;
        if (!this.ctx) this.ctx = this.canvas.getContext("2d");
        this.decoder = new VideoDecoder({
          output: (frame) => this.show(frame),
          error: (e) => {
            this.problem(`the decoder failed (${(e && e.message) || e})`);
            this.decoder = null;
          },
        });
        try {
          this.decoder.configure({ codec: this.codec, description: avcC(this.sps, this.pps), optimizeForLatency: !this.paced });
          this.params = params;
        } catch (e) {
          this.decoder = null;
          this.problem(`the decoder refused ${this.codec} (${e.message || e})`);
          return;
        }
      }
      if (!slices.length) return;
      const chunk = new EncodedVideoChunk({ type: key ? "key" : "delta", timestamp: Math.round((ts * 1000) / 90), data: avcc(slices) });
      if (this.paced) {
        // Keep encoded pictures (small) rather than decoded ones (large)
        // when they arrive faster than they are due.
        this.pendingChunks.push(chunk);
        this.pump();
        return;
      }
      if (this.decoder.decodeQueueSize > 10 && !key) return; // fall behind gracefully
      this.decode(chunk);
    }

    decode(chunk) {
      try {
        this.decoder.decode(chunk);
      } catch (e) {
        this.problem(`decoding failed (${e.message || e})`);
        this.decoder = null;
      }
    }

    // Paced: decode a few pictures ahead of the clock, no more.
    pump() {
      while (this.decoder && this.pendingChunks.length && this.decoder.decodeQueueSize + this.queue.length < 6) {
        this.decode(this.pendingChunks.shift());
      }
      if (!this.raf) this.raf = requestAnimationFrame(() => this.tick());
    }

    show(frame) {
      if (!this.paced) {
        this.draw(frame);
        return;
      }
      this.queue.push(frame);
      if (!this.raf) this.raf = requestAnimationFrame(() => this.tick());
    }

    // Paced: draw whatever is due by the clock, which starts on the first
    // picture and runs at real time (the timestamps carry any speed).
    tick() {
      this.raf = null;
      if (this.ended) {
        this.dropQueue();
        return;
      }
      const now = performance.now();
      if (this.queue.length) {
        if (this.firstTs === null) {
          this.firstTs = this.queue[0].timestamp;
          this.clockStart = now;
        }
        if (this.paused) {
          // Hold: the clock moves with us.
          this.clockStart = now - this.heldAt;
        } else {
          const due = (frame) => this.clockStart + (frame.timestamp - this.firstTs) / 1000;
          // Several due at once (after a stall): show the last, drop the rest.
          let pick = null;
          while (this.queue.length && due(this.queue[0]) <= now) {
            if (pick) pick.close();
            pick = this.queue.shift();
          }
          if (pick) {
            this.draw(pick);
            this.heldAt = now - this.clockStart;
          }
        }
      }
      this.pump();
      if (this.queue.length || this.pendingChunks.length || !this.ended) this.raf = requestAnimationFrame(() => this.tick());
    }

    draw(frame) {
      const c = this.canvas;
      if (c.width !== frame.displayWidth || c.height !== frame.displayHeight) {
        c.width = frame.displayWidth;
        c.height = frame.displayHeight;
        // Wide pictures and tall ones are laid out differently (see Stage).
        this.box.dataset.shape = shapeOf(c.width, c.height);
        if (this.onsize) this.onsize(c.width, c.height);
      }
      this.ctx.drawImage(frame, 0, 0);
      const ts = frame.timestamp / 1e6;
      frame.close();
      this.stats.shown++;
      if (this.stats.shown === 1) report(`${this.label}: first picture ${c.width}x${c.height} (${this.codec})`);
      this.readyState = 2;
      if (this.firstTs === null) this.firstTs = frame.timestamp;
      this.lastTs = frame.timestamp;
      this.currentTime = Math.max(0, (frame.timestamp - this.firstTs) / 1e6);
      void ts;
      if (this.onframe) this.onframe();
    }

    dropQueue() {
      for (const f of this.queue) f.close();
      this.queue = [];
      this.pendingChunks = [];
    }

    // A paced player can be held (to wait for other cameras) and let go.
    pause() {
      if (!this.paced || this.paused) return;
      this.paused = true;
      this.heldAt = this.clockStart === null ? 0 : performance.now() - this.clockStart;
    }
    play() {
      if (!this.paced || !this.paused) return Promise.resolve();
      this.paused = false;
      this.clockStart = performance.now() - (this.heldAt || 0);
      if (!this.raf) this.raf = requestAnimationFrame(() => this.tick());
      return Promise.resolve();
    }

    problem(text) {
      if (this.stats.problem !== text) report(`${this.label}: ${text}`);
      this.stats.problem = text;
    }

    // What to say over a blank picture, from what has (not) arrived.
    note() {
      const s = this.stats;
      if (s.shown > 0) return "";
      if (!s.received) return "waiting for the first picture…";
      if (s.problem) return `Pictures arrive (${s.received}) but cannot be shown: ${s.problem}`;
      return `Pictures arrive (${s.received}, ${s.keys} full) but none has been shown yet`;
    }

    // ---------- sound ----------

    voiceChain() {
      if (this.voice) return this.voice;
      const actx = audioContext();
      const level = actx.createGain();
      const comp = actx.createDynamicsCompressor();
      comp.threshold.value = -20;
      comp.knee.value = 10;
      comp.ratio.value = 8;
      comp.attack.value = 0.003;
      comp.release.value = 0.25;
      const out = actx.createGain();
      out.gain.value = this._sound ? this._volume : 0;
      level.connect(comp);
      comp.connect(out);
      out.connect(actx.destination);
      this.voice = { level, out };
      return this.voice;
    }

    levelVoice(samples) {
      if (!this.level) return;
      let sum = 0;
      for (let i = 0; i < samples.length; i++) sum += samples[i] * samples[i];
      const rms = Math.sqrt(sum / samples.length);
      if (rms < NOISE_RMS) return; // keep the gain; don't chase the hiss
      this.speech = this.speech * 0.9 + rms * 0.1;
      const gain = Math.max(0.5, Math.min(MAX_GAIN, TARGET_RMS / this.speech));
      this.voiceChain().level.gain.setTargetAtTime(gain, audioContext().currentTime, 0.4);
    }

    audio(bytes, ts) {
      this.stats.audio++;
      if (!this._sound || !shared) return; // nothing to hear, or no click yet
      const n = bytes.byteLength >> 1;
      if (!n) return;
      const actx = audioContext();
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      const buf = actx.createBuffer(1, n, 8000);
      const ch = buf.getChannelData(0);
      for (let i = 0; i < n; i++) ch[i] = view.getInt16(i * 2, true) / 32768;
      this.levelVoice(ch);
      const src = actx.createBufferSource();
      src.buffer = buf;
      src.connect(this.voiceChain().level);
      const now = actx.currentTime;
      let at;
      if (this.paced && this.firstTs !== null) {
        // Line the sound up with the pictures' clock.
        if (this.audioFirstTs === null) {
          this.audioFirstTs = ts;
          this.audioStart = now + Math.max(0, (ts * 1000) / 90 - this.firstTs) / 1e6 - (performance.now() - this.clockStart) / 1000;
        }
        at = this.audioStart + (ts - this.audioFirstTs) / 90000;
        if (at < now) return; // already past
      } else {
        // Live: a small cushion against network jitter; catch up when it grows.
        if (this.playAt < now + 0.02 || this.playAt > now + 0.4) this.playAt = now + 0.08;
        at = this.playAt;
        this.playAt += n / 8000;
      }
      src.start(at);
    }

    // ---------- end ----------

    // Stop decoding and sound but keep the last picture on show.
    freeze() {
      this.ended = true;
      if (this.raf) cancelAnimationFrame(this.raf);
      this.raf = null;
      this.dropQueue();
      if (this.decoder && this.decoder.state !== "closed") {
        try {
          this.decoder.close();
        } catch (e) {}
      }
      this.decoder = null;
      if (this.voice) {
        try {
          this.voice.out.disconnect();
        } catch (e) {}
        this.voice = null;
      }
    }

    stop() {
      this.freeze();
      this.box.remove();
    }
  }

  // "wide" (landscape, as most cameras) or "tall" (portrait, as a phone).
  function shapeOf(w, h) {
    return h > w * 1.05 ? "tall" : "wide";
  }

  // Full screen for an element holding a picture. The page fills the
  // window (a fixed layer) and, where the system allows, the screen too.
  // On an upright phone a wide picture is turned on its side, as video
  // apps do, so it uses the whole screen without the phone being rotated.
  function fullscreen(el, on) {
    const was = el.classList.contains("kz-full");
    if (on === undefined) on = !was;
    if (on === was) return on;
    el.classList.toggle("kz-full", on);
    document.body.classList.toggle("kz-has-full", on);
    // Back leaves full screen first.
    if (typeof KagazNav !== "undefined") {
      if (on) KagazNav.push("full", () => fullscreen(el, false));
      else KagazNav.pop("full");
    }
    try {
      if (on && document.documentElement.requestFullscreen && !document.fullscreenElement) {
        document.documentElement.requestFullscreen().catch(() => {});
      } else if (!on && document.fullscreenElement && document.exitFullscreen) {
        document.exitFullscreen().catch(() => {});
      }
    } catch (e) {}
    if (el._onfull) el._onfull(on);
    return on;
  }
  document.addEventListener("keydown", (ev) => {
    if (ev.key !== "Escape") return;
    const full = document.querySelector(".kz-full");
    if (full) {
      ev.stopImmediatePropagation();
      fullscreen(full, false);
    }
  }, true);
  // Leaving the system's full screen (Esc, a gesture) leaves ours too.
  document.addEventListener("fullscreenchange", () => {
    if (document.fullscreenElement) return;
    document.querySelectorAll(".kz-full").forEach((el) => fullscreen(el, false));
  });

  const ICON = {
    full: '<svg viewBox="0 0 24 24"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/></svg>',
    unfull: '<svg viewBox="0 0 24 24"><path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5"/></svg>',
    close: '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>',
  };

  // A stage: one picture with its controls over it, laid out by the
  // picture's shape, as the big video apps do.
  // - Wide (16:9 and so on), as YouTube: the full width of its place
  //   (edge to edge on a phone), a title along the top and a bar of
  //   buttons along the bottom over a soft shade.
  // - Tall (9:16), as TikTok and Reels: as tall as the screen allows,
  //   centred, the buttons in a column down the right edge.
  // The controls show while nothing plays, on hover with a mouse, and for
  // a few seconds after a tap with a finger; a tap on the picture itself
  // shows or hides them. Full screen turns a wide picture on its side on
  // an upright phone.
  class Stage {
    constructor(parent, { title = "", badge = "", ...options } = {}) {
      this.el = document.createElement("div");
      this.el.className = "kz-stage idle";
      this.el.dataset.shape = "wide";
      parent.appendChild(this.el);
      this.player = new Player(this.el, options);
      this.note = document.createElement("div");
      this.note.className = "kz-note";
      this.top = document.createElement("div");
      this.top.className = "kz-top";
      const name = document.createElement("span");
      name.className = "kz-title";
      name.textContent = title;
      this.badge = document.createElement("span");
      this.badge.className = "kz-badge";
      this.badge.textContent = badge;
      this.badge.classList.toggle("hidden", !badge);
      this.top.append(this.badge, name);
      this.bar = document.createElement("div");
      this.bar.className = "kz-bar";
      this.tools = document.createElement("div");
      this.tools.className = "kz-tools";
      this.bar.appendChild(this.tools);
      this.fullButton = this.button(ICON.full, "Full screen", () => this.fullscreen(), "kz-full-button");
      this.el.append(this.note, this.top, this.bar);
      this.player.onsize = (w, h) => {
        this.el.style.setProperty("--ar", `${w} / ${h}`);
        this.el.dataset.shape = shapeOf(w, h);
      };
      this.player.onframe = () => {
        if (this.el.classList.contains("idle")) {
          this.el.classList.remove("idle");
          this.peek();
        }
        this.setNote("");
      };
      this.el._onfull = (on) => {
        this.fullButton.innerHTML = on ? ICON.unfull : ICON.full;
        this.fullButton.title = on ? "Leave full screen (Esc)" : "Full screen";
        this.peek();
      };
      this.hideTimer = null;
      this.el.addEventListener("click", (ev) => {
        if (ev.target.closest("button, input, a")) {
          this.peek();
          return;
        }
        if (this.el.classList.contains("show")) this.hide();
        else this.peek();
      });
    }

    // A round button over the picture; `html` is an icon or a short word.
    button(html, title, onClick, extra = "") {
      const b = document.createElement("button");
      b.type = "button";
      b.className = `kz-button ${extra}`.trim();
      b.title = title;
      b.setAttribute("aria-label", title);
      b.innerHTML = html;
      b.addEventListener("click", (ev) => {
        ev.stopPropagation();
        onClick(ev);
        this.peek();
      });
      if (this.fullButton) this.bar.insertBefore(b, this.fullButton);
      else this.bar.appendChild(b);
      return b;
    }

    // Show the controls for a while (until hidden on a touch screen).
    peek() {
      this.el.classList.add("show");
      clearTimeout(this.hideTimer);
      this.hideTimer = setTimeout(() => this.hide(), 3500);
    }
    hide() {
      clearTimeout(this.hideTimer);
      if (!this.el.classList.contains("idle")) this.el.classList.remove("show");
    }

    setTitle(text) {
      this.top.querySelector(".kz-title").textContent = text;
    }
    setBadge(text) {
      this.badge.textContent = text;
      this.badge.classList.toggle("hidden", !text);
    }
    // Text over the picture: what is happening while none shows.
    setNote(text) {
      this.note.textContent = text || "";
      this.note.classList.toggle("hidden", !text);
    }
    // Nothing is playing (any more): the controls stay up.
    setIdle(idle) {
      this.el.classList.toggle("idle", idle);
      if (idle) this.el.classList.add("show");
    }

    fullscreen(on) {
      return fullscreen(this.el, on);
    }

    stop() {
      clearTimeout(this.hideTimer);
      fullscreen(this.el, false);
      this.player.stop();
      this.el.remove();
    }
  }

  // Start watching: `command` is a Rust command that takes `channel` and
  // returns the watch id (or an object with `id`); the handle's stop()
  // ends it on both sides.
  async function watch(player, command, args = {}) {
    const channel = new window.__TAURI__.core.Channel();
    channel.onmessage = (m) => player.feed(m);
    const result = await window.__TAURI__.core.invoke(command, { ...args, channel });
    const id = typeof result === "object" && result !== null ? result.id : result;
    let stopped = false;
    return {
      result,
      id,
      stop() {
        if (stopped) return;
        stopped = true;
        player.stop();
        window.__TAURI__.core.invoke("media_unwatch", { id }).catch(() => {});
      },
    };
  }

  return { Player, Stage, watch, audioContext, fullscreen, shapeOf, ICON };
})();
