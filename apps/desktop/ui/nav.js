// Back: Android's Back button (and a browser's) goes one step up, as
// people expect: a full-screen picture or an open viewer closes first,
// then a page inside a page (a device's settings, an open card, a
// printer), then a device's page goes to Home. On Home, Back leaves the
// app as usual.
//
// Tauri hands Back to the page's history when it has somewhere to go, so
// the page keeps one history entry per level open above Home. Each level
// has a way to close it; Back pops levels and closes them. When the page
// closes a level itself (its own button), the history is put back to
// match.

const KagazNav = (() => {
  let stack = []; // [{ id, close }]; Home is the empty stack
  let depth = 0; // our history entries above Home
  let pending = false; // a history.go() is on its way
  let closing = false;

  try {
    history.replaceState({ depth: 0 }, "");
  } catch (e) {}

  function sync() {
    if (pending) return; // again when it lands
    const want = stack.length;
    if (want > depth) {
      for (; depth < want; depth++) history.pushState({ depth: depth + 1 }, "");
    } else if (want < depth) {
      pending = true;
      history.go(want - depth);
      depth = want;
    }
  }

  window.addEventListener("popstate", (ev) => {
    if (pending) {
      pending = false;
      sync();
      return;
    }
    // Back (or forward): close the levels above where history now is.
    const d = (ev.state && ev.state.depth) || 0;
    depth = d;
    closing = true;
    while (stack.length > d) {
      const level = stack.pop();
      try {
        level.close();
      } catch (e) {}
    }
    closing = false;
    sync(); // forward past what is open: step back to it
  });

  // A computer's Back: Alt+Left (Windows, Linux), Cmd+[ (macOS) and the
  // mouse's back button. The web view itself does not act on them (WebView2
  // has its browser keys off), so they step back here, never past Home.
  const mac = /Mac/.test(navigator.platform);
  function back() {
    if (depth > 0 && !pending) history.back();
  }
  window.addEventListener("keydown", (ev) => {
    const isBack = mac
      ? ev.metaKey && !ev.altKey && !ev.ctrlKey && ev.key === "["
      : ev.altKey && !ev.ctrlKey && !ev.metaKey && !ev.shiftKey && ev.key === "ArrowLeft";
    if (!isBack) return;
    ev.preventDefault();
    back();
  });
  // Button 3 is the mouse's back button; stopping it here keeps a web view
  // that would also act on it from going back twice.
  window.addEventListener("mousedown", (ev) => {
    if (ev.button === 3) ev.preventDefault();
  });
  window.addEventListener("mouseup", (ev) => {
    if (ev.button !== 3) return;
    ev.preventDefault();
    back();
  });

  // A level opened on top of what shows.
  function push(id, close) {
    if (closing) return;
    stack = stack.filter((l) => l.id !== id);
    stack.push({ id, close });
    sync();
  }

  // That level closed from the page itself; levels above it go too.
  function pop(id) {
    if (closing) return;
    const i = stack.findIndex((l) => l.id === id);
    if (i < 0) return;
    stack.length = i;
    sync();
  }

  // A top-level place: Home (null), or a page one level above Home.
  function root(id, close) {
    if (closing) return;
    stack = id ? [{ id, close }] : [];
    sync();
  }

  return { push, pop, root };
})();
