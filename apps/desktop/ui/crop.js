// Frames around cards on a scanned page, adjusted by hand, as in Adobe
// Scan: drag a frame's four corners onto the card, and the middle of a
// side to bend it (a passport page that bows or sits skewed). A side's
// middle, until moved, stays halfway between its corners. Corners are
// rounded, as an ID card's are. Everything outside the frames turns white
// when saved. An ID page has two frames, one per half; remove one for a
// one-sided ID, add it back after. The card is never stretched or moved:
// it keeps its place and real size (kagaz_core scan::id, whiten_outside).
//
// A frame can also have a fixed shape (Crop's passport photo, 16:9, ...):
// then it stays a true rectangle of that shape; its corners resize it, and
// a handle above it turns it, for a photo lying crooked. Drag inside any
// frame to move it.
//
//   const crop = KagazCrop.attach(box, { src, outlines: [{ quad, found }],
//                                        radius, names: ["front", "back"],
//                                        ratio,       // width / height, or "lock"
//                                        removable }); // false: no ✕
//   crop.quads()       // the frames kept, each corner, middle, corner, ...
//                      // (8 points, fractions of the page); 4 corners for
//                      // a fixed shape
//   crop.setRatio(r)   // a fixed shape (width / height), "lock" (the shape
//                      // it has now), or null (free)

const KagazCrop = (() => {
  const CORNERS = ["top left", "top right", "bottom right", "bottom left"];
  const SIDES = ["top", "right", "bottom", "left"];
  // A fixed shape's corners, as (across, down) from its middle.
  const SIGNS = [[-1, -1], [1, -1], [1, 1], [-1, 1]];

  function attach(box, { src, outlines, radius = 0.015, names, ratio = null, removable = true }) {
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
    // and each side's middle (null: halfway between its corners). A fixed
    // shape: its middle, width, height (pixels) and turn (radians).
    const frames = outlines.map((o, i) => ({
      start: copy(o.quad),
      corners: copy(o.quad),
      mids: [null, null, null, null],
      fixed: null,
      // What a new shape starts from: the frame before any shape, or as
      // the person last set it (so trying shapes does not shrink it).
      base: null,
      kept: true,
      name: (names && names[i]) || `card ${i + 1}`,
      handles: [],
      midHandles: [],
      turn: null,
      remove: null,
    }));
    const half = (a, b) => [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    const mid = (f, i) => f.mids[i] || half(f.corners[i], f.corners[(i + 1) % 4]);
    // Corner, middle, corner, middle, ... as Rust takes it; a fixed shape's
    // four corners.
    const shape = (f) => (f.fixed ? f.corners.map((c) => c.slice()) : f.corners.flatMap((c, i) => [c.slice(), mid(f, i).slice()]));

    // The picture's size in pixels, which a fixed shape is measured in.
    const W = () => img.naturalWidth || 1000;
    const H = () => img.naturalHeight || 1414;
    const px = (p) => [p[0] * W(), p[1] * H()];
    const fr = (p) => [p[0] / W(), p[1] / H()];
    const dist = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1]);
    const axes = (g) => [[Math.cos(g.a), Math.sin(g.a)], [-Math.sin(g.a), Math.cos(g.a)]];
    // A fixed shape's corner (signs from SIGNS), in pixels.
    const cornerOf = (g, [sx, sy]) => {
      const [u, v] = axes(g);
      return [g.c[0] + (u[0] * sx * g.w + v[0] * sy * g.h) / 2, g.c[1] + (u[1] * sx * g.w + v[1] * sy * g.h) / 2];
    };
    const fits = (g) => SIGNS.every((s) => {
      const [x, y] = cornerOf(g, s);
      return x >= -0.5 && y >= -0.5 && x <= W() + 0.5 && y <= H() + 0.5;
    });
    // A fixed shape takes effect only where it fits on the picture.
    function setFixed(f, g) {
      if (!fits(g)) return false;
      f.fixed = g;
      f.corners = SIGNS.map((s) => fr(cornerOf(g, s)));
      f.mids = [null, null, null, null];
      return true;
    }

    // A frame made `r` wide per high (or kept as it is: "lock"), as large
    // as fits inside it, its middle and turn kept (the turn: its bottom
    // side's, as Rust levels it).
    function fix(f, r) {
      if (!f.base) f.base = copy(f.corners);
      const [tl, tr, br, bl] = f.base.map(px);
      const w0 = (dist(tl, tr) + dist(bl, br)) / 2;
      const h0 = (dist(tl, bl) + dist(tr, br)) / 2;
      if (r === "lock") r = w0 / Math.max(h0, 1);
      const g = {
        c: [(tl[0] + tr[0] + br[0] + bl[0]) / 4, (tl[1] + tr[1] + br[1] + bl[1]) / 4],
        a: Math.atan2(br[1] - bl[1], br[0] - bl[0]),
        w: Math.min(w0, h0 * r),
        h: 0,
        r,
      };
      if (Math.abs(g.a) > Math.PI / 2) g.a = 0;
      g.h = g.w / r;
      // Smaller, then square to the page in its middle, until it fits.
      for (let i = 0; i < 40 && !fits(g); i++) {
        if (i === 20) Object.assign(g, { a: 0, c: [W() / 2, H() / 2] });
        g.w *= 0.92;
        g.h = g.w / r;
      }
      setFixed(f, g);
    }

    // The outline in the picture's pixels, its corners rounded as Rust
    // rounds them (a curve between the points `r` along each side).
    function path(f, w, h) {
      const pts = shape(f).map(([x, y]) => [x * w, y * h]);
      const r = f.fixed ? 0 : radius * w;
      const every = f.fixed ? 1 : 2;
      let d = "";
      pts.forEach((c, i) => {
        if (i % every) {
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

    // The turn handle: off the top side's middle, or the bottom's when
    // the top is near the picture's edge. (where, and which side.)
    function turnAt(f) {
      const g = f.fixed;
      const [, v] = axes(g);
      const r = stage.getBoundingClientRect();
      const off = 30 * (r.width ? W() / r.width : W() / 500);
      const above = [g.c[0] - v[0] * (g.h / 2 + off), g.c[1] - v[1] * (g.h / 2 + off)];
      if (above[1] >= 0 && above[0] >= 0 && above[0] <= W()) return [fr(above), -1];
      return [fr([g.c[0] + v[0] * (g.h / 2 + off), g.c[1] + v[1] * (g.h / 2 + off)]), 1];
    }

    const clamp = (v) => Math.min(1, Math.max(0, v));
    const place = (el, p) => {
      el.style.left = `${p[0] * 100}%`;
      el.style.top = `${p[1] * 100}%`;
    };
    function draw() {
      const w = W();
      const h = H();
      svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
      // Dim outside the kept frames (even-odd: the page, less each frame);
      // inside each, a body to drag it by.
      const kept = frames.filter((f) => f.kept);
      svg.innerHTML =
        `<path class="crop-dim" fill-rule="evenodd" d="M0,0H${w}V${h}H0Z ${kept.map((f) => path(f, w, h)).join(" ")}"/>` +
        frames.map((f, i) => (f.kept ? `<path class="crop-body" data-frame="${i}" d="${path(f, w, h)}"/>` : "")).join("") +
        kept.map((f) => `<path class="crop-line" d="${path(f, w, h)}"/>`).join("") +
        kept.filter((f) => f.fixed).map((f) => {
          const [t, side] = turnAt(f);
          const [, v] = axes(f.fixed);
          const edge = [f.fixed.c[0] + side * v[0] * f.fixed.h / 2, f.fixed.c[1] + side * v[1] * f.fixed.h / 2];
          return `<line class="crop-line" x1="${edge[0]}" y1="${edge[1]}" x2="${t[0] * w}" y2="${t[1] * h}"/>`;
        }).join("");
      for (const f of frames) {
        f.handles.forEach((el, i) => {
          el.hidden = !f.kept;
          place(el, f.corners[i]);
        });
        f.midHandles.forEach((el, i) => {
          el.hidden = !f.kept || !!f.fixed;
          place(el, mid(f, i));
        });
        f.turn.hidden = !f.kept || !f.fixed;
        if (f.fixed) place(f.turn, turnAt(f)[0]);
        f.remove.hidden = !f.kept || !removable;
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
          f.base = null;
          if (f.fixed) fix(f, f.fixed.r);
          draw();
        });
        tools.appendChild(add);
      }
    }

    // Where the pointer is on the picture, as fractions.
    const at = (e) => {
      const r = stage.getBoundingClientRect();
      return [clamp((e.clientX - r.left) / r.width), clamp((e.clientY - r.top) / r.height)];
    };

    // A handle: dragged with a mouse or a finger, nudged with the arrows
    // (`key`, when given, takes the arrows itself).
    function handle(cls, label, get, set, key) {
      const el = document.createElement("button");
      el.type = "button";
      el.className = cls;
      el.setAttribute("aria-label", label);
      el.addEventListener("pointerdown", (ev) => {
        ev.preventDefault();
        el.setPointerCapture(ev.pointerId);
        const move = (e) => {
          set(at(e));
          draw();
        };
        const up = () => {
          if (el.settled) el.settled();
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
        if (key) key(d, e.shiftKey);
        else {
          const p = get();
          set([clamp(p[0] + d[0]), clamp(p[1] + d[1])]);
        }
        if (el.settled) el.settled();
        draw();
      });
      stage.appendChild(el);
      return el;
    }

    // A fixed shape's corner, dragged: the opposite corner stays, and the
    // frame grows or shrinks along its diagonal, keeping its shape.
    function resize(f, i, p) {
      const g = f.fixed;
      const [sx, sy] = SIGNS[i];
      const o = cornerOf(g, [-sx, -sy]);
      const [u, v] = axes(g);
      const d = [px(p)[0] - o[0], px(p)[1] - o[1]];
      const across = sx * (d[0] * u[0] + d[1] * u[1]);
      const down = sy * (d[0] * v[0] + d[1] * v[1]);
      const t = Math.max((across * g.r + down) / (g.r * g.r + 1), Math.min(W(), H()) * 0.02);
      const w = t * g.r;
      const c = [o[0] + (u[0] * sx * w + v[0] * sy * t) / 2, o[1] + (u[1] * sx * w + v[1] * sy * t) / 2];
      setFixed(f, { ...g, c, w, h: t });
    }

    // Turned by the handle: the frame faces where the handle is pulled,
    // square to the page within a degree.
    function turn(f, p) {
      const g = f.fixed;
      const [, side] = f.turnSide || turnAt(f);
      const q = px(p);
      let a = Math.atan2(q[1] - g.c[1], q[0] - g.c[0]) - side * Math.PI / 2;
      a = Math.atan2(Math.sin(a), Math.cos(a));
      if (Math.abs(a) < Math.PI / 180) a = 0;
      setFixed(f, { ...g, a });
    }

    for (const f of frames) {
      f.handles = CORNERS.map((corner, i) =>
        handle("crop-handle", `${corner[0].toUpperCase()}${corner.slice(1)} corner of the ${f.name} frame: drag, or arrow keys`,
          () => f.corners[i], (p) => {
            if (f.fixed) resize(f, i, p);
            else f.corners[i] = p;
          }));
      // A side's middle: free once moved; until then it follows its corners.
      f.midHandles = SIDES.map((side, i) =>
        handle("crop-handle crop-mid", `Middle of the ${side} side of the ${f.name} frame: drag to bend the side, or arrow keys`,
          () => mid(f, i), (p) => { f.mids[i] = p; }));
      f.turn = handle("crop-handle crop-turn", `Turn the ${f.name} frame: drag around, or left and right arrow keys`,
        () => null, (p) => turn(f, p),
        (d, big) => {
          if (d[0]) setFixed(f, { ...f.fixed, a: f.fixed.a + Math.sign(d[0]) * (big ? 5 : 0.5) * Math.PI / 180 });
        });
      // Which side the handle was on stays put while it is dragged.
      f.turn.addEventListener("pointerdown", () => { f.turnSide = turnAt(f); });
      f.turn.addEventListener("pointerup", () => { f.turnSide = null; });
      // A fixed shape, once adjusted, is what the next shape starts from.
      const settled = () => { if (f.fixed) f.base = copy(f.corners); };
      for (const el of [...f.handles, f.turn]) el.settled = settled;
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

    // Dragged by its inside, a frame moves whole, staying on the picture.
    svg.addEventListener("pointerdown", (ev) => {
      const body = ev.target.closest && ev.target.closest(".crop-body");
      if (!body) return;
      const f = frames[Number(body.dataset.frame)];
      ev.preventDefault();
      stage.setPointerCapture(ev.pointerId);
      const from = at(ev);
      const was = { corners: copy(f.corners), mids: f.mids.map((m) => m && m.slice()), fixed: f.fixed && { ...f.fixed } };
      const move = (e) => {
        const now = at(e);
        let d = [now[0] - from[0], now[1] - from[1]];
        const pts = was.corners.concat(was.mids.filter(Boolean));
        // As far as the picture's edges allow.
        const lo = [Math.max(...pts.map((p) => -p[0])), Math.max(...pts.map((p) => -p[1]))];
        const hi = [Math.min(...pts.map((p) => 1 - p[0])), Math.min(...pts.map((p) => 1 - p[1]))];
        d = [Math.min(hi[0], Math.max(lo[0], d[0])), Math.min(hi[1], Math.max(lo[1], d[1]))];
        if (was.fixed) setFixed(f, { ...was.fixed, c: [was.fixed.c[0] + d[0] * W(), was.fixed.c[1] + d[1] * H()] });
        else {
          f.corners = was.corners.map((p) => [p[0] + d[0], p[1] + d[1]]);
          f.mids = was.mids.map((m) => m && [m[0] + d[0], m[1] + d[1]]);
        }
        draw();
      };
      const up = () => {
        if (f.fixed) f.base = copy(f.corners);
        stage.removeEventListener("pointermove", move);
        stage.removeEventListener("pointerup", up);
        stage.removeEventListener("pointercancel", up);
      };
      stage.addEventListener("pointermove", move);
      stage.addEventListener("pointerup", up);
      stage.addEventListener("pointercancel", up);
    });

    function setRatio(r) {
      for (const f of frames) {
        if (r == null) Object.assign(f, { fixed: null, base: null });
        else fix(f, r);
      }
      draw();
    }

    img.addEventListener("load", () => {
      if (ratio != null) setRatio(ratio);
      else draw();
    });
    img.src = src;
    draw();

    return {
      quads: () => frames.filter((f) => f.kept).map(shape),
      setRatio,
    };
  }

  return { attach };
})();
