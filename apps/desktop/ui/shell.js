// The frame around every page: the navigation, the page's bar and the ring
// banner. What it looks like is each system's own (skins.css); this file
// decides which navigation the window has room for, keeps the bar's title
// and Back in step with where the person is, and shows a ring anywhere.
//
// Layouts, by window width at each system's own breakpoints:
//   android  < 600 bottom bar · 600–1199 navigation rail · 1200+ expanded rail
//            (a phone on its side, under 500 tall, has the rail too)
//   ios      < 600 tab bar · 600+ sidebar (as on an iPad)
//   windows  ≤ 640 pane hidden behind the menu button · 641–1007 icons only
//            · 1008+ the pane with names (its menu button folds it)
//   gnome    < 550 bottom bar · 550+ sidebar
//   macos    sidebar
//
// The list itself, wherever it has room for more than a phone's bar: Home;
// then the kinds of device the person has added, by name, and only those
// (Home says which, home.js), with Add a device at their end, behind
// which every kind is; Tools and Settings at the foot. A kind's page that
// is open has its entry even before its first device, so that the list
// always says where the person is.
// Uses app.js's selectCategory; nav.js's KagazNav; home.js's KagazHome.

const KagazShell = (() => {
  const $ = (id) => document.getElementById(id);
  const root = document.documentElement;
  const os = root.dataset.os || "gnome";

  // The page's name in the bar, for each place.
  const TITLES = {
    home: "Home", add: "Add a device", settings: "Settings", tools: "Tools",
    doorbell: "Doorbell", cameras: "Security cameras", print: "Printers & scanners",
    fans: "Ceiling fans", purifiers: "Air purifiers", society: "Society gate",
    remote: "Computers & phones",
  };
  // Places the navigation itself reaches: there, Back has nowhere to go
  // unless something is open inside the page.
  const PHONE_ROOTS = new Set(["home", "tools", "settings"]);
  const ALL_ROOTS = new Set(["home", "tools", "settings", "doorbell", "cameras", "print", "fans", "purifiers", "society", "remote"]);

  let view = "home";
  let layout = "";

  // ---------- which navigation fits ----------
  const PANE_KEY = "kagaz.shell.pane"; // Windows: the person folded the pane
  let paneFolded = false;
  try {
    paneFolded = localStorage.getItem(PANE_KEY) === "folded";
  } catch (e) {}

  function pickLayout() {
    const w = window.innerWidth;
    const h = window.innerHeight;
    switch (os) {
      case "android":
        if (w < 600 && !(h < 500 && w > h)) return "bar";
        return w >= 1200 ? "sidebar" : "rail";
      case "ios":
        return w < 600 ? "bar" : "sidebar";
      case "windows":
        if (w <= 640) return "minimal";
        if (w < 1008) return "rail";
        return paneFolded ? "rail" : "sidebar";
      case "macos":
        return "sidebar";
      default:
        return w < 550 ? "bar" : "sidebar";
    }
  }

  function applyLayout() {
    const next = pickLayout();
    if (next !== layout) {
      layout = next;
      root.dataset.layout = layout;
      closeOverlay();
    }
    $("pane-toggle").setAttribute("aria-expanded", String(layout === "sidebar"));
    updateBar();
  }

  // Windows: the menu button folds the pane to its icons (remembered), or,
  // where the window is too narrow for the names, lays the whole pane over
  // the page until a place is chosen.
  function closeOverlay() {
    root.classList.remove("pane-over");
  }
  $("pane-toggle").addEventListener("click", () => {
    if (os === "windows" && window.innerWidth >= 1008) {
      paneFolded = !paneFolded;
      try {
        localStorage.setItem(PANE_KEY, paneFolded ? "folded" : "open");
      } catch (e) {}
      applyLayout();
    } else {
      root.classList.toggle("pane-over");
    }
  });
  $("bar-menu").addEventListener("click", () => root.classList.add("pane-over"));
  document.addEventListener("click", (ev) => {
    if (!root.classList.contains("pane-over")) return;
    if (ev.target.closest("#categories li[data-cat]") || !ev.target.closest("#sidebar, #bar-menu, #pane-toggle")) closeOverlay();
  });
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") closeOverlay();
  });

  // ---------- the person's kinds ----------
  // Those known when Kagaz was last open show at once; Home's word on
  // them follows in a moment.
  const KINDS_KEY = "kagaz.shell.kinds";
  let added = new Set();
  try {
    added = new Set(JSON.parse(localStorage.getItem(KINDS_KEY) || "[]"));
  } catch (e) {}
  const kindEntries = () => [...document.querySelectorAll("#cat-kinds li[data-cat]:not(.cat-add)")];
  // By the name each shows, whatever order the page lists them in.
  {
    const name = (li) => li.querySelector(".cat-name").textContent;
    const add = document.querySelector("#cat-kinds .cat-add");
    for (const li of kindEntries().sort((a, b) => name(a).localeCompare(name(b), undefined, { sensitivity: "base" }))) add.before(li);
  }
  function showKinds() {
    let any = false;
    for (const li of kindEntries()) {
      const on = added.has(li.dataset.cat) || li.dataset.cat === view;
      li.classList.toggle("hidden", !on);
      any = any || on;
    }
    $("cat-head").classList.toggle("hidden", !any);
  }
  function kinds(ids) {
    added = new Set(ids);
    try {
      localStorage.setItem(KINDS_KEY, JSON.stringify(ids));
    } catch (e) {}
    showKinds();
  }
  // Ask Home which kinds there are now (it draws itself when it is the
  // page on screen, and says so then).
  function askKinds() {
    if (view !== "home" && typeof KagazHome !== "undefined") KagazHome.kinds();
  }

  // ---------- the bar ----------
  function updateBar() {
    const levels = KagazNav.depth();
    const roots = layout === "bar" ? PHONE_ROOTS : ALL_ROOTS;
    const canBack = !roots.has(view) || levels > 1;
    const back = $("bar-back");
    back.classList.toggle("hidden", !canBack);
    // iOS names where Back goes; the others show the arrow alone.
    back.querySelector(".bar-back-word").textContent = levels > 1 && TITLES[view] && view !== "add" ? shortTitle(view) : "Home";
    back.setAttribute("aria-label", "Back");
    // A page can name a step inside it (data-title on its section).
    const pane = $(view);
    const step = pane && pane.dataset.title;
    $("bar-title").textContent = view === "add" ? $("add-title").textContent || TITLES.add : step || TITLES[view] || "Kagaz";
    $("bar-add").classList.toggle("hidden", view !== "home");
    $("bar-menu").classList.toggle("hidden", layout !== "minimal");
    root.dataset.view = view;
  }
  const SHORT = { cameras: "Cameras", print: "Print & scan", fans: "Fans", purifiers: "Purifiers", remote: "Computers" };
  const shortTitle = (v) => SHORT[v] || TITLES[v];

  $("bar-back").addEventListener("click", () => {
    if (KagazNav.depth() > 0) KagazNav.back();
    else selectCategory("home");
  });
  $("bar-add").addEventListener("click", () => KagazHome.openAdd());
  KagazNav.watch(() => updateBar());
  // Add a device names its step (a kind's name once one is chosen).
  new MutationObserver(() => view === "add" && updateBar()).observe($("add-title"), { childList: true, characterData: true, subtree: true });

  // A place was chosen (app.js's selectCategory tells the shell).
  function place(cat) {
    view = cat;
    closeOverlay();
    updateBar();
    showRing();
    // What was added on the page just left has its entry from now on.
    showKinds();
    askKinds();
  }

  // ---------- a ring, wherever the person is ----------
  // Each source (the cloud call line, the stations on this network) says
  // when it rings and how to answer; the banner shows the newest, except on
  // the Doorbell page, which shows the call itself.
  const rings = new Map(); // source -> { title, sub, answer, decline }
  function ring(source, info) {
    if (info) rings.set(source, info);
    else rings.delete(source);
    showRing();
  }
  function showRing() {
    const box = $("urgent");
    const last = [...rings.values()].pop();
    const shown = !!last && view !== "doorbell";
    box.classList.toggle("hidden", !shown);
    if (!shown) return;
    $("urgent-title").textContent = last.title;
    $("urgent-sub").textContent = last.sub || "";
    $("urgent-answer").onclick = last.answer;
    $("urgent-decline").onclick = last.decline;
  }

  window.addEventListener("resize", applyLayout);
  // app.js chose the first place before this file loaded.
  if (typeof currentView !== "undefined" && currentView) view = currentView;
  applyLayout();
  showKinds();
  if (typeof KagazHome !== "undefined") KagazHome.kinds();

  return { place, ring, kinds, layout: () => layout };
})();
