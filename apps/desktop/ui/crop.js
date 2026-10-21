// Frames around cards on a scanned page, adjusted by hand, as in Adobe
// Scan: drag a frame's four corners onto the card, and the middle of a
// side to bend it (a passport page that bows or sits skewed). A side's
// middle, until moved, stays halfway between its corners. Corners are
// rounded, as an ID card's are. Everything outside the frames turns white
// when saved. An ID page has two frames, one per half; remove one for a
// one-sided ID, add it back after. The card is never stretched or moved:
// it keeps its place and real size (kagaz_core scan::id, whiten_outside).
//
//   const crop = KagazCrop.attach(box, { src, outlines: [{ quad, found }],
//                                        radius, names: ["front", "back"] });
//   crop.quads()  // the frames kept, each corner, middle, corner, ... (8
//                 // points, fractions of the page)

const KagazCrop = (() => {
  const CORNERS = ["top left", "top right", "bottom right", "bottom left"];
  const SIDES = ["top", "right", "bottom", "left"];

  function attach(box, { src, outlines, radius = 0.015, names }) {
    box.innerHTML = "";
    box.classList.add("crop");
    const stage = document.createElement("div");
    stage.className = "crop-stage";
    const img = document.createElement("img");
    img.alt = "The page, with a frame around each card";
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("aria-hidden", "true");
    stage.append(img, svg);
    const tools = document.createElement("div");
    tools.className = "crop-tools";
    box.append(stage, tools);

    const copy = (q) => q.map((p) => p.slice());
    // Each frame: its corners as found (to add it back), its corners now,
    // and each side's middle (null: halfway between its corners).
    const frames = outlines.map((o, i) => ({
      start: copy(o.quad),
      corners: copy(o.quad),
      mids: [null, null, null, null],
      kept: true,
      name: (names && names[i]) || `card ${i + 1}`,
      handles: [],
      midHandles: [],
      remove: null,
    }));
    const half = (a, b) => [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    const mid = (f, i) => f.mids[i] || half(f.corners[i], f.corners[(i + 1) % 4]);
    // Corner, middle, corner, middle, ... as Rust takes it.
    const shape = (f) => f.corners.flatMap((c, i) => [c.slice(), mid(f, i).slice()]);

    // The outline in the picture's pixels, its corners rounded as Rust
    // rounds them (a curve between the points `r` along each side).
    function path(f, w, h) {
      const pts = shape(f).map(([x, y]) => [x * w, y * h]);
      const r = radius * w;
      let d = "";
      pts.forEach((c, i) => {
        if (i % 2) {
          d += `L${c[0]},${c[1]}`;
          return;
        }
        const toward = (q) => {
          const dx = q[0] - c[0], dy = q[1] - c[1];
          const len = Math.max(Math.hypot(dx, dy), 1e-3);
          const k = Math.min(r, len / 2) / len;
          return [c[0] + dx * k, c[1] + dy * k];
        };
        const a = toward(pts[(i + pts.length - 1) % pts.length]);
        const b = toward(pts[(i + 1) % pts.length]);
        d += `${d ? "L" : "M"}${a[0]},${a[1]}Q${c[0]},${c[1]} ${b[0]},${b[1]}`;
      });
      return d + "Z";
    }

    const clamp = (v) => Math.min(1, Math.max(0, v));
    const place = (el, p) => {
      el.style.left = `${p[0] * 100}%`;
      el.style.top = `${p[1] * 100}%`;
    };
    function draw() {
      const w = img.naturalWidth || 1000;
      const h = img.naturalHeight || 1414;
      svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
      // Dim outside the kept frames (even-odd: the page, less each frame).
      const kept = frames.filter((f) => f.kept);
      svg.innerHTML =
        `<path class="crop-dim" fill-rule="evenodd" d="M0,0H${w}V${h}H0Z ${kept.map((f) => path(f, w, h)).join(" ")}"/>` +
        kept.map((f) => `<path class="crop-line" d="${path(f, w, h)}"/>`).join("");
      for (const f of frames) {
        f.handles.forEach((el, i) => {
          el.hidden = !f.kept;
          place(el, f.corners[i]);
        });
        f.midHandles.forEach((el, i) => {
          el.hidden = !f.kept;
          place(el, mid(f, i));
        });
        f.remove.hidden = !f.kept;
        place(f.remove, f.corners.reduce((s, c) => [s[0] + c[0] / 4, s[1] + c[1] / 4], [0, 0]));
      }
      tools.innerHTML = "";
      for (const f of frames.filter((f) => !f.kept)) {
        const add = document.createElement("button");
        add.type = "button";
        add.textContent = `Add the ${f.name} frame`;
        add.addEventListener("click", () => {
          f.kept = true;
          f.corners = copy(f.start);
          f.mids = [null, null, null, null];
          draw();
        });
        tools.appendChild(add);
      }
    }

    // A handle: dragged with a mouse or a finger, nudged with the arrows.
    function handle(cls, label, get, set) {
      const el = document.createElement("button");
      el.type = "button";
      el.className = cls;
      el.setAttribute("aria-label", label);
      el.addEventListener("pointerdown", (ev) => {
        ev.preventDefault();
        el.setPointerCapture(ev.pointerId);
        const move = (e) => {
          const r = stage.getBoundingClientRect();
          set([clamp((e.clientX - r.left) / r.width), clamp((e.clientY - r.top) / r.height)]);
          draw();
        };
        const up = () => {
          el.removeEventListener("pointermove", move);
          el.removeEventListener("pointerup", up);
          el.removeEventListener("pointercancel", up);
        };
        el.addEventListener("pointermove", move);
        el.addEventListener("pointerup", up);
        el.addEventListener("pointercancel", up);
      });
      el.addEventListener("keydown", (e) => {
        const step = e.shiftKey ? 0.02 : 0.004;
        const d = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }[e.key];
        if (!d) return;
        e.preventDefault();
        const p = get();
        set([clamp(p[0] + d[0]), clamp(p[1] + d[1])]);
        draw();
      });
      stage.appendChild(el);
      return el;
    }

    for (const f of frames) {
      f.handles = CORNERS.map((corner, i) =>
        handle("crop-handle", `${corner[0].toUpperCase()}${corner.slice(1)} corner of the ${f.name} frame: drag, or arrow keys`,
          () => f.corners[i], (p) => { f.corners[i] = p; }));
      // A side's middle: free once moved; until then it follows its corners.
      f.midHandles = SIDES.map((side, i) =>
        handle("crop-handle crop-mid", `Middle of the ${side} side of the ${f.name} frame: drag to bend the side, or arrow keys`,
          () => mid(f, i), (p) => { f.mids[i] = p; }));
      f.remove = document.createElement("button");
      f.remove.type = "button";
      f.remove.className = "crop-remove";
      f.remove.textContent = "✕";
      f.remove.title = `Remove the ${f.name} frame (a one-sided ID)`;
      f.remove.setAttribute("aria-label", f.remove.title);
      f.remove.addEventListener("click", () => {
        f.kept = false;
        draw();
      });
      stage.appendChild(f.remove);
    }
    img.addEventListener("load", draw);
    img.src = src;
    draw();

    return {
      quads: () => frames.filter((f) => f.kept).map(shape),
    };
  }

  return { attach };
})();
