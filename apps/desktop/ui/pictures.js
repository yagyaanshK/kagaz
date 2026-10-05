// Small line drawings next to the options, for people who would rather look
// than read. Everything is inline SVG in currentColor so it follows the theme.

const Pictures = (() => {
  const NS = 'xmlns="http://www.w3.org/2000/svg"';
  const stroke = 'fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"';

  // A flatbed scanner seen from the front-top: lid, glass, feeder tray on top.
  function scanner(opts) {
    const { glass = false, feeder = false, duplex = false } = opts;
    const page = (x, y, w, h, extra = "") => `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="1.5" ${extra}/>`;
    return `<svg ${NS} viewBox="0 0 120 72" ${stroke} aria-hidden="true">
      <!-- body -->
      <rect x="8" y="34" width="104" height="30" rx="4"/>
      <!-- glass -->
      <rect x="18" y="40" width="72" height="18" rx="2" ${glass ? 'class="hi"' : ""}/>
      <!-- control panel -->
      <rect x="96" y="42" width="10" height="4" rx="1"/><rect x="96" y="50" width="10" height="4" rx="1"/>
      <!-- lid, open a little -->
      <path d="M8 34 L14 14 L96 14 L104 34"/>
      <!-- feeder tray on the lid -->
      <path d="M30 14 L34 6 L86 6 L90 14" ${feeder ? 'class="hi"' : ""}/>
      ${glass ? `<g class="hi">${page(20, 41, 22, 15, 'fill="currentColor" fill-opacity="0.18"')}<path d="M22 44 l4 0 M22 48 l10 0 M22 52 l7 0"/><path d="M56 30 l-10 10 M46 40 l6 -1 M46 40 l1 -6"/></g>` : ""}
      ${feeder ? `<g class="hi">${page(42, 8, 36, 5, 'fill="currentColor" fill-opacity="0.18"')}${duplex ? page(44, 2, 32, 5, "") : ""}<path d="M60 -2 l0 6 M57 2 l3 3 l3 -3"/></g>` : ""}
    </svg>`;
  }

  // Paper outlines with their proportions.
  function paper(kind) {
    const dims = { A4: [210, 297], Letter: [216, 279], Legal: [216, 356], Max: [216, 297] };
    const [w, h] = dims[kind] || dims.A4;
    const scale = 40 / 356;
    const pw = w * scale, ph = h * scale;
    const x = (60 - pw) / 2, y = (48 - ph) / 2 + 4;
    return `<svg ${NS} viewBox="0 0 60 56" ${stroke} aria-hidden="true">
      <rect x="${x.toFixed(1)}" y="${y.toFixed(1)}" width="${pw.toFixed(1)}" height="${ph.toFixed(1)}" rx="1.5" ${kind === "Max" ? 'stroke-dasharray="3 2"' : ""}/>
      <text x="30" y="${(y + ph + 8).toFixed(1)}" text-anchor="middle" font-size="7" fill="currentColor" stroke="none">${kind === "Max" ? "whole glass" : kind}</text>
    </svg>`;
  }

  // Three swatches: colour, grey, black and white.
  function colour(mode) {
    const on = (m) => (m === mode ? 'class="hi"' : 'opacity="0.35"');
    return `<svg ${NS} viewBox="0 0 96 40" aria-hidden="true">
      <defs><linearGradient id="g-col" x1="0" x2="1"><stop offset="0" stop-color="#d9534f"/><stop offset="0.5" stop-color="#5cb85c"/><stop offset="1" stop-color="#337ab7"/></linearGradient>
      <linearGradient id="g-grey" x1="0" x2="1"><stop offset="0" stop-color="#222"/><stop offset="1" stop-color="#ddd"/></linearGradient></defs>
      <g ${on("Color")}><rect x="2" y="6" width="26" height="22" rx="3" fill="url(#g-col)"/><text x="15" y="37" text-anchor="middle" font-size="7" fill="currentColor">colour</text></g>
      <g ${on("Gray")}><rect x="35" y="6" width="26" height="22" rx="3" fill="url(#g-grey)"/><text x="48" y="37" text-anchor="middle" font-size="7" fill="currentColor">grey</text></g>
      <g ${on("BlackWhite")}><rect x="68" y="6" width="13" height="22" rx="3" fill="#111"/><rect x="81" y="6" width="13" height="22" rx="3" fill="#fff" stroke="#111" stroke-width="1"/><text x="81" y="37" text-anchor="middle" font-size="7" fill="currentColor">b &amp; w</text></g>
    </svg>`;
  }

  // Dots per inch: a coarse or fine dot grid.
  function dots(dpi) {
    const n = dpi >= 600 ? 8 : dpi >= 300 ? 5 : 3;
    let out = "";
    const step = 32 / n;
    for (let i = 0; i < n; i++) for (let j = 0; j < n; j++) {
      out += `<circle cx="${(4 + step / 2 + i * step).toFixed(1)}" cy="${(4 + step / 2 + j * step).toFixed(1)}" r="${(step / 5).toFixed(1)}" fill="currentColor"/>`;
    }
    return `<svg ${NS} viewBox="0 0 40 40" aria-hidden="true">${out}</svg>`;
  }

  // A file with its extension.
  function file(ext) {
    return `<svg ${NS} viewBox="0 0 40 48" ${stroke} aria-hidden="true">
      <path d="M8 4 h16 l10 10 v30 h-26 z"/><path d="M24 4 v10 h10"/>
      <text x="21" y="36" text-anchor="middle" font-size="9" font-weight="600" fill="currentColor" stroke="none">${ext}</text>
    </svg>`;
  }

  // Verdict badges for the About pane.
  function verdict(status) {
    const ring = '<circle cx="12" cy="12" r="10"/>';
    const mark = {
      Works: '<path d="M7 12.5 l3.5 3.5 l6.5 -7"/>',
      NeedsSetup: '<path d="M12 7 v5 l3 2"/><circle cx="12" cy="12" r="4"/>',
      NeedsDriver: '<path d="M12 6 v9 M8.5 11.5 l3.5 3.5 l3.5 -3.5 M7 17 h10"/>',
      Unknown: '<path d="M9.5 9.5 a2.5 2.5 0 1 1 3.5 2.3 c-0.8 0.4 -1 1 -1 2"/><circle cx="12" cy="17" r="0.8" fill="currentColor"/>',
    }[status] || "";
    return `<svg ${NS} viewBox="0 0 24 24" ${stroke} class="verdict ${status}" aria-hidden="true">${ring}${mark}</svg>`;
  }

  function set(id, svg) {
    const el = document.getElementById(id);
    if (el) el.innerHTML = svg;
  }

  function update(form) {
    const f = new FormData(form);
    const source = f.get("source");
    set("pic-source", scanner({
      glass: source === "Glass" || source === "Auto",
      feeder: source !== "Glass",
      duplex: source === "FeederDuplex",
    }));
    set("pic-dpi", dots(Number(f.get("dpi"))));
    set("pic-color", colour(f.get("color")));
    set("pic-paper", paper(f.get("paper")));
    set("pic-format", file({ Pdf: "PDF", Jpeg: "JPG", Png: "PNG" }[f.get("format")] || ""));
  }

  return { update, verdict };
})();
