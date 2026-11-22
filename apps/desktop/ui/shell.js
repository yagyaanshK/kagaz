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
    fans: "Fans", purifiers: "Air purifiers", tvs: "TVs & displays", society: "Society gate",
    remote: "Computers & phones", nearby: "On your network",
  };
  // Places the navigation itself reaches: there, Back has nowhere to go
  // unless something is open inside the page.
  const PHONE_ROOTS = new Set(["home", "tools", "settings"]);
  const ALL_ROOTS = new Set(["home", "tools", "settings", "doorbell", "cameras", "print", "fans", "purifiers", "tvs", "society", "remote"]);

  let view = "home";
  let layout = "";

  // ---------- which navigation fits ----------
  const PANE_KEY = "kagaz.shell.pane"; // Windows: the person folded the pane
  let paneFolded = false;
  try {
    paneFolded = localStorage.getItem(PANE_KEY) === "folded";
  } catch (e) {}

  function pickLayout() {
    // On a phone, the screen's own width: innerWidth grows when the page is
    // zoomed out, and something too wide for a moment (the bar's buttons
    // as the phone turns upright) would then keep the wide layout for good.
    const phone = os === "android" || os === "ios";
    const w = phone ? document.documentElement.clientWidth || window.innerWidth : window.innerWidth;
    const h = phone ? document.documentElement.clientHeight || window.innerHeight : window.innerHeight;
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
    // Printers has no pane of its own name: a printer's page (#detail),
    // while it shows, names the step.
    const detail = view === "print" && !$("detail").classList.contains("hidden") ? $("detail") : null;
    const pane = detail || $(view);
    const step = pane && pane.dataset.title;
    $("bar-title").textContent = view === "add" ? $("add-title").textContent || TITLES.add : step || TITLES[view] || "Kagaz";
    $("bar-add").classList.toggle("hidden", view !== "home");
    $("bar-menu").classList.toggle("hidden", layout !== "minimal");
    root.dataset.view = view;
    lift();
  }

  // A page's own actions (its gear, Add, Look again) sit in the bar, as
  // every system's apps have them, not in a row of boxes under it. The
  // page marks them data-bar; they move into the bar while their part of
  // the page shows and back when it goes, keeping their handlers. A page's
  // Look again (data-bar="refresh") is the bar's refresh icon.
  const lifted = []; // [node, the mark where it came from]
  let refreshFor = null;
  const shownPane = () => document.querySelector("#main > section.pane:not(.hidden)");
  function lift() {
    for (const [node, mark] of lifted.splice(0)) mark.replaceWith(node);
    const pane = shownPane();
    const slot = $("bar-page");
    refreshFor = null;
    if (pane) {
      // Only what belongs to the part of the page on screen.
      const showing = (n) => { for (let p = n.parentElement; p && p !== pane; p = p.parentElement) if (p.classList.contains("hidden")) return false; return true; };
      for (const n of pane.querySelectorAll("[data-bar]")) {
        if (!showing(n)) continue;
        if (n.dataset.bar === "refresh") { refreshFor = refreshFor || n; continue; }
        const mark = document.createComment("bar");
        n.replaceWith(mark);
        slot.append(n);
        lifted.push([n, mark]);
      }
    }
    const own = pane && "refresh" in pane.dataset;
    $("bar-refresh").classList.toggle("hidden", !(own || (refreshFor && !refreshFor.classList.contains("hidden"))));
    if (refreshFor) {
      $("bar-refresh").title = refreshFor.title || "Look again";
      $("bar-refresh").setAttribute("aria-label", $("bar-refresh").title);
    }
  }
  // Watched on the page: a page or a part of one showing or going (app.js
  // shows the page after telling the shell), and the page's Look again,
  // which turns the bar's icon while it looks (it is disabled meanwhile)
  // and shows or hides it with itself.
  const PARTS = "section.pane, #remote-list-view, #door-settings-view";
  new MutationObserver((list) => {
    if (list.some((m) => m.attributeName === "class" && m.target.matches && m.target.matches(PARTS))) {
      lift();
      return;
    }
    if (!refreshFor) return;
    const b = $("bar-refresh");
    b.classList.toggle("working", refreshFor.disabled);
    b.disabled = refreshFor.disabled;
    b.classList.toggle("hidden", refreshFor.classList.contains("hidden"));
  }).observe($("main"), { attributes: true, attributeFilter: ["disabled", "class"], subtree: true });
  const SHORT = { cameras: "Cameras", print: "Print & scan", fans: "Fans", purifiers: "Purifiers", tvs: "TVs", remote: "Computers" };
  const shortTitle = (v) => SHORT[v] || TITLES[v];

  $("bar-back").addEventListener("click", () => {
    if (KagazNav.depth() > 0) KagazNav.back();
    else selectCategory("home");
  });
  $("bar-add").addEventListener("click", () => KagazHome.openAdd());
  // The page looks again itself (it hears "refresh" on its section).
  $("bar-refresh").addEventListener("click", () => {
    if (refreshFor) { if (!refreshFor.disabled) refreshFor.click(); return; }
    const p = shownPane();
    if (p) p.dispatchEvent(new Event("refresh"));
  });
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
  // A session with a computer or phone has a window of its own
  // (session.html): the ring shows there too, and its Answer and Decline
  // come back here, where the call is.
  const tauriEvents = window.__TAURI__ && window.__TAURI__.event;
  let stripSaid = "";
  function tellWindows(last, again) {
    if (!tauriEvents || !tauriEvents.emit) return;
    const strip = last ? { title: last.title, sub: last.sub || "" } : null;
    const say = JSON.stringify(strip);
    if (say === stripSaid && !again) return;
    stripSaid = say;
    tauriEvents.emit("ring-strip", strip).catch(() => {});
  }
  if (tauriEvents && tauriEvents.listen) {
    tauriEvents.listen("ring-strip-do", (ev) => {
      const last = [...rings.values()].pop();
      if (!last) return;
      if (ev.payload === "answer") last.answer();
      else last.decline();
    });
    // A session's window that opened in the middle of a ring.
    tauriEvents.listen("ring-strip-ask", () => tellWindows([...rings.values()].pop(), true));
  }

  function showRing() {
    const box = $("urgent");
    const last = [...rings.values()].pop();
    tellWindows(last);
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

  return { place, ring, kinds, layout: () => layout, refresh: () => updateBar() };
})();
