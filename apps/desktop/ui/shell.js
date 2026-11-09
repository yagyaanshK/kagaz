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
// Uses app.js's selectCategory; nav.js's KagazNav.

const KagazShell = (() => {
  const $ = (id) => document.getElementById(id);
  const root = document.documentElement;
  const os = root.dataset.os || "gnome";

  // The page's name in the bar, for each place.
  const TITLES = {
    home: "Home", add: "Add a device", settings: "Settings", tools: "Tools",
    doorbell: "Doorbell", cameras: "Security cameras", print: "Printers & scanners",
    fans: "Ceiling fans", purifiers: "Air purifiers", society: "Society gate",
  };
  // Places the navigation itself reaches: there, Back has nowhere to go
  // unless something is open inside the page.
  const PHONE_ROOTS = new Set(["home", "tools", "settings"]);
  const ALL_ROOTS = new Set(["home", "tools", "settings", "doorbell", "cameras", "print", "fans", "purifiers", "society"]);

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
    $("bar-title").textContent = view === "add" ? $("add-title").textContent || TITLES.add : TITLES[view] || "Kagaz";
    $("bar-add").classList.toggle("hidden", view !== "home");
    $("bar-menu").classList.toggle("hidden", layout !== "minimal");
    root.dataset.view = view;
  }
  const SHORT = { cameras: "Cameras", print: "Print & scan", fans: "Fans", purifiers: "Purifiers" };
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

  return { place, ring, layout: () => layout };
})();
