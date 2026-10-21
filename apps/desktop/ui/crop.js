// Frames around cards on a scanned page, adjusted by hand, as in Adobe
// Scan: drag a frame's four corners onto the card (a tilted card too);
// everything outside the frames turns white when saved. An ID page has two
// frames, one per half; remove one for a one-sided ID, add it back after.
// The card is never stretched or moved: it keeps its place and real size.
//
//   const crop = KagazCrop.attach(box, { src, outlines: [{ quad, found }],
//                                        names: ["front", "back"], onChange });
//   crop.quads()  // the frames kept: [[[x, y] x4]], fractions of the page

const KagazCrop = (() => {
  const CORNERS = ["top left", "top right", "bottom right", "bottom left"];

  function attach(box, { src, outlines, names, onChange }) {
    box.innerHTML = "";
    box.classList.add("crop");
    const stage = document.createElement("div");
    stage.className = "crop-stage";
    const img = document.createElement("img");
    img.alt = "The page, with a frame around each card";
    img.src = src;
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 1 1");
    svg.setAttribute("preserveAspectRatio", "none");
    svg.setAttribute("aria-hidden", "true");
    stage.append(img, svg);
    const tools = document.createElement("div");
    tools.className = "crop-tools";
    box.append(stage, tools);

    // Each frame: where it started (to add it back), where it is, kept or not.
    const frames = outlines.map((o, i) => ({
      start: o.quad.map((p) => p.slice()),
      quad: o.quad.map((p) => p.slice()),
      kept: true,
      name: (names && names[i]) || `card ${i + 1}`,
      handles: [],
      remove: null,
    }));

    const clamp = (v) => Math.min(1, Math.max(0, v));
    function draw() {
      // Dim outside the kept frames (even-odd: the page, less each frame).
      const kept = frames.filter((f) => f.kept);
      const outline = (q) => `M${q.map((p) => `${p[0]},${p[1]}`).join("L")}Z`;
      svg.innerHTML =
        `<path class="crop-dim" fill-rule="evenodd" d="M0,0H1V1H0Z ${kept.map((f) => outline(f.quad)).join(" ")}"/>` +
        kept.map((f) => `<path class="crop-line" d="${outline(f.quad)}"/>`).join("");
      for (const f of frames) {
        f.handles.forEach((h, i) => {
          h.hidden = !f.kept;
          h.style.left = `${f.quad[i][0] * 100}%`;
          h.style.top = `${f.quad[i][1] * 100}%`;
        });
        f.remove.hidden = !f.kept;
        f.remove.style.left = `${((f.quad[0][0] + f.quad[1][0]) / 2) * 100}%`;
        f.remove.style.top = `${((f.quad[0][1] + f.quad[1][1]) / 2) * 100}%`;
      }
      tools.innerHTML = "";
      for (const f of frames.filter((f) => !f.kept)) {
        const add = document.createElement("button");
        add.type = "button";
        add.textContent = `Add the ${f.name} frame`;
        add.addEventListener("click", () => {
          f.kept = true;
          f.quad = f.start.map((p) => p.slice());
          changed();
        });
        tools.appendChild(add);
      }
    }
    const changed = () => {
      draw();
      if (onChange) onChange();
    };

    for (const f of frames) {
      f.handles = CORNERS.map((corner, i) => {
        const h = document.createElement("button");
        h.type = "button";
        h.className = "crop-handle";
        h.setAttribute("aria-label", `${corner[0].toUpperCase()}${corner.slice(1)} corner of the ${f.name} frame: drag, or arrow keys`);
        // Drag with a mouse or a finger.
        h.addEventListener("pointerdown", (ev) => {
          ev.preventDefault();
          h.setPointerCapture(ev.pointerId);
          const move = (e) => {
            const r = stage.getBoundingClientRect();
            f.quad[i] = [clamp((e.clientX - r.left) / r.width), clamp((e.clientY - r.top) / r.height)];
            draw();
          };
          const up = () => {
            h.removeEventListener("pointermove", move);
            h.removeEventListener("pointerup", up);
            h.removeEventListener("pointercancel", up);
            if (onChange) onChange();
          };
          h.addEventListener("pointermove", move);
          h.addEventListener("pointerup", up);
          h.addEventListener("pointercancel", up);
        });
        // Or the keyboard: arrows nudge, with Shift further.
        h.addEventListener("keydown", (e) => {
          const step = e.shiftKey ? 0.02 : 0.004;
          const d = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }[e.key];
          if (!d) return;
          e.preventDefault();
          f.quad[i] = [clamp(f.quad[i][0] + d[0]), clamp(f.quad[i][1] + d[1])];
          changed();
        });
        stage.appendChild(h);
        return h;
      });
      f.remove = document.createElement("button");
      f.remove.type = "button";
      f.remove.className = "crop-remove";
      f.remove.textContent = "✕";
      f.remove.title = `Remove the ${f.name} frame (a one-sided ID)`;
      f.remove.setAttribute("aria-label", f.remove.title);
      f.remove.addEventListener("click", () => {
        f.kept = false;
        changed();
      });
      stage.appendChild(f.remove);
    }
    draw();

    return {
      quads: () => frames.filter((f) => f.kept).map((f) => f.quad.map((p) => p.slice())),
    };
  }

  return { attach };
})();
