// This computer as a screen (here.rs): the question when a device asks to
// show its screen here, and the window its screen shows in.
(() => {
  const { invoke, Channel } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const $ = (id) => document.getElementById(id);
  const page = document.documentElement.dataset.pop;
  if (typeof KagazIcons !== "undefined") KagazIcons.fill(document);

  if (page === "here-ask") {
    let left = 30;
    const show = (st) => {
      if (st && st.asking) $("ask-who").textContent = `${st.asking} wants to show its screen here`;
    };
    invoke("here_status").then(show).catch(() => {});
    listen("here", (e) => show(e.payload));
    const tick = () => {
      $("ask-left").textContent = left > 0 ? `No answer in ${left} seconds means no.` : "Declined.";
      left -= 1;
    };
    tick();
    setInterval(tick, 1000);
    $("ask").addEventListener("submit", (e) => {
      e.preventDefault();
      invoke("here_answer", { allow: true });
    });
    $("ask-no").addEventListener("click", () => invoke("here_answer", { allow: false }));
    $("ask-yes").focus();
    return;
  }

  // The picture's window.
  const stage = $("here-stage");
  const player = new KagazPlayer.Player(stage, { sound: false, label: "here" });
  let size = "";
  player.onframe = () => $("here-wait").classList.add("hidden");
  player.onsize = (w, h) => {
    size = `${w} × ${h}`;
    $("here-info").textContent = size;
  };
  player.onended = () => {
    $("here-wait").textContent = "The device stopped showing its screen.";
    $("here-wait").classList.remove("hidden");
  };
  const channel = new Channel();
  channel.onmessage = (m) => player.feed(m);
  invoke("here_watch", { channel });
  const show = (st) => {
    if (st && st.showing) {
      $("bar-title").textContent = `${st.showing}'s screen`;
      document.title = `${st.showing} · Kagaz`;
    }
  };
  invoke("here_status").then(show).catch(() => {});
  listen("here", (e) => show(e.payload));
  // The pictures a second, beside the size.
  listen("here-line", (e) => {
    const m = /^(\d+) pictures a second/.exec(e.payload || "");
    if (m) $("here-info").textContent = [size, `${m[1]} a second`].filter(Boolean).join(" · ");
  });
  $("here-stop").addEventListener("click", () => invoke("here_stop"));
  // Full screen: the bar fades away until the pointer moves.
  const full = $("here-full");
  full.addEventListener("click", () => {
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    else document.documentElement.requestFullscreen().catch(() => {});
  });
  let idle = null;
  const wake = () => {
    document.body.classList.remove("here-idle");
    clearTimeout(idle);
    if (document.fullscreenElement) idle = setTimeout(() => document.body.classList.add("here-idle"), 2500);
  };
  document.addEventListener("fullscreenchange", () => {
    const on = !!document.fullscreenElement;
    full.innerHTML = KagazIcons.svg(on ? "unfull" : "full");
    full.title = on ? "Leave full screen" : "Full screen";
    full.setAttribute("aria-label", full.title);
    wake();
  });
  document.addEventListener("mousemove", wake);
  document.addEventListener("keydown", (e) => {
    if (e.key === "f" || e.key === "F") full.click();
  });
})();
