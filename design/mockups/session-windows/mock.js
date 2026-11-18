// Session windows, drawn with made-up devices.
//   win.html?os=gnome|windows|macos
//           &scene=main|session
//           &name=Study laptop&desk=windows|ubuntu     (a session window: whose, and what its screen looks like)
//           &ask=close|again|quit                      (a question on top)
//           &ring=1                                    (a call at the door, in a session window)
(() => {
  const q = new URLSearchParams(location.search);
  const root = document.documentElement;
  const $ = (id) => document.getElementById(id);
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  const icon = (name, cls) => {
    const s = el("span", cls || "ic");
    s.dataset.icon = name;
    return s;
  };
  const scene = q.get("scene") || "main";
  const name = q.get("name") || "Study laptop";
  root.dataset.layout = "sidebar";

  function question(title, detail, keep, act, danger) {
    const dlg = el("dialog", "ask");
    const form = el("form");
    form.append(el("p", "ask-title", title));
    for (const d of [].concat(detail)) form.append(el("p", "muted small", d));
    const row = el("div", "actions");
    const k = el("button", "", keep);
    k.type = "button";
    const a = el("button", danger ? "danger" : "accent", act);
    a.type = "button";
    row.append(k, a);
    form.append(row);
    dlg.append(form);
    document.body.append(dlg);
    dlg.showModal();
    k.focus();
  }

  if (scene === "session") {
    // A window that is one device's screen: Kagaz's green bar with the
    // session's tools, and nothing else of Kagaz.
    root.dataset.viewer = "on";
    $("bar-title").textContent = name;
    const tools = $("bar-remote");
    tools.classList.remove("hidden");
    const chip = el("span", "rc-chip");
    chip.append(icon("lock"), el("span", "", "Local network · 2 ms"));
    tools.append(chip);
    for (const [ic, label] of [["keyboard", "Send keys"], ["clipboard", "Send the clipboard"], ["file", "Send a file"], ["speaker", "Its sound"], ["picture", "Picture"], ["full", "Full screen"]]) {
      const b = el("button", "bar-btn");
      b.type = "button";
      b.title = label;
      b.append(icon(ic));
      tools.append(b);
    }
    const view = el("div", "rc-viewer");
    const stage = el("div", "rc-stage");
    const desk = el("div", "desk " + (q.get("desk") || "windows"));
    desk.append(el("div", "desk-top"), el("div", "desk-win one"), el("div", "desk-win two"), el("div", "desk-bar"));
    stage.append(desk);
    view.append(stage);
    $("remote").append(view);
    if (q.get("ring")) {
      const box = $("urgent");
      box.classList.remove("hidden");
      const text = el("span", "urgent-text");
      text.append(el("strong", "", "Front door is ringing"), el("span", "", "Answer opens the call in Kagaz's main window"));
      const acts = el("span", "urgent-actions");
      const ans = el("button", "urgent-btn primary", "Answer");
      const dec = el("button", "urgent-btn", "Decline");
      acts.append(ans, dec);
      box.append(icon("bell", "urgent-icon ic"), text, acts);
    }
    if (q.get("ask") === "close") question(`End the session with ${name}?`, "Everything open on it stays open there.", "Stay", "End session", true);
  } else {
    // Kagaz's main window, free for everything else while sessions run.
    const list = $("categories");
    const entry = (id, label, ic, cls) => {
      const li = el("li", cls || "");
      li.append(icon(ic, "cat-icon"), el("span", "cat-name", label));
      if (id === "remote") li.classList.add("active");
      return li;
    };
    const scroll = el("li", "cat-scroll");
    const kinds = el("ul", "cat-kinds");
    kinds.append(el("li", "cat-head", "Your devices"), entry("cameras", "Cameras", "cameras"), entry("remote", "Computers", "computer"),
      entry("doorbell", "Doorbell", "doorbell"), entry("tv", "TVs", "tv"), entry("add", "Add a device", "plus", "cat-add"));
    scroll.append(kinds);
    list.append(entry("home", "Home", "home"), scroll, entry("tools", "Tools", "tools", "cat-foot"), entry("settings", "Settings", "settings"));
    $("bar-title").textContent = "Computers & phones";
    const rows = el("div", "sw-rows");
    const row = (ic, title, sub, state, button, accent) => {
      const r = el("div", "sw-row");
      const words = el("div", "sw-words");
      const subline = el("span", "sw-sub");
      if (state) subline.append(el("span", "sw-dot " + state));
      subline.append(sub);
      words.append(el("strong", "", title), subline);
      const b = el("button", accent ? "accent" : "", button);
      b.type = "button";
      r.append(icon(ic, "sw-icon ic"), words, b);
      return r;
    };
    rows.append(
      row("laptop", "Study laptop", "In its own window · local network, 2 ms", "live", "Show window"),
      row("computer", "Workshop PC", "In its own window · Tailscale, 31 ms", "live", "Show window"),
      row("mobile", "Old phone", "Within reach: local network", "on", "Connect", true),
      row("computer", "Attic server", "Not reached", "", "Connect"),
    );
    $("remote").append(rows);
    if (q.get("ask") === "again") {
      question("Study laptop already has a window open",
        ["A Windows computer gives each account one screen. Connecting again takes that screen over: the window already open loses it and closes.",
          "With a different account, or a computer that allows it, the two run side by side."],
        "Show that window", "Connect again", false);
    }
    if (q.get("ask") === "quit") question("Close Kagaz and end 2 sessions?", ["Study laptop and Workshop PC. Everything open on them stays open there.", "Settings can keep session windows open when Kagaz's main window closes."], "Stay", "Close Kagaz", true);
  }
  if (typeof KagazIcons !== "undefined") KagazIcons.fill(document);
})();
