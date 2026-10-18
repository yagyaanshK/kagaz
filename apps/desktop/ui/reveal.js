// Every password field gets a Show/Hide button, so people can check what
// they typed before signing in (on a phone especially). Fields added later
// get one too. A field that already has its own button (aria-controls) is
// left alone. KagazReveal.hide(input) puts a field back to hidden, as after
// a sign-in.

const KagazReveal = (() => {
  function set(input, button, shown) {
    input.type = shown ? "text" : "password";
    button.textContent = shown ? "Hide" : "Show";
    button.setAttribute("aria-pressed", String(shown));
    button.setAttribute("aria-label", shown ? "Hide password" : "Show password");
  }

  function attach(input) {
    if (input.dataset.reveal) return;
    if (input.id && document.querySelector(`button[aria-controls="${CSS.escape(input.id)}"]`)) return;
    input.dataset.reveal = "1";
    const wrap = document.createElement("span");
    wrap.className = "reveal-field";
    input.parentNode.insertBefore(wrap, input);
    wrap.appendChild(input);
    const button = document.createElement("button");
    button.type = "button";
    button.className = "reveal-button";
    if (input.id) button.setAttribute("aria-controls", input.id);
    set(input, button, false);
    wrap.appendChild(button);
    // Keep the keyboard up and the caret where it was on a phone.
    button.addEventListener("pointerdown", (ev) => ev.preventDefault());
    button.addEventListener("click", () => {
      const at = input.selectionStart;
      set(input, button, input.type === "password");
      input.focus();
      try { input.setSelectionRange(at, at); } catch (_) {}
    });
    input._reveal = button;
  }

  function hide(input) {
    if (input && input._reveal) set(input, input._reveal, false);
  }

  function scan(root) {
    if (root.matches && root.matches('input[type="password"]')) attach(root);
    if (root.querySelectorAll) root.querySelectorAll('input[type="password"]').forEach(attach);
  }

  document.addEventListener("DOMContentLoaded", () => {
    scan(document.body);
    new MutationObserver((changes) => {
      for (const c of changes) c.addedNodes.forEach((n) => n.nodeType === 1 && scan(n));
    }).observe(document.body, { childList: true, subtree: true });
  });

  return { hide, scan };
})();
