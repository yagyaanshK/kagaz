// The kinds of device Kagaz knows, and the brands people look for in each.
// A brand Kagaz works with shows in colour; the others show greyed with
// "Coming soon", so people can see what is planned. Logos come from
// ui/brands/ (see its README for where each file is from); a brand without
// a file is shown as its name in its colour.

const KagazBrands = (() => {
  const ICON = {
    doorbell: '<svg viewBox="0 0 24 24"><path d="M6 8a6 6 0 0 1 12 0v5l2 3H4l2-3zM10 19a2 2 0 0 0 4 0"/></svg>',
    security: '<svg viewBox="0 0 24 24"><path d="M4 6h12l-1 6H5zM10 12v4M6 20h8M18 8l3-1"/></svg>',
    print: '<svg viewBox="0 0 24 24"><path d="M6 9V3h12v6M6 18H4a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2M6 14h12v7H6z"/></svg>',
    camera: '<svg viewBox="0 0 24 24"><path d="M4 7h3l2-2h6l2 2h3a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1zM12 16a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7z"/></svg>',
    vacuum: '<svg viewBox="0 0 24 24"><path d="M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 8a2 2 0 1 0 0 4 2 2 0 0 0 0-4zM6 16h12"/></svg>',
    tv: '<svg viewBox="0 0 24 24"><path d="M3 5h18v12H3zM8 21h8M9 17l-1 4M15 17l1 4"/></svg>',
    fridge: '<svg viewBox="0 0 24 24"><path d="M6 2h12v20H6zM6 10h12M9 5v2M9 13v3"/></svg>',
    washer: '<svg viewBox="0 0 24 24"><path d="M4 2h16v20H4zM4 7h16M12 18a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM7 4.5h2"/></svg>',
    ac: '<svg viewBox="0 0 24 24"><path d="M2 5h20v8H2zM6 10h12M7 17l-1 3M12 17v3M17 17l1 3"/></svg>',
    fan: '<svg viewBox="0 0 24 24"><path d="M12 12m-2 0a2 2 0 1 0 4 0a2 2 0 1 0-4 0M12 10c0-4 1-7 3-7s3 3 0 7M14 12c4 0 7 1 7 3s-3 3-7 0M12 14c0 4-1 7-3 7s-3-3 0-7M10 12c-4 0-7-1-7-3s3-3 7 0"/></svg>',
    room: '<svg viewBox="0 0 24 24"><path d="M4 21V8l8-5 8 5v13M9 21v-7h6v7"/></svg>',
    sip: '<svg viewBox="0 0 24 24"><path d="M6 3h12v18H6zM10 8h4M12 13a1.5 1.5 0 1 0 0 .01M9 18h6"/></svg>',
    other: '<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>',
  };

  const KINDS = [
    { id: "doorbell", name: "Doorbell & intercom", note: "Video door stations and indoor monitors", page: "doorbell", supported: true },
    { id: "security", name: "Security cameras", note: "Home and shop cameras", page: "cameras", supported: true },
    { id: "print", name: "Printers & scanners", note: "On the network and USB", page: "print", supported: true },
    { id: "fan", name: "Ceiling fans", note: "Over Bluetooth, and Wi-Fi", page: "fans", supported: true },
    { id: "camera", name: "Cameras", note: "DSLR and mirrorless", supported: false },
    { id: "vacuum", name: "Robot vacuums", supported: false },
    { id: "tv", name: "TVs", supported: false },
    { id: "fridge", name: "Fridges", supported: false },
    { id: "washer", name: "Washing machines", supported: false },
    { id: "ac", name: "Air conditioners", supported: false },
  ];

  // [id, name, logo file or "", colour, supported, words that identify it in a device's name]
  const B = (id, name, logo, color, supported = false, words = null) => ({
    id, name, logo: logo ? `brands/${logo}.svg` : "", color, supported,
    words: words || new RegExp(`\\b${name.toLowerCase().replace(/[^a-z0-9]+/g, "\\W*")}\\b`, "i"),
  });

  const BRANDS = {
    doorbell: [
      B("onetouch", "OneTouch", "", "#0b6fb8", true),
      B("sip", "Door station on this network", "", "#2f6f4e", true),
      B("akuvox", "Akuvox", "", "#e60012"),
      B("ring", "Ring", "ring", "#1c9ad6"),
      B("googlenest", "Google Nest", "googlenest", "#4285f4"),
      B("arlo", "Arlo", "arlo", "#49b48a"),
      B("eufy", "eufy", "", "#00a0e9"),
      B("hikvision", "Hikvision", "hikvision", "#e3001b"),
      B("dahua", "Dahua", "dahua", "#e6001f"),
      B("2n", "2N", "", "#1a1a1a"),
      B("commax", "Commax", "", "#003da5"),
      B("godrej", "Godrej", "", "#7a1f7a"),
    ],
    security: [
      B("tapo", "Tapo", "tapo", "#1a1a1a", true),
      B("xiaomi", "Xiaomi", "xiaomi", "#ff6900", true, /\b(xiaomi|mi home|mijia|imilab|xiaobai)\b/i),
      B("hikvision", "Hikvision", "hikvision", "#e3001b"),
      B("dahua", "Dahua", "dahua", "#e6001f"),
      B("cpplus", "CP Plus", "", "#e21e26"),
      B("ezviz", "EZVIZ", "", "#0a64d8"),
      B("imou", "Imou", "", "#f0731f"),
      B("xiaomi", "Xiaomi", "xiaomi", "#ff6900"),
      B("ring", "Ring", "ring", "#1c9ad6"),
      B("arlo", "Arlo", "arlo", "#49b48a"),
      B("googlenest", "Google Nest", "googlenest", "#4285f4"),
      B("eufy", "eufy", "", "#00a0e9"),
      B("reolink", "Reolink", "", "#1a6dd6"),
      B("wyze", "Wyze", "wyze", "#1df0bb"),
      B("blink", "Blink", "", "#2b8ce6"),
      B("ubiquiti", "Ubiquiti", "ubiquiti", "#0559c9"),
      B("tplink", "TP-Link VIGI", "tplink", "#4acbd6"),
    ],
    // Every printer that speaks IPP works without a driver, whatever its brand.
    print: [
      B("brother", "Brother", "brother", "#0d2ea0", true),
      B("hp", "HP", "hp", "#0096d6", true, /\b(hp|hewlett|laserjet|deskjet|officejet|envy|smart tank)\b/i),
      B("canon", "Canon", "canon", "#cc0000", true, /\b(canon|pixma|imageclass|i-sensys|maxify)\b/i),
      B("epson", "Epson", "epson", "#003399", true, /\b(epson|ecotank|workforce)\b/i),
      B("xerox", "Xerox", "xerox", "#d52b1e", true),
      B("ricoh", "Ricoh", "ricoh", "#cf142b", true),
      B("kyocera", "Kyocera", "kyocera", "#df0522", true, /\b(kyocera|ecosys|taskalfa)\b/i),
      B("lexmark", "Lexmark", "lexmark", "#00a650", true),
      B("samsung", "Samsung", "samsung", "#1428a0", true),
      B("sharp", "Sharp", "sharp", "#99cc00", true),
      B("konica", "Konica Minolta", "konicaminolta", "#0057a8", true, /\b(konica|minolta|bizhub)\b/i),
      B("pantum", "Pantum", "", "#d7000f", true),
      B("other", "Another printer", "", "#2f6f4e", true, /^$/),
    ],
    // Atomberg fans speak to Kagaz over Bluetooth from the day they are
    // unboxed, and over Wi-Fi once put on the network.
    fan: [
      B("atomberg", "Atomberg", "", "#e8541e", true),
      B("havells", "Havells", "", "#d71920"),
      B("orient", "Orient Electric", "", "#1b3f8f"),
      B("crompton", "Crompton", "", "#0072bc"),
      B("usha", "Usha", "", "#e4002b"),
      B("bajaj", "Bajaj", "", "#0057a8"),
      B("superfan", "Superfan", "", "#00a651"),
      B("bigassfans", "Big Ass Fans", "", "#ffcc00"),
      B("hunter", "Hunter", "", "#1a1a1a"),
    ],
    camera: [
      B("canon", "Canon", "canon", "#cc0000"),
      B("nikon", "Nikon", "nikon", "#1a1a1a"),
      B("sony", "Sony", "sony", "#1a1a1a"),
      B("fujifilm", "Fujifilm", "fujifilm", "#fb0020"),
      B("panasonic", "Panasonic", "", "#0049ab"),
      B("olympus", "OM System", "olympus", "#08107b"),
      B("leica", "Leica", "leica", "#e20612"),
      B("gopro", "GoPro", "gopro", "#1a1a1a"),
    ],
    vacuum: [
      B("roborock", "Roborock", "roborock", "#1a1a1a"),
      B("xiaomi", "Xiaomi", "xiaomi", "#ff6900"),
      B("irobot", "iRobot", "irobot", "#6cb86a"),
      B("ecovacs", "Ecovacs", "ecovacs", "#1e384b"),
      B("dreame", "Dreame", "", "#1a1a1a"),
      B("eufy", "eufy", "", "#00a0e9"),
      B("samsung", "Samsung", "samsung", "#1428a0"),
      B("dyson", "Dyson", "dyson", "#1a1a1a"),
    ],
    tv: [
      B("samsung", "Samsung", "samsung", "#1428a0"),
      B("lg", "LG", "lg", "#a50034"),
      B("sony", "Sony", "sony", "#1a1a1a"),
      B("tcl", "TCL", "tcl", "#e60012"),
      B("hisense", "Hisense", "hisense", "#00a9a6"),
      B("xiaomi", "Xiaomi", "xiaomi", "#ff6900"),
      B("philips", "Philips", "philips", "#0b5ed7"),
      B("panasonic", "Panasonic", "", "#0049ab"),
    ],
    fridge: [
      B("samsung", "Samsung", "samsung", "#1428a0"),
      B("lg", "LG", "lg", "#a50034"),
      B("whirlpool", "Whirlpool", "whirlpool", "#eeb111"),
      B("haier", "Haier", "haier", "#005bac"),
      B("bosch", "Bosch", "bosch", "#ea0016"),
      B("siemens", "Siemens", "siemens", "#009999"),
      B("godrej", "Godrej", "", "#7a1f7a"),
      B("panasonic", "Panasonic", "", "#0049ab"),
    ],
    washer: [
      B("samsung", "Samsung", "samsung", "#1428a0"),
      B("lg", "LG", "lg", "#a50034"),
      B("bosch", "Bosch", "bosch", "#ea0016"),
      B("whirlpool", "Whirlpool", "whirlpool", "#eeb111"),
      B("haier", "Haier", "haier", "#005bac"),
      B("siemens", "Siemens", "siemens", "#009999"),
      B("ifb", "IFB", "", "#e31e24"),
      B("panasonic", "Panasonic", "", "#0049ab"),
    ],
    ac: [
      B("daikin", "Daikin", "", "#0097e0"),
      B("lg", "LG", "lg", "#a50034"),
      B("samsung", "Samsung", "samsung", "#1428a0"),
      B("panasonic", "Panasonic", "", "#0049ab"),
      B("haier", "Haier", "haier", "#005bac"),
      B("voltas", "Voltas", "", "#0b4ea2"),
      B("bluestar", "Blue Star", "", "#0055a5"),
      B("mitsubishi", "Mitsubishi Electric", "", "#e60012"),
    ],
  };

  function list(kind) {
    return BRANDS[kind] || [];
  }
  function byId(kind, id) {
    return list(kind).find((b) => b.id === id) || null;
  }
  function kind(id) {
    return KINDS.find((k) => k.id === id) || null;
  }
  // The brand a device's name and maker belong to.
  function match(kindId, text) {
    return list(kindId).find((b) => b.id !== "other" && b.words.test(text || "")) || null;
  }

  // The brand's mark: its logo, else its name in its colour.
  function mark(b) {
    const box = document.createElement("span");
    box.className = "brand-logo";
    if (b.logo) {
      const img = document.createElement("img");
      img.src = b.logo;
      img.alt = "";
      img.loading = "lazy";
      box.appendChild(img);
    } else if (ICON[b.id]) {
      box.classList.add("brand-icon");
      box.style.color = b.color;
      box.innerHTML = ICON[b.id];
    } else {
      const w = document.createElement("span");
      w.className = "wordmark";
      w.style.color = b.color;
      w.textContent = b.name;
      box.appendChild(w);
    }
    return box;
  }

  // Fill `container` with the brands of `kindId`; `onPick(brand)` for each tap.
  function fill(container, kindId, onPick) {
    container.innerHTML = "";
    for (const b of list(kindId)) {
      const t = document.createElement("button");
      t.type = "button";
      t.className = "brand-tile" + (b.supported ? "" : " soon");
      t.title = b.supported ? b.name : `${b.name}: coming soon`;
      t.appendChild(mark(b));
      const name = document.createElement("span");
      name.className = "brand-name";
      name.textContent = b.name;
      t.appendChild(name);
      if (!b.supported) {
        const soon = document.createElement("span");
        soon.className = "soon-badge";
        soon.textContent = "Coming soon";
        t.appendChild(soon);
      }
      t.addEventListener("click", () => onPick(b));
      container.appendChild(t);
    }
  }

  // Fill `container` with the kinds; `onPick(kind)` for each tap.
  function fillKinds(container, onPick) {
    container.innerHTML = "";
    for (const k of KINDS) {
      const t = document.createElement("button");
      t.type = "button";
      t.className = "kind-tile" + (k.supported ? "" : " soon");
      t.innerHTML = `<span class="kind-icon">${ICON[k.id]}</span><span class="kind-name"></span><span class="kind-note muted"></span>`;
      t.querySelector(".kind-name").textContent = k.name;
      t.querySelector(".kind-note").textContent = k.supported ? k.note || "" : "Coming soon";
      t.addEventListener("click", () => onPick(k));
      container.appendChild(t);
    }
  }

  // An Atomberg series code's product name ("R1" is a Renesa).
  const FAN_PRODUCTS = {
    R1: "Renesa", R2: "Renesa+", R3: "Renesa Smart+", S1: "Studio+", S2: "Renesa Elite",
    I1: "Aris", I2: "Aris Starlight", I3: "Aris Star", I4: "Aris", I5: "Aris Contours",
    M1: "Efficio", M2: "Efficio+", K1: "Gorilla",
  };
  const FAN_FAMILIES = { R: "Renesa", S: "Studio", I: "Aris", M: "Efficio", K: "Gorilla" };
  function fanProduct(series) {
    const s = (series || "").toUpperCase();
    return FAN_PRODUCTS[s] || FAN_FAMILIES[s[0]] || "Atomberg fan";
  }

  return { KINDS, ICON, list, byId, kind, match, mark, fill, fillKinds, fanProduct };
})();
