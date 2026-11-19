// TVs & displays: the TVs the person added and the ones found nearby, and
// every way this device can show its screen on one: Kagaz's own sender
// (Miracast, over the network, or Wi-Fi Direct from Linux), a second
// screen (Linux), and the system's own (Windows' Cast, the phone's Cast
// screen, Samsung DeX, AirPlay on a Mac). Nothing reaches a TV until the
// person presses a button here, and the system's own prompt (the
// desktop's sharing dialog, Android's) comes first.
//
// Layout: the TVs first, each a card with its state and one main button
// (mirror, or stop while mirroring); the other ways in a sheet; news in a
// snackbar at the bottom, a failure inside the card it belongs to. A
// card's state is a word or two on one line and changes only when the
// state does: the session's running numbers (pictures a second, the
// stream's speed) are in the TV's sheet, so nothing on the page moves
// while mirroring.
//
// A phone's session has switches, for privacy: the screen hidden (the TV
// gets a blur) and the sound's place (the TV or the phone). Every session
// starts hidden. Under the mirroring TV's card: Show everything (or Hide
// everything), and a chip each for the screen and the sound. The same
// switches are in the phone's notification, quick panel and floating
// button; a change made there shows here.
// Uses app.js's globals: $, invoke, listen; ask.js's KagazAsk; icons.js's
// KagazIcons.

const KagazTvs = (() => {
  const OS = window.__KAGAZ_OS__ || "gnome";
  const PHONE = OS === "android" || OS === "ios";
  const SAMSUNG = /SM-|Samsung/i.test(navigator.userAgent);
  const DEVICE = PHONE ? "phone" : "computer";
  let saved = [];
  let found = [];
  let scanning = false;
  let lastScan = 0;
  let showing = null; // the TV this screen is on now
  let playing = false; // its picture is on the TV (before that: connecting)
  let line = ""; // the session's latest news, in full, for the TV's sheet
  let sheetLine = null; // where an open sheet shows it
  let sw = null; // a phone session's switches: { live, screenHidden, soundOnTv, sound }
  let failed = {}; // TV name -> why its last try failed

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function button(cls, text, onClick, label) {
    // Text, or nodes (an icon and its word).
    const b = el("button", cls, typeof text === "string" ? text : null);
    if (text && typeof text !== "string") b.append(text);
    b.type = "button";
    if (label) b.setAttribute("aria-label", label);
    b.addEventListener("click", onClick);
    return b;
  }

  const icon = (name) => {
    const i = el("span", "ic");
    i.innerHTML = KagazIcons.svg(name);
    return i;
  };
  const withIcon = (name, word) => {
    const b = document.createDocumentFragment();
    b.append(icon(name), el("span", "", word));
    return b;
  };

  // The snackbar: a short line at the bottom that goes by itself.
  let toastTimer = null;
  function toast(text, long) {
    const t = $("tvs-toast");
    t.textContent = text;
    t.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => t.classList.add("hidden"), long ? 8000 : 4000);
  }

  // For the Home tile.
  function homeWords(t) {
    if (showing === t.name) return `Showing this ${DEVICE}`;
    const here = found.find((f) => f.name === t.name);
    return here ? (here.mirror ? "Ready to mirror" : "On the network") : t.model || "TV";
  }

  // The TV's state in a few words, and how it should look.
  function state(t) {
    if (showing === t.name) {
      if (!playing) return { text: "Connecting…", tone: "on" };
      return { text: sw && sw.screenHidden ? "Hidden" : "Mirroring", tone: "on" };
    }
    if (failed[t.name]) return { text: "Did not connect", tone: "bad" };
    const here = found.find((f) => f.name === t.name);
    if (here && here.mirror) return { text: "Ready", tone: "ok" };
    if (here) return { text: "On the network, not taking mirroring now", tone: "" };
    return { text: lastScan ? "Not found on this network" : "Looking…", tone: "" };
  }

  // Kagaz's own: this device's screen on the TV.
  async function show(t, what) {
    if (showing) return;
    delete failed[t.name];
    try {
      if (OS === "android") {
        await invoke("tv_phone", { what: "consent" });
      } else if (OS === "gnome" && what !== "extend") {
        toast("Choose what to share in the desktop's dialog.");
      }
      await invoke("tv_show", { name: t.name, what });
      showing = t.name;
      playing = false;
      line = "";
    } catch (e) {
      failed[t.name] = String(e);
    }
    render();
  }

  // The system's own way.
  async function system(what) {
    try {
      const said = PHONE ? await invoke("tv_phone", { what }) : await invoke("tv_system_sender");
      toast(said, true);
    } catch (e) {
      toast(String(e), true);
    }
  }

  // A TV's sheet (a bottom sheet on a phone, a dialog on a computer, as
  // for every simple device): Material's list rows, each an icon, a name,
  // one short line and what tapping does (an arrow opens something, a
  // switch turns something on); the TV itself last. On a phone it closes
  // with a drag down, a tap above it or Back (ask.js), so it has no Close.
  function sheet(t) {
    const ways = [];
    if (PHONE) {
      if (OS === "ios") ways.push({ icon: "cast", name: "Screen Mirroring", what: "The iPhone's own, for TVs with AirPlay", go: () => system("systemCast") });
      else ways.push({ icon: "cast", name: "Android's Cast", what: "The phone's own, with every app's sound", go: () => system("systemCast") });
      if (SAMSUNG) ways.push({ icon: "computer", name: "Samsung DeX", what: "A separate desktop on the TV", go: () => system("dex") });
    } else {
      if (OS === "gnome") {
        ways.push({ icon: "picture", name: "Mirror one window", what: "Only the window you choose", go: () => show(t, "window"), disabled: !!showing });
        ways.push({ icon: "screens", name: "Second screen", what: "The TV as a new monitor", go: () => show(t, "extend"), disabled: !!showing });
      }
      const own = {
        windows: ["Windows' Cast", "Win+K; also reaches TVs on no network"],
        macos: ["AirPlay", "The Mac's own, for TVs with AirPlay"],
        gnome: ["GNOME Network Displays", "The system's own sender, if installed"],
      }[OS] || ["The system's own sender", ""];
      ways.push({ icon: "cast", name: own[0], what: own[1], go: () => system("system") });
    }
    KagazAsk.ask((dlg, finish) => {
      dlg.classList.add("sheet", "tv-sheet");
      dlg.appendChild(el("h3", "ask-title", t.name));
      const st = state(t);
      const model = t.model && !t.name.includes(t.model) ? t.model : "";
      dlg.appendChild(el("p", "tv-sheet-state muted small", [model, st.text].filter(Boolean).join(" · ")));
      // The session's news as it comes: here there is room for it.
      sheetLine = el("p", "tv-sheet-line small", showing === t.name ? line : "");
      sheetLine.setAttribute("aria-live", "polite");
      sheetLine.classList.toggle("hidden", showing !== t.name || !line);
      dlg.appendChild(sheetLine);
      if (failed[t.name]) dlg.appendChild(el("p", "cap-warn small", failed[t.name]));
      const list = el("div", "tv-ways");
      const section = (text) => list.appendChild(el("div", "tv-way-section", text));
      // One row: icon, name and line, then the row's end (an arrow, a
      // switch, a chevron).
      const row = (tag, cls, c, end) => {
        const r = el(tag, "tv-way" + (cls ? " " + cls : ""));
        const words = el("span", "tv-way-words");
        words.appendChild(el("span", "tv-way-name", c.name));
        if (c.what) words.appendChild(el("span", "tv-way-what", c.what));
        r.append(icon(c.icon), words);
        if (end) r.appendChild(end);
        list.appendChild(r);
        return r;
      };
      const opens = (c) => {
        const b = row("button", c.danger ? "danger-text" : "", c, c.danger ? null : icon("forward"));
        b.type = "button";
        b.disabled = !!c.disabled;
        b.addEventListener("click", () => {
          finish(null);
          c.go();
        });
        return b;
      };
      section("Other ways");
      ways.forEach(opens);
      if (OS === "android" && sw) {
        // Where a mirroring session's switches can also be reached.
        section("While mirroring");
        const box = el("input", "tv-switch");
        box.type = "checkbox";
        box.setAttribute("role", "switch");
        box.checked = !!sw.floating;
        box.addEventListener("change", async () => {
          const wanted = box.checked;
          try {
            sw = Object.assign({}, sw, await invoke("tv_control", { what: "float", on: wanted }));
          } catch (e) {
            toast(String(e));
          }
          // Android's own screen may be asking first; the switch shows
          // on once that is allowed.
          box.checked = !!sw.floating;
          if (wanted && !sw.floating) toast("Allow Kagaz to appear on top in Android's screen, then turn this on again.", true);
        });
        row("label", "", { icon: "floating", name: "Floating button", what: "Controls over every app" }, box);
        // The quick panel's tiles: Android's own offer where it has one
        // (13 and later), else the steps, in this phone's words, folded
        // away until asked for.
        const tiles = { icon: "grid", name: "Quick panel buttons", what: "Show, Screen and Sound on TV" };
        if (sw.tiles) {
          const b = row("button", "", tiles, icon("plus"));
          b.type = "button";
          b.addEventListener("click", () => invoke("tv_control", { what: "tiles", on: true }).catch((e) => toast(String(e))));
        } else {
          const fold = icon("down");
          fold.classList.add("tv-fold");
          const b = row("button", "", Object.assign(tiles, { what: "How to add them" }), fold);
          b.type = "button";
          b.setAttribute("aria-expanded", "false");
          const steps = SAMSUNG
            ? ["Swipe down twice from the top.", "Tap ⋮, then Edit buttons.", "Find Show on TV, Screen on TV and Sound on TV at the top.", "Drag each one down, then tap Done."]
            : ["Swipe down twice from the top.", "Tap the pencil.", "Find Show on TV, Screen on TV and Sound on TV.", "Drag each one up among your tiles."];
          const ol = el("ol", "tv-steps small hidden");
          steps.forEach((x) => ol.appendChild(el("li", "", x)));
          list.appendChild(ol);
          b.addEventListener("click", () => {
            const open = ol.classList.toggle("hidden") === false;
            b.setAttribute("aria-expanded", String(open));
          });
        }
      }
      if (!PHONE) {
        // The session's shortcuts (tvs.rs: the desktop's shortcuts portal
        // on Linux, which asks once; Windows and macOS directly).
        section("While mirroring");
        const mac = OS === "macos";
        const keys = (k) => (mac ? "⌃⌥" + k : "Ctrl+Alt+" + k);
        row("div", "tv-way-info", { icon: "keyboard", name: keys("H"), what: "Hide or show everything" });
        row("div", "tv-way-info", { icon: "keyboard", name: keys("S"), what: "Sound on the TV or on this computer" });
      }
      list.appendChild(el("div", "tv-way-rule"));
      opens({ icon: "bin", name: "Remove this TV", danger: true, go: () => remove(t) });
      dlg.appendChild(list);
      if (!PHONE) {
        const actions = el("div", "actions");
        actions.appendChild(button("", "Close", () => finish(null)));
        dlg.appendChild(actions);
      }
    });
  }

  async function remove(t) {
    if (!(await KagazAsk.confirm(`Remove ${t.name} from your TVs?`, "Remove TV", "The TV stays as it is; a search finds it again."))) return;
    try {
      saved = await invoke("tv_forget", { name: t.name });
    } catch (e) {
      toast("Could not remove it: " + e);
    }
    render();
    if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
  }

  // A TV as Home's device tile: tap it for its sheet; the tile's own
  // button mirrors (Stop while mirroring), as Connect does on Computers &
  // phones.
  function tile(t) {
    const st = state(t);
    const live = showing === t.name;
    const tile = el("div", "home-tile has-actions" + (live ? " on live" : st.tone === "bad" ? " bad" : ""));
    const main = button("tile-main", null, () => sheet(t), `${t.name}: ${st.text}`);
    const ic = el("span", "tile-icon");
    ic.innerHTML = KagazIcons.svg("tv");
    const text = el("span", "tile-text");
    const sub = el("span", "tile-sub");
    sub.append(el("span", "rc-dot" + (st.tone === "ok" || live ? " ok" : "")), st.text);
    if (live) sub.setAttribute("aria-live", "polite");
    text.append(el("span", "tile-name", t.name), sub);
    const chev = el("span", "home-chevron rc-chev");
    chev.innerHTML = KagazIcons.svg("forward");
    main.append(ic, text, chev);
    const acts = el("span", "tile-actions");
    const go = live
      ? button("tile-act", withIcon("pause", "Stop"), () => invoke("tv_stop").catch(() => {}), `Stop mirroring to ${t.name}`)
      : button("tile-act", withIcon("tv", "Mirror"), () => show(t, "screen"), `Mirror this ${DEVICE} to ${t.name}`);
    if (!live) go.disabled = !!showing;
    acts.append(go);
    tile.append(main, acts);
    return tile;
  }

  // Turns a switch, and draws how they stand after.
  async function turn(what, on) {
    try {
      sw = (await invoke("tv_control", { what, on })) || sw;
    } catch (e) {
      toast(String(e));
    }
    render();
  }

  // The mirroring TV's switches, under its card: the one button that
  // shows (or hides) everything, then a chip each for the screen and the
  // sound, checked while that goes to the TV.
  function switches() {
    const row = el("div", "tv-switches");
    // Sound counts only where it can go to the TV at all.
    const hiddenSome = sw.screenHidden || (sw.sound && !sw.soundOnTv);
    row.append(
      hiddenSome
        ? button("tv-all show", withIcon("eye", "Show everything"), () => turn("all", true), "Show the screen and play the sound on the TV")
        : button("tv-all", withIcon("lock", "Hide everything"), () => turn("all", false), `Hide the screen and keep the sound on this ${DEVICE}`)
    );
    const chip = (word, on, what, label) => {
      const c = button("tv-chip" + (on ? " on" : ""), on ? withIcon("check", word) : word, () => turn(what, !on), label);
      c.setAttribute("role", "switch");
      c.setAttribute("aria-checked", String(on));
      return c;
    };
    row.append(chip("Screen on TV", !sw.screenHidden, "screen", "The screen shows on the TV"));
    if (sw.sound) row.append(chip("Sound on TV", sw.soundOnTv, "sound", "The sound plays on the TV"));
    // An app that forbids taking its sound: said, not left a puzzle.
    if (sw.soundOnTv && sw.soundKept) row.append(el("p", "muted small tv-kept", "The app playing now keeps its sound on the phone (it does not let other apps send it)."));
    return row;
  }

  // A TV found nearby, as Computers & phones lists them.
  function foundRow(f) {
    const li = el("li", "rc-row");
    const ic = el("span", "tile-icon");
    ic.innerHTML = KagazIcons.svg("tv");
    const text = el("span", "tile-text");
    text.append(el("span", "tile-name", f.name), el("span", "tile-sub", [f.model, f.mirror ? "takes screen mirroring" : f.ways.join(", ")].filter(Boolean).join(" · ")));
    const add = button("", "Add", async () => {
      add.disabled = true;
      try {
        saved = await invoke("tv_add", { name: f.name, model: f.model, address: f.address });
        toast(`${f.name} is in your TVs.`);
      } catch (e) {
        toast("Could not add it: " + e);
      }
      render();
      if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
    }, `Add ${f.name}`);
    li.append(ic, text, add);
    return li;
  }

  function render() {
    $("tvs-tiles").replaceChildren(...saved.flatMap((t) => (showing === t.name && playing && sw && sw.live ? [tile(t), switches()] : [tile(t)])));
    $("tvs-tiles").classList.toggle("hidden", saved.length === 0);
    $("tvs-empty").classList.toggle("hidden", saved.length > 0);
    // This computer itself, listed as a screen, is not one to show on.
    const self = (f) => here && here.on && f.model === "Kagaz" && f.name === here.name;
    const others = found.filter((f) => !f.added && !saved.some((s) => s.name === f.name) && !self(f));
    $("tvs-found").replaceChildren(...others.map(foundRow));
    $("tvs-found-head").classList.toggle("hidden", others.length === 0);
  }

  async function rescan() {
    if (scanning) return;
    scanning = true;
    // The bar's refresh turns while looking: the one sign of it.
    const b = $("bar-refresh");
    b.disabled = true;
    b.classList.add("working");
    try {
      found = await invoke("tvs_search");
      saved = await invoke("tvs_saved");
      lastScan = Date.now();
      if (!found.length) toast("No TV answered on this network. Is it on, and on this Wi-Fi?", true);
    } catch (e) {
      toast("Could not search: " + e, true);
    }
    b.classList.remove("working");
    b.disabled = false;
    scanning = false;
    render();
    if (typeof KagazHome !== "undefined") KagazHome.devicesChanged();
  }

  async function showPage() {
    try {
      saved = (await invoke("tvs_saved")) || saved;
      showing = (await invoke("tv_showing")) || showing;
      sw = await invoke("tv_controls").catch(() => null);
    } catch (e) {
      toast("Could not read your TVs: " + e);
    }
    render();
    if (Date.now() - lastScan > 2 * 60 * 1000) rescan();
  }

  // A session's news: the card says connecting, then mirroring; the
  // running numbers go to the TV's sheet; the end in a snackbar, a
  // failure in the card. The page is drawn again only when what a card
  // says changes, not with every second's numbers.
  listen("tv-session", (e) => {
    const n = e.payload || {};
    const before = [showing, playing, failed[n.tv]].join("|");
    if (n.phase === "ended") {
      showing = null;
      playing = false;
      line = "";
      toast(n.line);
    } else if (n.phase === "failed") {
      showing = null;
      playing = false;
      line = "";
      failed[n.tv] = n.line;
    } else {
      showing = n.tv;
      playing = playing || n.phase === "playing";
      const m = /(\d+) s, (\d+) pictures, (\d+) kbit\/s; the screen gave (\d+)\/s/.exec(n.line);
      line = m ? `${m[4]} pictures a second · ${(m[3] / 1000).toFixed(1)} Mbit/s` : String(n.line || "").trim();
    }
    if (sheetLine && sheetLine.isConnected) {
      sheetLine.textContent = line;
      sheetLine.classList.toggle("hidden", !line);
    }
    if ([showing, playing, failed[n.tv]].join("|") !== before) render();
  });

  // The switches changed, here or outside the app.
  listen("tv-controls", (e) => {
    sw = Object.assign({}, sw, e.payload);
    render();
  });

  // This computer as a screen (here.rs): a row as "This computer's
  // screens" has, below the TVs; off until turned on. Computers only.
  let here = null;
  function hereSection() {
    if (PHONE) return;
    let sec = document.getElementById("tvs-here");
    if (!sec) {
      sec = el("section", "set-section here-section");
      sec.id = "tvs-here";
      document.querySelector("#tvs .tv-about").before(sec);
    }
    const st = here || { on: false, name: "" };
    const r = el("div", "set-row");
    const l = el("label", "set-label");
    l.htmlFor = "here-on";
    l.append(
      el("span", "", "Let phones and computers show their screen here"),
      el("span", "muted small", st.on ? `Listed as “${st.name}” on this network. Each one is asked about first.` : "This computer is listed as a screen on this network while this is on. Each one is asked about first.")
    );
    const c = el("span", "set-control");
    const box = el("input", "tv-switch");
    box.type = "checkbox";
    box.setAttribute("role", "switch");
    box.id = "here-on";
    box.checked = !!st.on;
    box.addEventListener("change", async () => {
      try {
        here = await invoke("here_set", { on: box.checked });
      } catch (e) {
        toast(String(e), true);
      }
      hereSection();
    });
    c.append(box);
    r.append(l, c);
    const parts = [el("h3", "", "This computer as a screen"), r];
    if (st.problem) parts.push(el("p", "cap-warn small", st.problem));
    if (st.showing) parts.push(el("p", "muted small", `${st.showing}'s screen is showing in its own window.`));
    sec.replaceChildren(...parts);
  }
  if (!PHONE) {
    invoke("here_status")
      .then((st) => {
        here = st;
        hereSection();
      })
      .catch(() => {});
    listen("here", (e) => {
      here = e.payload;
      hereSection();
      render();
    });
  }

  $("tvs").addEventListener("refresh", rescan);

  return { show: showPage, rescan, homeWords, saved: () => invoke("tvs_saved").catch(() => []) };
})();
