// The strip on top while this computer is on a TV (tvs.rs opens it). The
// icons stay the same; a line through one means that part is not on the
// TV. Its switches are the TV card's, the tray's and the shortcuts'.
(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const $ = (id) => document.getElementById(id);
  let sw = null;

  function show(s) {
    if (!s) return;
    sw = s;
    const anything = !s.screenHidden || (s.sound && s.soundOnTv);
    $("strip-state").textContent = s.screenHidden ? "Hidden" : "Mirroring";
    $("strip-all").textContent = anything ? "Hide everything" : "Show everything";
    $("strip-all").classList.toggle("show", !anything);
    const screen = $("strip-screen");
    screen.classList.toggle("off", s.screenHidden);
    screen.setAttribute("aria-pressed", String(!s.screenHidden));
    screen.title = s.screenHidden ? "The TV shows frosted glass. Show the screen (Ctrl+Alt+H shows everything)" : "The screen is on the TV. Hide it";
    screen.setAttribute("aria-label", screen.title);
    const sound = $("strip-sound");
    sound.classList.toggle("hidden", !s.sound);
    sound.classList.toggle("off", !s.soundOnTv);
    sound.setAttribute("aria-pressed", String(!!s.soundOnTv));
    sound.title = s.soundOnTv ? "The sound plays on the TV. Play it here (Ctrl+Alt+S)" : "The sound plays here. Play it on the TV (Ctrl+Alt+S)";
    sound.setAttribute("aria-label", sound.title);
  }

  const turn = (what, on) => invoke("tv_control", { what, on }).then(show).catch(() => {});
  $("strip-all").addEventListener("click", () => sw && turn("all", !(!sw.screenHidden || (sw.sound && sw.soundOnTv))));
  $("strip-screen").addEventListener("click", () => sw && turn("screen", sw.screenHidden));
  $("strip-sound").addEventListener("click", () => sw && turn("sound", !sw.soundOnTv));
  $("strip-stop").addEventListener("click", () => invoke("tv_stop"));

  invoke("tv_controls").then(show).catch(() => {});
  invoke("tv_showing").then((n) => { if (n) $("strip-state").title = `On ${n}`; }).catch(() => {});
  listen("tv-controls", (e) => show(Object.assign({}, sw, e.payload)));
  if (typeof KagazIcons !== "undefined") KagazIcons.fill(document);
})();
