// Which system Kagaz runs on, before anything is drawn: the page wears that
// system's own look (skins.css). The app says it (a Tauri plugin sets
// window.__KAGAZ_OS__ before the page loads, main.rs); a plain browser, as
// in the tests, falls back to its user agent, and ?os= picks one by hand.
//   android · ios · windows · gnome (Linux) · macos
(() => {
  const KNOWN = ["android", "ios", "windows", "gnome", "macos"];
  let os = null;
  try {
    const asked = new URLSearchParams(location.search).get("os");
    if (KNOWN.includes(asked)) os = asked;
  } catch (e) {}
  if (!os && KNOWN.includes(window.__KAGAZ_OS__)) os = window.__KAGAZ_OS__;
  if (!os) {
    const ua = navigator.userAgent;
    os = /Android/i.test(ua) ? "android"
      : /iPhone|iPad|iPod/i.test(ua) ? "ios"
      : /Windows/i.test(ua) ? "windows"
      : /Macintosh|Mac OS X/i.test(ua) ? "macos"
      : "gnome";
  }
  document.documentElement.dataset.os = os;
  window.KAGAZ_OS = os;
})();
