// The camera on a video call: the page films with the device's camera,
// encodes H.264 (WebCodecs, baseline, Annex-B) and hands each picture to
// the line (calls_video), which sends it to the other side. While it is
// off the line sends a black picture each second instead (phone.rs), so
// the other side still sees video coming.
//
//   const cam = KagazCallCamera.start(previewVideo, { onError });  // a promise
//   cam.keyframe();  // the other side asked for a full picture
//   cam.stop();
//   KagazCallCamera.supported()  // a promise: can this window encode H.264?

const KagazCallCamera = (() => {
  const FPS = 15;
  const KEY_EVERY = FPS * 2; // a full picture every two seconds
  // Baseline profile (what door stations and phones take), from level 3.1
  // down: encoders differ in what they accept.
  const CODECS = ["avc1.42E01F", "avc1.42001F", "avc1.42E01E", "avc1.42C01F"];

  function config(codec, width, height) {
    return {
      codec,
      width,
      height,
      bitrate: 600_000,
      framerate: FPS,
      latencyMode: "realtime",
      avc: { format: "annexb" },
    };
  }

  async function pickCodec(width, height) {
    if (typeof VideoEncoder === "undefined" || typeof VideoFrame === "undefined") return null;
    for (const codec of CODECS) {
      try {
        const r = await VideoEncoder.isConfigSupported(config(codec, width, height));
        if (r && r.supported) return codec;
      } catch (e) {}
    }
    return null;
  }

  const supported = () => pickCodec(640, 480).then((c) => !!c);

  function toBase64(u8) {
    let s = "";
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
    return btoa(s);
  }

  // SPS and PPS out of an avcC record (the encoder's description), as
  // Annex-B: put before a keyframe that came without them.
  function parameterSets(avcc) {
    const b = new Uint8Array(avcc);
    const out = [];
    let i = 5;
    const take = (n) => {
      for (let k = 0; k < n && i + 2 <= b.length; k++) {
        const len = (b[i] << 8) | b[i + 1];
        out.push(0, 0, 0, 1, ...b.subarray(i + 2, i + 2 + len));
        i += 2 + len;
      }
    };
    take(b[i++] & 0x1f);
    take(b[i++] || 0);
    return new Uint8Array(out);
  }

  const hasNal = (u8, type) => {
    for (let i = 0; i + 3 < u8.length; i++) {
      if (u8[i] === 0 && u8[i + 1] === 0 && u8[i + 2] === 1 && (u8[i + 3] & 0x1f) === type) return true;
    }
    return false;
  };

  async function start(preview, { onError } = {}) {
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) throw new Error("this window cannot use the camera");
    const stream = await navigator.mediaDevices.getUserMedia({
      video: { width: { ideal: 640 }, height: { ideal: 480 }, frameRate: { ideal: FPS }, facingMode: "user" },
      audio: false,
    });
    preview.srcObject = stream;
    preview.muted = true;
    preview.playsInline = true;
    await preview.play().catch(() => {});
    // Wait for the camera's size.
    for (let i = 0; i < 50 && !preview.videoWidth; i++) await new Promise((r) => setTimeout(r, 50));
    // The picture keeps the camera's own shape (upright on an upright
    // phone), its longer side at most 640.
    const even = (n) => Math.max(2, Math.round(n / 2) * 2);
    const sizeOf = () => {
      const w = preview.videoWidth || 640, h = preview.videoHeight || 480;
      const scale = Math.min(1, 640 / Math.max(w, h));
      return [even(w * scale), even(h * scale)];
    };
    let [width, height] = sizeOf();
    const codec = await pickCodec(width, height);
    if (!codec) {
      stream.getTracks().forEach((t) => t.stop());
      throw new Error("this window cannot encode video (no H.264 encoder)");
    }
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    let sets = null; // SPS and PPS, from the encoder's description
    let stopped = false;
    let count = 0;
    let wantKey = true;
    const encoder = new VideoEncoder({
      output: (chunk, meta) => {
        if (meta && meta.decoderConfig && meta.decoderConfig.description) sets = parameterSets(meta.decoderConfig.description);
        let data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        if (chunk.type === "key" && sets && !hasNal(data, 7)) {
          const both = new Uint8Array(sets.length + data.length);
          both.set(sets);
          both.set(data, sets.length);
          data = both;
        }
        invoke("calls_video", { data: toBase64(data) }).catch(() => {});
      },
      error: (e) => {
        if (onError) onError(e);
        stop();
      },
    });
    encoder.configure(config(codec, width, height));
    const started = performance.now();
    const timer = setInterval(() => {
      if (stopped || preview.readyState < 2) return;
      // Drop a picture rather than queue them when the encoder falls behind.
      if (encoder.encodeQueueSize > 2) return;
      // The phone turned (the camera's picture changed shape): the encoder
      // takes the new shape, from a full picture, rather than stretch it.
      const [w, h] = sizeOf();
      if (w !== width || h !== height) {
        width = w;
        height = h;
        canvas.width = width;
        canvas.height = height;
        try {
          encoder.configure(config(codec, width, height));
        } catch (e) {
          if (onError) onError(e);
          stop();
          return;
        }
        wantKey = true;
      }
      ctx.drawImage(preview, 0, 0, width, height);
      const frame = new VideoFrame(canvas, { timestamp: Math.round((performance.now() - started) * 1000) });
      const key = wantKey || count % KEY_EVERY === 0;
      wantKey = false;
      count++;
      try {
        encoder.encode(frame, { keyFrame: key });
      } finally {
        frame.close();
      }
    }, 1000 / FPS);

    function stop() {
      if (stopped) return;
      stopped = true;
      clearInterval(timer);
      try {
        encoder.close();
      } catch (e) {}
      stream.getTracks().forEach((t) => t.stop());
      preview.srcObject = null;
    }

    return {
      stop,
      keyframe: () => {
        wantKey = true;
      },
    };
  }

  return { start, supported };
})();
