// The small windows of sharing this computer (share.rs opens them): the
// question when a paired device asks to connect, and the sign while one
// is connected.
(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const $ = (id) => document.getElementById(id);
  const send = (line) => invoke("share_send", { line }).catch(() => {});
  const page = document.documentElement.dataset.pop;

  if (page === "ask") {
    let asking = null;
    let left = 30;
    let timer = null;
    const show = (st) => {
      const next = (st.asking || [])[0] || null;
      if (!next) return;
      if (!asking || asking.id !== next.id) {
        asking = next;
        left = next.seconds || 30;
        $("ask-who").textContent = `${next.name} wants to connect`;
        clearInterval(timer);
        timer = setInterval(() => {
          left -= 1;
          $("ask-left").textContent = left > 0 ? `No answer in ${left} seconds means no.` : "Not allowed.";
          if (left <= 0) clearInterval(timer);
        }, 1000);
        $("ask-left").textContent = `No answer in ${left} seconds means no.`;
        $("ask-yes").focus();
      }
    };
    invoke("share_status").then(show);
    listen("share", (ev) => show(ev.payload));
    $("ask").addEventListener("submit", (ev) => {
      ev.preventDefault();
      if (!asking) return;
      const how = document.querySelector("input[name=how]:checked").value;
      send(`answer ${asking.id} ${how}`);
    });
    $("ask-no").addEventListener("click", () => asking && send(`answer ${asking.id} deny`));
    if (typeof KagazIcons !== "undefined") KagazIcons.fill(document);
  } else {
    let may = false;
    const show = (st) => {
      const s = st.session;
      if (!s) return;
      may = String(s.may || "").split(" ").includes("control");
      $("sign-text").textContent = st.control ? `${s.name} can use this computer` : `${s.name} is watching this screen`;
      $("sign-mode").textContent = st.control ? "Watch only" : "Let it control";
      $("sign-mode").classList.toggle("hidden", !st.control && !may);
      $("sign-mode").dataset.control = st.control ? "1" : "";
      // Files the device sent, in Downloads: shown in Explorer.
      const got = st.received;
      $("sign-files").classList.toggle("hidden", !got);
      if (got) $("sign-files").textContent = got.count === 1 ? "1 file in Downloads" : `${got.count} files in Downloads`;
    };
    invoke("share_status").then(show);
    listen("share", (ev) => show(ev.payload));
    $("sign-mode").addEventListener("click", () => send($("sign-mode").dataset.control ? "watch" : "control"));
    $("sign-end").addEventListener("click", () => send("end"));
    $("sign-files").addEventListener("click", () => send("show-received"));
  }
})();
