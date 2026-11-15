// Computers & phones mockup: sample content for each screen, inside the
// app's own chrome and skins. Plain DOM, no framework, nothing is live.
(() => {
  const q = new URLSearchParams(location.search);
  const root = document.documentElement;
  const os = root.dataset.os;
  const screen = q.get("screen") || "list";
  const state = q.get("state") || "";
  const open = q.get("open") || "";
  const $ = (id) => document.getElementById(id);

  // ---- icons this kind adds, on the app's grid and rules (icons.js):
  // every stroke closes or ends on another, drawn inside 2..22.
  const M = {
    computer: '<rect x="3" y="4" width="18" height="12" rx="1.5"/><path d="M12 16v4M8 20h8"/>',
    laptop: '<rect x="5" y="5" width="14" height="10" rx="1.5"/><path d="M3 18.5h18L19 15H5z"/>',
    keyboard: '<rect x="2.5" y="6" width="19" height="12" rx="2"/><path d="M6.5 10h.01M10.2 10h.01M13.8 10h.01M17.5 10h.01M6.5 14h.01M17.5 14h.01" class="dot"/><path d="M10 14h4"/>',
    clipboard: '<rect x="5" y="4.5" width="14" height="16.5" rx="2"/><rect x="9" y="2.5" width="6" height="4" rx="1"/>',
    file: '<path d="M7 3h6.5l5 5v11.5A1.5 1.5 0 0 1 17 21H7a1.5 1.5 0 0 1-1.5-1.5v-15A1.5 1.5 0 0 1 7 3zM13.5 3v5h5"/>',
    screens: '<rect x="2.5" y="5.5" width="11.5" height="8.5" rx="1.2"/><rect x="10" y="10" width="11.5" height="8.5" rx="1.2"/>',
    picture: '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 15.5l4.5-4.5 4 4 3-3 6.5 6.5"/>',
    lock: '<rect x="5" y="10.5" width="14" height="10" rx="2"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5"/>',
    trackpad: '<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M3 14.5h18M12 14.5V20"/>',
  };
  const mi = (name) => `<svg viewBox="0 0 24 24" class="kz-i" aria-hidden="true">${M[name]}</svg>`;
  const ic = (name) => (M[name] ? mi(name) : KagazIcons.svg(name));
  document.querySelectorAll("[data-m]").forEach((el) => (el.innerHTML = mi(el.dataset.m)));

  // ---- the navigation that fits (shell.js's own rule) ----
  function pickLayout() {
    const w = innerWidth, h = innerHeight;
    switch (os) {
      case "android": return w < 600 && !(h < 500 && w > h) ? "bar" : w >= 1200 ? "sidebar" : "rail";
      case "ios": return w < 600 ? "bar" : "sidebar";
      case "windows": return w <= 640 ? "minimal" : w < 1008 ? "rail" : "sidebar";
      case "macos": return "sidebar";
      default: return w < 550 ? "bar" : "sidebar";
    }
  }
  function applyLayout() {
    const layout = pickLayout();
    root.dataset.layout = layout;
    root.dataset.view = screen === "viewer" ? "viewer" : "remote";
    root.dataset.pointer = os === "android" || os === "ios" ? "coarse" : "fine";
    const inside = screen !== "list";
    $("bar-back").classList.toggle("hidden", !(inside || layout === "bar"));
    $("bar-back").querySelector(".bar-back-word").textContent = inside ? "Computers" : "Home";
    $("bar-menu").classList.toggle("hidden", layout !== "minimal" || inside);
  }
  addEventListener("resize", applyLayout);

  // ---- sample devices (invented) ----
  const devices = [
    { icon: "laptop", name: "Study laptop", what: "Windows", reach: "Within reach", how: "home Wi-Fi, 12\u00a0ms", ok: true },
    { icon: "computer", name: "Living-room computer", what: "Ubuntu", reach: "Within reach", how: "Tailscale, 48\u00a0ms", ok: true },
    { icon: "mobile", name: "My phone", what: "Android", reach: "Within reach", how: "its hotspot, 6\u00a0ms", ok: true },
    { icon: "computer", name: "Office desktop", what: "Windows", reach: "Out of reach", how: "not on this network, and Tailscale is off there", ok: false },
  ];
  const nearby = [
    { icon: "computer", name: "DESKTOP-7Q2", where: "Windows Remote Desktop · 192.0.2.31" },
    { icon: "mobile", name: "Pixel tablet", where: "VNC · 192.0.2.44" },
  ];
  const tailnet = [
    { icon: "laptop", name: "travel-laptop", where: "Linux · online" },
    { icon: "computer", name: "media-box", where: "Windows · last seen 3 days ago", off: true },
  ];
  const dev = devices[0];

  const tile = (d) => `
    <div class="home-tile has-actions ${d.ok ? "on" : ""}">
      <button type="button" class="tile-main">
        <span class="tile-icon">${ic(d.icon)}</span>
        <span class="tile-text"><span class="tile-name">${d.name}</span>
          <span class="tile-sub"><span class="rc-dot ${d.ok ? "ok" : ""}"></span><span class="rc-what">${d.what} · </span>${d.reach}: ${d.how}</span></span>
      </button>
      <span class="tile-actions">
        <button type="button" class="tile-act" ${d.ok ? "" : "disabled"}>${ic("computer")}<span>Connect</span></button>
        <button type="button" class="tile-star" aria-label="More for ${d.name}">${ic("more")}</button>
      </span>
    </div>`;
  const row = (d, word = "Add") => `
    <li class="rc-row ${d.off ? "off" : ""}"><span class="tile-icon">${ic(d.icon)}</span>
      <span class="tile-text"><span class="tile-name">${d.name}</span><span class="tile-sub">${d.where}</span></span>
      <button type="button">${word}</button></li>`;

  // ---- screens ----
  const screens = {
    list() {
      if (state === "empty") {
        return `<div class="rc-empty"><span class="rc-empty-icon">${ic("computer")}</span>
          <h3>No computers or phones yet</h3>
          <p class="muted">Add one of yours to see its screen here and use it from this device.</p>
          <button type="button" class="accent">${ic("plus")}<span>Add a computer or phone</span></button></div>
          <h3 class="list-head">Found nearby</h3><p class="muted" role="status">Looking on this network and in Tailscale…</p>`;
      }
      return `<div class="pane-head"><h2>Computers &amp; phones</h2><div class="fan-tools">
          <button type="button">${ic("reload")}<span>Look again</span></button>
          <button type="button" class="accent">${ic("plus")}<span>Add</span></button></div></div>
        <div class="home-tiles rc-tiles">${devices.map(tile).join("")}</div>
        <h3 class="list-head">Found nearby</h3>
        <p class="muted" role="status">${state === "searching" ? "Looking on this network and in Tailscale…" : "2 found. Add one to keep it here."}</p>
        <ul class="rc-rows">${state === "searching" ? "" : nearby.map((d) => row(d)).join("")}</ul>`;
    },

    add() {
      const signin = state === "signin";
      return `
        <section class="set-section ${signin ? "rc-done" : ""}"><h3>Found on this network</h3>
          <ul class="rc-rows">${nearby.map((d) => row(d, "Choose")).join("")}</ul></section>
        <section class="set-section ${signin ? "rc-done" : ""}"><h3>In your Tailscale</h3>
          <p class="muted">Read from Tailscale on this computer. Nothing is changed there.</p>
          <ul class="rc-rows">${tailnet.map((d) => row(d, "Choose")).join("")}</ul></section>
        <section class="set-section"><h3>${signin ? "Sign in to Study laptop" : "By its address"}</h3>
          ${signin ? `
          <p class="muted">Windows Remote Desktop at 192.0.2.20. Its certificate will be remembered, and Kagaz warns if it ever changes.</p>
          <form class="rc-form" onsubmit="return false">
            <label>User name<input type="text" autocomplete="username" value="asha"></label>
            <label>Password<span class="rc-pass"><input type="password" autocomplete="current-password" value="samplepass"><button type="button" aria-label="Show the password">${ic("eye")}</button></span></label>
            <label class="switch"><input type="checkbox" checked> Remember the password in this computer's password store</label>
            <div class="rc-buttons"><button type="button">Cancel</button><button type="submit" class="accent">Connect</button></div>
          </form>` : `
          <form class="rc-form" onsubmit="return false">
            <label>Name<input type="text" value="Study laptop"></label>
            <label>Address<input type="text" value="study-laptop.local" aria-describedby="addr-hint"><small id="addr-hint">A name or an address, such as study-laptop.local or 192.0.2.20.</small></label>
            <label>It speaks<select><option>Find out for me</option><option>Windows Remote Desktop (RDP)</option><option>VNC</option></select></label>
            <div class="rc-buttons"><button type="submit" class="accent">Add device</button></div>
          </form>`}
        </section>`;
    },

    device() {
      return `
        <div class="rc-head"><span class="rc-big">${ic(dev.icon)}</span>
          <div class="rc-head-text"><h2>${dev.name}</h2>
            <p class="muted"><span class="rc-dot ok"></span>${dev.what} · ${dev.reach}: ${dev.how}</p></div>
          <button type="button" class="accent big">Connect</button></div>
        <section class="set-section"><h3>Ways to reach it</h3>
          <p class="muted">All are tried at once; the first to answer is used.</p>
          <div class="set-row"><span><strong>Home Wi-Fi</strong> · 192.0.2.20</span><span class="rc-good">${ic("check")} answers, 12 ms</span></div>
          <div class="set-row"><span><strong>Tailscale</strong> · 100.64.0.7</span><span class="rc-good">${ic("check")} answers, 48 ms</span></div>
          <div class="set-row"><span><strong>Name</strong> · study-laptop.local</span><span class="muted">no answer</span></div>
          <div class="rc-buttons start"><button type="button">Add an address</button><button type="button">Check again</button></div></section>
        <section class="set-section"><h3>Screen</h3>
          <div class="set-row"><label for="size">Size</label><select id="size"><option>Fit this window, sharp</option><option>The computer's own size</option></select></div>
          <div class="set-row"><label for="mon">Monitor</label><select id="mon"><option>All, side by side</option><option>Monitor 1</option><option>Monitor 2</option></select></div>
          <div class="set-row"><label for="pic">Picture</label><span class="rc-slider"><span>Sharper</span><input id="pic" type="range" min="0" max="10" value="5"><span>Smoother</span></span></div>
          <div class="set-row"><label class="switch"><input type="checkbox" checked> Adjust by itself to the connection</label></div></section>
        <section class="set-section"><h3>Mouse, keyboard and sound</h3>
          <div class="set-row"><label class="switch wrap"><input type="checkbox" checked> Keep left click on the left<small class="muted">For a computer whose mouse buttons are swapped.</small></label></div>
          <div class="set-row"><label class="switch"><input type="checkbox" checked> Play the computer's sound here</label></div></section>
        <section class="set-section"><h3>Sign-in</h3>
          <div class="set-row"><span>User name</span><span>asha</span></div>
          <div class="set-row"><label class="switch"><input type="checkbox" checked> Remember the password</label></div></section>
        <p><button type="button" class="danger-text">Forget this device</button></p>`;
    },

    viewer() {
      const vnc = state === "vnc";
      if (state === "connecting") {
        return `<div class="rc-stage"><div class="rc-card" role="status">
          <h3>Connecting to ${dev.name}</h3>
          <ul class="rc-paths">
            <li><span>Home Wi-Fi · 192.0.2.20</span><span class="muted"><span class="rc-spin"></span> trying</span></li>
            <li><span>Tailscale · 100.64.0.7</span><span class="rc-good">${ic("check")} answered, 48 ms</span></li>
            <li><span>Name · study-laptop.local</span><span class="muted">no answer</span></li></ul>
          <p class="muted">Signing in over Tailscale…</p>
          <div class="rc-buttons"><button type="button">Cancel</button></div></div></div>`;
      }
      if (state === "lost") {
        return `<div class="rc-stage"><div class="rc-card">
          <h3>${dev.name} is out of reach</h3>
          <p>It answers on your home network or through Tailscale. This device is on another network, and Tailscale is turned off here.</p>
          <p class="muted">Turn Tailscale on, or join the home Wi-Fi, then try again.</p>
          <div class="rc-buttons"><button type="button">Edit its addresses</button><button type="button" class="accent">Try again</button></div></div></div>`;
      }
      const dim = vnc ? 'disabled title="VNC carries none of this; the device needs Remote Desktop for it"' : "";
      const phoneBar = `<div class="rc-touchbar">
          <button type="button">${ic("keyboard")}<span>Keyboard</span></button>
          <button type="button" aria-pressed="false">${ic("trackpad")}<span>Trackpad</span></button>
          <button type="button">${ic("clipboard")}<span>Paste</span></button>
          <button type="button">${ic("more")}<span>More</span></button></div>`;
      const pop = open === "keys" ? `<div class="rc-pop keys" role="menu"><strong>Send keys</strong>
            ${["Ctrl + Alt + Del", "Windows key", "Alt + Tab", "Esc", "Print Screen", "Alt + F4"].map((k) => `<button type="button" role="menuitem">${k}</button>`).join("")}</div>`
        : open === "picture" ? `<div class="rc-pop picture"><strong>Picture</strong>
            <span class="rc-slider"><span>Sharper</span><input type="range" min="0" max="10" value="6" aria-label="Sharper to smoother"><span>Smoother</span></span>
            <label class="switch"><input type="checkbox" checked> Adjust by itself</label>
            <p class="muted">14 ms delay · 2.1 Mbit/s now · 38 MB this session</p></div>` : "";
      return `${vnc ? `<div class="rc-banner" role="status">${ic("unlock")}<span><strong>Not encrypted on this network.</strong> The screen and what you type can be read by others on it. Through Tailscale they are protected.</span></div>` : ""}
        <div class="rc-stage"><div class="rc-remote ${vnc ? "tall" : ""}" role="img" aria-label="The remote screen (a drawing, sample only)">
            <div class="rc-win a"></div><div class="rc-win b"></div><div class="rc-task"></div>
            <span class="rc-sample">Sample picture</span></div>${pop}</div>
        ${phoneBar}
        <template id="viewer-actions">
          <span class="rc-chip" title="How this session travels">${vnc ? ic("unlock") : ic("lock")}<span>${vnc ? "Home Wi-Fi · 9 ms" : "Home Wi-Fi · 14 ms"}</span></span>
          <button type="button" class="bar-btn" title="Send keys such as Ctrl + Alt + Del" ${open === "keys" ? 'aria-expanded="true"' : ""}>${ic("keyboard")}</button>
          <button type="button" class="bar-btn" title="Clipboard: copies travel both ways">${ic("clipboard")}</button>
          <button type="button" class="bar-btn" title="Send a file" ${dim}>${ic("file")}</button>
          <button type="button" class="bar-btn" title="Mute the computer's sound here" ${dim}>${ic("speaker")}</button>
          <button type="button" class="bar-btn" title="Choose a monitor" ${dim}>${ic("screens")}</button>
          <button type="button" class="bar-btn" title="Picture: sharper or smoother" ${open === "picture" ? 'aria-expanded="true"' : ""}>${ic("picture")}</button>
          <button type="button" class="bar-btn" title="Full screen">${ic("full")}</button>
        </template>`;
    },
  };

  const TITLES = { list: "Computers & phones", add: "Add a computer or phone", device: dev.name, viewer: screen === "viewer" && state === "vnc" ? "My phone" : dev.name };
  $("bar-title").textContent = TITLES[screen] || TITLES.list;
  $("main").innerHTML = `<section class="pane rc-pane rc-${screen}">${(screens[screen] || screens.list)()}</section>`;
  const acts = document.getElementById("viewer-actions");
  if (acts) $("bar-actions").append(acts.content);
  KagazIcons.fill();
  applyLayout();
})();
