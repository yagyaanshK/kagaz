// Every icon in Kagaz, drawn once on the same 24-unit grid and stroked the
// same way (style.css: round ends and joins, --icon-stroke wide), so they
// match wherever they sit. The rules they keep: each stroke closes into a
// shape or ends on another stroke (no loose lines); corners are rounded
// alike; the drawing stays inside 2..22 so no icon looks bigger than the
// next. Details meant to stand alone (a keyhole, a speaker's sound waves,
// a fridge's handles) are short, centred and symmetric.
//
// KagazIcons.svg("bell") gives the markup; an element written as
// <span class="ic" data-icon="bell"></span> in the page is filled in at start.

const KagazIcons = (() => {
  // Shared pieces.
  const HANDSET = "M6.5 3.5h3l1.5 4.5-2 1.3a11 11 0 0 0 5.7 5.7l1.3-2 4.5 1.5v3a2 2 0 0 1-2 2A16.5 16.5 0 0 1 4.5 5.5a2 2 0 0 1 2-2z";
  const SPEAKER = "M4 9.5h3.5L12 5.5v13l-4.5-4H4z";
  const MIC = "M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3zM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21M9 21h6";
  const CAR_BODY = '<rect x="3.5" y="11.5" width="17" height="5.5" rx="1.5"/><path d="M5 11.5 6.6 7a1.5 1.5 0 0 1 1.4-1h8a1.5 1.5 0 0 1 1.4 1l1.6 4.5M6.5 17v2M17.5 17v2"/><path d="M7 14.25h.01M17 14.25h.01" class="dot"/>';
  const PHOTO = "M3 8.5A1.5 1.5 0 0 1 4.5 7h2.7l1.6-2.2A1.5 1.5 0 0 1 10 4h4a1.5 1.5 0 0 1 1.2.8L16.8 7h2.7A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z";

  const P = {
    // ---- places ----
    home: '<path d="M4 10.2 12 3.8l8 6.4V19a1.5 1.5 0 0 1-1.5 1.5H15v-5.5a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v5.5H5.5A1.5 1.5 0 0 1 4 19z"/>',
    settings: '<path d="M10.38 4.98 10.71 2.79A9.3 9.3 0 0 1 13.29 2.79L13.62 4.98A7.2 7.2 0 0 1 15.82 5.89L17.6 4.57A9.3 9.3 0 0 1 19.43 6.4L18.11 8.18A7.2 7.2 0 0 1 19.02 10.38L21.21 10.71A9.3 9.3 0 0 1 21.21 13.29L19.02 13.62A7.2 7.2 0 0 1 18.11 15.82L19.43 17.6A9.3 9.3 0 0 1 17.6 19.43L15.82 18.11A7.2 7.2 0 0 1 13.62 19.02L13.29 21.21A9.3 9.3 0 0 1 10.71 21.21L10.38 19.02A7.2 7.2 0 0 1 8.18 18.11L6.4 19.43A9.3 9.3 0 0 1 4.57 17.6L5.89 15.82A7.2 7.2 0 0 1 4.98 13.62L2.79 13.29A9.3 9.3 0 0 1 2.79 10.71L4.98 10.38A7.2 7.2 0 0 1 5.89 8.18L4.57 6.4A9.3 9.3 0 0 1 6.4 4.57L8.18 5.89A7.2 7.2 0 0 1 10.38 4.98z"/><circle cx="12" cy="12" r="3"/>',
    tools: '<path d="M14.7 6.3a4 4 0 0 0-5.4 5.4l-5.9 5.9a2.1 2.1 0 0 0 3 3l5.9-5.9a4 4 0 0 0 5.4-5.4l-2.5 2.5-2.4-.6-.6-2.4z"/>',

    // ---- kinds of device ----
    doorbell: '<rect x="7" y="2.5" width="10" height="19" rx="3"/><circle cx="12" cy="8" r="2"/><circle cx="12" cy="15.5" r="2.5"/>',
    cameras: '<rect x="3" y="6" width="18" height="4" rx="1.5"/><path d="M5.5 10v1a6.5 6.5 0 0 0 13 0v-1"/><circle cx="12" cy="13" r="2.4"/>',
    print: '<path d="M7 8V4.5A1.5 1.5 0 0 1 8.5 3h7A1.5 1.5 0 0 1 17 4.5V8M7 17H5a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2"/><rect x="7" y="13.5" width="10" height="7.5" rx="1"/>',
    fan: '<circle cx="12" cy="12" r="2.6"/><path d="M11.27 9.51C10.02 5.47 12.17 2.11 15.09 2.79 17.82 3.41 17.08 6.94 13.88 10.21M14.52 12.62C18.65 13.56 20.49 17.09 18.43 19.28 16.53 21.33 13.84 18.93 12.62 14.52M10.21 13.88C7.33 16.98 3.35 16.8 2.48 13.93 1.65 11.25 5.08 10.13 9.51 11.27"/>',
    purifier: '<path d="M3 9h10.5A2.5 2.5 0 1 0 11 6.5M3 13h15.5a2.5 2.5 0 1 1-2.5 2.5M3 17h7"/>',
    society: '<path d="M4.5 20V7.5A1.5 1.5 0 0 1 6 6h1a1.5 1.5 0 0 1 1.5 1.5V20M3 20h7M8.5 9h11a1.5 1.5 0 0 1 0 3h-11M12.5 9 11 12M16.5 9 15 12"/>',
    photo: `<path d="${PHOTO}"/><circle cx="12" cy="12.5" r="3.5"/>`,
    vacuum: '<circle cx="12" cy="12" r="9"/><path d="M3.52 9h16.96"/><circle cx="12" cy="14" r="2.5"/>',
    tv: '<rect x="2.5" y="4" width="19" height="13" rx="2"/><path d="M7 17l-1.5 3.5M17 17l1.5 3.5"/>',
    fridge: '<rect x="5.5" y="2.5" width="13" height="19" rx="2"/><path d="M5.5 9.5h13M9 5.5V7M9 12.5v3"/>',
    washer: '<rect x="4" y="2.5" width="16" height="19" rx="2"/><path d="M4 7h16"/><circle cx="12" cy="14" r="4"/><path d="M7 4.75h.01M10 4.75h.01" class="dot"/>',
    ac: '<rect x="2.5" y="5" width="19" height="9" rx="2"/><path d="M6 10.5h12M7 17.5 6 20M12 17.5V20M17 17.5l1 2.5"/>',
    indoor: '<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M12 16v4M8 20h8"/>',
    guard: '<path d="M12 3l7.5 3v5.5c0 4.6-3.2 8.3-7.5 9.5-4.3-1.2-7.5-4.9-7.5-9.5V6z"/>',
    mobile: '<rect x="7" y="2.5" width="10" height="19" rx="2.5"/><path d="M11 18.5h2"/>',
    family: '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0M14.2 16.1A5 5 0 0 1 21.5 20"/><circle cx="16.5" cy="9" r="2.5"/>',
    device: '<rect x="4" y="4" width="16" height="16" rx="3"/><circle cx="12" cy="12" r="2"/>',

    // ---- actions ----
    plus: '<path d="M12 5v14M5 12h14"/>',
    menu: '<path d="M4 7h16M4 12h16M4 17h16"/>',
    back: '<path d="M15 5l-7 7 7 7"/>',
    forward: '<path d="M9 5l7 7-7 7"/>',
    down: '<path d="M5 9l7 7 7-7"/>',
    close: '<path d="M6 6l12 12M18 6 6 18"/>',
    check: '<path d="M5 12.5l4.5 4.5L19 7"/>',
    search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="M15.2 15.2 20 20"/>',
    eye: '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/><circle cx="12" cy="12" r="3"/>',
    qr: '<rect x="3.5" y="3.5" width="7" height="7" rx="1.5"/><rect x="13.5" y="3.5" width="7" height="7" rx="1.5"/><rect x="3.5" y="13.5" width="7" height="7" rx="1.5"/><path d="M7 7h.01M17 7h.01M7 17h.01M14.5 14.5h.01M19.5 14.5h.01M17 17h.01M14.5 19.5h.01M19.5 19.5h.01" class="dot"/>',
    grid: '<rect x="3.5" y="3.5" width="7" height="7" rx="1.5"/><rect x="13.5" y="3.5" width="7" height="7" rx="1.5"/><rect x="3.5" y="13.5" width="7" height="7" rx="1.5"/><rect x="13.5" y="13.5" width="7" height="7" rx="1.5"/>',
    full: '<path d="M4 9V5.5A1.5 1.5 0 0 1 5.5 4H9M15 4h3.5A1.5 1.5 0 0 1 20 5.5V9M20 15v3.5a1.5 1.5 0 0 1-1.5 1.5H15M9 20H5.5A1.5 1.5 0 0 1 4 18.5V15"/>',
    unfull: '<path d="M9 4v3.5A1.5 1.5 0 0 1 7.5 9H4M20 9h-3.5A1.5 1.5 0 0 1 15 7.5V4M15 20v-3.5a1.5 1.5 0 0 1 1.5-1.5H20M4 15h3.5A1.5 1.5 0 0 1 9 16.5V20"/>',
    enter: '<path d="M14 4h4.5A1.5 1.5 0 0 1 20 5.5v13a1.5 1.5 0 0 1-1.5 1.5H14M3.5 12H14M10 8l4 4-4 4"/>',
    leave: '<path d="M10 4H5.5A1.5 1.5 0 0 0 4 5.5v13A1.5 1.5 0 0 0 5.5 20H10M20.5 12H10M14 8l-4 4 4 4"/>',
    reload: '<path d="M19 12a7.5 7.5 0 1 0-3.75 6.5M21.1 9.9 19 12l-2.1-2.1"/>',
    star: '<path d="M12 3.5l2.6 5.3 5.9.9-4.25 4.1 1 5.8L12 16.9l-5.25 2.7 1-5.8L3.5 9.7l5.9-.9z"/>',
    more: '<path d="M5.5 12h.01M12 12h.01M18.5 12h.01" class="dot"/>',

    // ---- calls and sound ----
    phone: `<path d="${HANDSET}"/>`,
    incoming: `<path d="${HANDSET}"/><path d="M20.5 3.5l-6 6M14.5 4.5v5h5"/>`,
    outgoing: `<path d="${HANDSET}"/><path d="M14.5 9.5l6-6M15.5 3.5h5v5"/>`,
    missed: '<path d="M3.5 6.5l6 6 4-4 7 7M20.5 10.5v5h-5"/>',
    bell: '<path d="M6 9.5a6 6 0 0 1 12 0c0 4 1 5.8 2 6.8.3.3.1.7-.3.7H4.3c-.4 0-.6-.4-.3-.7 1-1 2-2.8 2-6.8zM10 17v.5a2 2 0 0 0 4 0V17"/>',
    speaker: `<path d="${SPEAKER}"/><path d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11"/>`,
    mute: `<path d="${SPEAKER}"/><path d="M16 9.5l5 5M21 9.5l-5 5"/>`,
    mic: `<path d="${MIC}"/>`,
    micOff: `<path d="${MIC}M4 4l16 16"/>`,
    video: '<rect x="3" y="7" width="12.5" height="10" rx="2"/><path d="M15.5 10.5 21 7.5v9l-5.5-3"/>',
    videoOff: '<rect x="3" y="7" width="12.5" height="10" rx="2"/><path d="M15.5 10.5 21 7.5v9l-5.5-3M3 4l18 16"/>',
    unlock: '<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7.5a4 4 0 0 1 7.7-1.5M12 15v2"/>',

    // ---- cameras and recordings ----
    shot: `<path d="${PHOTO}"/><circle cx="12" cy="12.5" r="3.5"/>`,
    rec: '<circle cx="12" cy="12" r="7.5"/><circle cx="12" cy="12" r="3.5" class="fill"/>',
    past: '<path d="M5 12a7.5 7.5 0 1 1 3.75 6.5M2.9 9.9 5 12l2.1-2.1M12.5 8v4.2l2.8 1.8"/>',
    live: '<circle cx="12" cy="12" r="2.5" class="fill"/><path d="M7.8 7.8a6 6 0 0 0 0 8.4M16.2 7.8a6 6 0 0 1 0 8.4M5 5a10 10 0 0 0 0 14M19 5a10 10 0 0 1 0 14"/>',
    pause: '<path d="M9 6v12M15 6v12"/>',
    play: '<path d="M8 5.5v13l10.5-6.5z" class="fill"/>',

    // ---- the society's gate ----
    guest: '<circle cx="12" cy="8" r="4"/><path d="M4.5 20.5a7.5 7.5 0 0 1 15 0"/>',
    delivery: '<rect x="2.5" y="6" width="11" height="10" rx="1"/><path d="M13.5 9h4l3.5 3.5V16h-7.5"/><circle cx="7" cy="17.8" r="1.8"/><circle cx="17" cy="17.8" r="1.8"/>',
    car: CAR_BODY,
    cab: CAR_BODY + '<path d="M10 6V4.5h4V6"/>',
  };

  function svg(name, cls) {
    const body = P[name];
    if (!body) return "";
    return `<svg viewBox="0 0 24 24" class="kz-i${cls ? " " + cls : ""}" aria-hidden="true">${body}</svg>`;
  }

  // A button's face: the icon, then its word (the word stays for screen
  // readers and for windows wide enough to show it).
  function label(name, text) {
    const span = document.createElement("span");
    span.textContent = text;
    return svg(name) + span.outerHTML;
  }

  // Fill the page's placeholders: <span class="ic" data-icon="bell"></span>.
  function fill(root) {
    (root || document).querySelectorAll("[data-icon]").forEach((el) => {
      if (!el.firstElementChild) el.innerHTML = svg(el.dataset.icon);
    });
  }
  fill();

  return { svg, label, fill, names: Object.keys(P) };
})();
