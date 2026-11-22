// Loaded before the Claude tab's own page script (src/claude_chat/page.js)
// in the phone's copy of that page, a frame of the phone app. It stands in
// for the Mac web view's IPC: what the page posts goes to the phone app
// (app.js, `__abromux.chatPost`), which relays it to the Mac's tab, and the
// Mac tab's events come back through `window.__chat.recv`.
"use strict";
window.__phone = true;
// A terminal's Claude Code session (`#t:<surface>`): what you write is
// typed into the terminal, so the controls only a Claude tab has go.
if (location.hash.startsWith("#t:")) document.documentElement.dataset.terminal = "";
(() => {
  const host = window.parent && window.parent !== window ? window.parent.__abromux : null;
  window.ipc = {
    postMessage(body) {
      if (host) host.chatPost(window, body);
    },
  };

  // The phone's own light or dark, not the Mac's.
  const dark = matchMedia("(prefers-color-scheme: dark)");
  const theme = () => { document.documentElement.dataset.theme = dark.matches ? "dark" : "light"; };
  theme();
  dark.addEventListener("change", theme);

  // A photo button in the composer: images go in as a paste would.
  document.addEventListener("DOMContentLoaded", () => {
    const row = document.querySelector(".composer-row");
    if (!row || "terminal" in document.documentElement.dataset) return;
    const pick = document.createElement("input");
    pick.type = "file";
    pick.accept = "image/*";
    pick.multiple = true;
    pick.hidden = true;
    const b = document.createElement("button");
    b.className = "attach";
    b.type = "button";
    b.setAttribute("aria-label", "Add photos");
    b.innerHTML = '<svg viewBox="0 0 16 16"><path d="M8 3.25v9.5M3.25 8h9.5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></svg>';
    b.addEventListener("click", () => pick.click());
    pick.addEventListener("change", () => {
      for (const f of pick.files || []) window.__chat && window.__chat.addImage(f);
      pick.value = "";
    });
    row.prepend(b, pick);
  });
})();
