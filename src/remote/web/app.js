// abromux on a phone: the Mac's workspaces in a sidebar as the Mac shows
// them, each workspace's tabs, terminals live, and Claude tabs as the Mac's
// own chat page. Everything goes over one WebSocket (/api/ws) that relays
// control-socket calls (see src/remote/mod.rs).
"use strict";

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};
const icon = (name, cls = "") => {
  const i = el("i", "ic " + cls);
  i.style.setProperty("--ic", `url(/icons/${name}.svg)`);
  return i;
};
const tilde = (p) => (p || "").replace(/^\/Users\/[^/]+/, "~");
const store = {
  get(k, d) { try { const v = localStorage.getItem("abromux." + k); return v == null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem("abromux." + k, JSON.stringify(v)); } catch {} },
};

// ---- connection ------------------------------------------------------------

const conn = {
  ws: null,
  next: 1,
  calls: new Map(), // id -> {resolve, reject}
  subs: new Map(), // id -> subscription (its current start)
  pending: new Set(), // subscriptions to start once connected
  backoff: 500,
  up: false,
  listeners: [],
};

function connect() {
  const url = (location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/api/ws";
  const ws = new WebSocket(url);
  conn.ws = ws;
  setConn("connecting");
  ws.onopen = () => {
    conn.up = true;
    conn.backoff = 500;
    setConn("up");
    sendVisible();
    // Every subscription starts again, under a new id.
    const all = new Set([...conn.subs.values(), ...conn.pending]);
    conn.subs.clear();
    conn.pending.clear();
    for (const s of all) startSub(s);
    conn.listeners.forEach((f) => f());
  };
  ws.onmessage = (m) => {
    let msg;
    try { msg = JSON.parse(m.data); } catch { return; }
    if (msg.sub != null) {
      const s = conn.subs.get(msg.sub);
      if (!s) return;
      if (msg.ended) {
        conn.subs.delete(msg.sub);
        s.live = false;
        if (s.id === msg.sub) s.id = 0;
        s.onEnd && s.onEnd();
        return;
      }
      s.onData(msg.data);
      return;
    }
    const c = conn.calls.get(msg.id);
    if (!c) return;
    conn.calls.delete(msg.id);
    if (msg.ok) c.resolve(msg.result);
    else c.reject(new Error(msg.error || "failed"));
  };
  ws.onclose = () => {
    conn.up = false;
    setConn("down");
    for (const [, c] of conn.calls) c.reject(new Error("disconnected"));
    conn.calls.clear();
    for (const [, s] of conn.subs) s.live = false;
    setTimeout(connect, conn.backoff);
    conn.backoff = Math.min(conn.backoff * 2, 5000);
  };
}

let downTimer = 0;
function setConn(state) {
  // A blip isn't worth a banner.
  clearTimeout(downTimer);
  if (state === "up") $("offline").hidden = true;
  else downTimer = setTimeout(() => { $("offline").hidden = conn.up; }, 1500);
}

function send(obj) {
  if (conn.ws && conn.ws.readyState === WebSocket.OPEN) conn.ws.send(JSON.stringify(obj));
}

function call(method, params = {}) {
  return new Promise((resolve, reject) => {
    if (!conn.up) return reject(new Error("Not connected to the Mac"));
    const id = conn.next++;
    conn.calls.set(id, { resolve, reject });
    send({ id, method, params });
  });
}

// A subscription that is made again after every reconnect. `onStart` gets
// the call's result each time. Each start has its own id, so the end of an
// old one can't be taken for the new one's.
function subscribe(method, params, { onStart, onData, onEnd, onError }) {
  const s = { id: 0, method, params, onStart, onData, onEnd, onError, live: false, cancelled: false };
  if (conn.up) startSub(s);
  else conn.pending.add(s);
  const stop = () => {
    if (s.id) {
      conn.subs.delete(s.id);
      conn.calls.delete(s.id);
      send({ cancel: s.id });
      s.id = 0;
    }
  };
  return {
    cancel() {
      s.cancelled = true;
      conn.pending.delete(s);
      stop();
    },
    restart() {
      stop();
      if (conn.up) startSub(s);
      else conn.pending.add(s);
    },
  };
}

function startSub(s) {
  if (s.cancelled) return;
  const id = conn.next++;
  s.id = id;
  s.live = true;
  conn.subs.set(id, s);
  conn.calls.set(id, {
    resolve: (r) => s.onStart && s.onStart(r),
    reject: (e) => { s.live = false; s.onError && s.onError(e); },
  });
  send({ id, method: s.method, params: s.params });
}

function onConnect(f) { conn.listeners.push(f); }

function sendVisible() {
  send({ visible: document.visibilityState === "visible" });
}
document.addEventListener("visibilitychange", () => {
  sendVisible();
  if (document.visibilityState === "visible") {
    if (!conn.up && conn.ws && conn.ws.readyState > 1) connect();
    else refresh();
  }
});

// ---- app state -------------------------------------------------------------

const state = {
  workspaces: [],
  groups: [],
  notes: [],
  route: { ws: null, surface: null },
  loaded: false,
  closedGroups: new Set(store.get("closedGroups", [])),
};

function toast(text, ms = 2400) {
  const t = $("toast");
  t.textContent = text;
  t.hidden = false;
  t.style.animation = "none";
  void t.offsetWidth;
  t.style.animation = "";
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { t.hidden = true; }, ms);
}

async function refresh() {
  if (!conn.up) return;
  try {
    const [tree, groups, notes] = await Promise.all([
      call("workspace.tree", { all: true }),
      call("group.list").catch(() => []),
      call("notification.list").catch(() => []),
    ]);
    state.workspaces = tree;
    state.groups = groups;
    state.notes = notes;
    state.loaded = true;
    render();
  } catch (e) {
    if (conn.up) toast(e.message);
  }
}

let refreshTimer = 0;
function refreshSoon(ms = 200) {
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(refresh, ms);
}

// ---- routing: #/ws/<workspace>/<surface> ----------------------------------

function parseRoute() {
  const h = decodeURIComponent(location.hash.replace(/^#\/?/, ""));
  if (h.startsWith("ws/")) {
    const [ws, s] = h.slice(3).split("/");
    return { ws: ws || null, surface: s || null };
  }
  return { ws: null, surface: null };
}

function go(ws, surface) {
  const hash = ws ? `#/ws/${encodeURIComponent(ws)}${surface ? "/" + encodeURIComponent(surface) : ""}` : "#/";
  if (location.hash !== hash) history.replaceState(null, "", hash);
  state.route = { ws, surface };
  if (ws) store.set("last", { ws, surface });
  render();
}

window.addEventListener("hashchange", () => {
  state.route = parseRoute();
  render();
});

// ---- the sidebar: open, closed, or dragged between -----------------------

const app = $("app");
const wide = () => matchMedia("(min-width: 820px)").matches;
let sbOpen = false;

function setProgress(p) {
  app.style.setProperty("--p", String(p));
}
function setSidebar(open) {
  sbOpen = open && !wide();
  app.classList.toggle("sb-open", sbOpen);
  setProgress(sbOpen ? 1 : 0);
  if (sbOpen) document.activeElement && document.activeElement.blur && document.activeElement.blur();
}
$("menu-btn").onclick = () => setSidebar(true);
$("scrim").onclick = () => setSidebar(false);
matchMedia("(min-width: 820px)").addEventListener("change", () => setSidebar(false));

// Swipe in from the right edge to pull the sidebar out; swipe it back to close.
(() => {
  let d = null;
  const width = () => $("sidebar").offsetWidth || 300;
  const start = (e, opening) => {
    if (wide() || e.touches.length !== 1) return;
    const t = e.touches[0];
    d = { x: t.clientX, y: t.clientY, opening, active: false, last: t.clientX, lastT: e.timeStamp, v: 0 };
  };
  $("edge").addEventListener("touchstart", (e) => { if (!sbOpen) start(e, true); }, { passive: true });
  $("main").addEventListener("touchstart", (e) => {
    if (!sbOpen && e.touches[0].clientX > window.innerWidth - 20) start(e, true);
  }, { passive: true });
  $("sidebar").addEventListener("touchstart", (e) => { if (sbOpen) start(e, false); }, { passive: true });
  $("scrim").addEventListener("touchstart", (e) => { if (sbOpen) start(e, false); }, { passive: true });
  document.addEventListener("touchmove", (e) => {
    if (!d) return;
    const t = e.touches[0];
    const dx = t.clientX - d.x, dy = t.clientY - d.y;
    if (!d.active) {
      if (Math.abs(dy) > 10 && Math.abs(dy) > Math.abs(dx)) { d = null; return; }
      if (Math.abs(dx) < 8) return;
      if (d.opening ? dx > 0 : dx < 0) { d = null; return; }
      d.active = true;
      app.classList.add("dragging");
    }
    const dt = Math.max(1, e.timeStamp - d.lastT);
    d.v = (t.clientX - d.last) / dt;
    d.last = t.clientX;
    d.lastT = e.timeStamp;
    const w = width();
    const p = d.opening ? -dx / w : 1 - dx / w;
    setProgress(Math.max(0, Math.min(1, p)));
  }, { passive: true });
  const end = () => {
    if (!d) return;
    const was = d;
    d = null;
    if (!was.active) return;
    app.classList.remove("dragging");
    const p = parseFloat(getComputedStyle(app).getPropertyValue("--p")) || 0;
    const open = was.v < -0.35 ? true : was.v > 0.35 ? false : p > 0.5;
    setSidebar(open);
  };
  document.addEventListener("touchend", end);
  document.addEventListener("touchcancel", end);
})();

// ---- rendering -------------------------------------------------------------

const SPIN = ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];
let spinI = 0;
setInterval(() => {
  spinI = (spinI + 1) % SPIN.length;
  for (const s of document.querySelectorAll(".spinner")) s.textContent = SPIN[spinI];
}, 120);
const spinner = () => el("span", "spinner", SPIN[spinI]);

const running = (p) => /^(running|working)/i.test(p.value || "");
const needsInput = (p) => /needs|blocked|approval|input/i.test(p.value || "");

function latestUnread(w) {
  return state.notes.filter((n) => !n.read && n.workspace === w.ref).sort((a, b) => a.age_seconds - b.age_seconds)[0];
}

function render() {
  renderSidebar();
  renderMain();
  const unread = state.notes.filter((n) => !n.read).length;
  $("note-count").hidden = !unread;
  $("note-count").textContent = unread > 99 ? "99+" : String(unread);
  $("menu-dot").hidden = !state.workspaces.some((w) => w.unread && w.ref !== state.route.ws);
}

function renderSidebar() {
  const list = $("ws-list");
  const scroll = list.scrollTop;
  list.replaceChildren();
  if (state.loaded && !state.workspaces.length) {
    list.append(el("div", "sb-empty", "No workspaces open on the Mac."));
  }
  const groups = new Map(state.groups.map((g) => [g.ref, g]));
  let lastGroup = null;
  for (const w of state.workspaces) {
    if (w.group !== lastGroup) {
      lastGroup = w.group;
      if (w.group) list.append(groupHead(w.group, groups.get(w.group)));
    }
    if (w.group && state.closedGroups.has(w.group)) continue;
    list.append(wsRow(w));
  }
  list.scrollTop = scroll;
}

function groupHead(ref, g) {
  const members = state.workspaces.filter((w) => w.group === ref);
  const closed = state.closedGroups.has(ref);
  const b = el("button", "group-head" + (closed ? " closed" : ""));
  b.append(icon("chevron-down"), el("span", "", (g && g.name) || "Group"));
  if (closed && members.some((w) => w.unread)) b.append(el("span", "gdot"));
  if (closed && members.some((w) => (w.status || []).some(running))) b.append(spinner());
  b.onclick = () => {
    if (closed) state.closedGroups.delete(ref);
    else state.closedGroups.add(ref);
    store.set("closedGroups", [...state.closedGroups]);
    renderSidebar();
  };
  return b;
}

function wsRow(w) {
  const on = w.ref === state.route.ws;
  const b = el("button", "ws" + (on ? " on" : "") + (w.group ? " in-group" : ""));
  if (w.color) {
    const rail = el("span", "rail");
    rail.style.background = w.color;
    b.append(rail);
  }
  const top = el("div", "ws-top");
  if (w.unread) top.append(el("span", "unread-dot"));
  if ((w.status || []).some(running)) top.append(spinner());
  top.append(el("span", "ws-name", w.name || "Workspace"));
  b.append(top);

  for (const line of (w.description || "").split("\n").filter(Boolean).slice(0, 2)) {
    b.append(el("div", "ws-line", line));
  }
  // An agent's notification rides on its pill; another shows on its own.
  const note = latestUnread(w);
  const pillSurfaces = new Set((w.status || []).map((p) => p.surface).filter(Boolean));
  if (note && !pillSurfaces.has(note.surface)) {
    const n = el("div", "ws-note");
    n.append(icon("bell"), el("span", "", note.body || note.title));
    b.append(n);
  }
  if (w.status && w.status.length) {
    const pills = el("div", "pills");
    const agents = w.status.filter((p) => p.surface).length;
    for (const p of w.status) {
      const detail = p.surface && state.notes.find((n) => !n.read && n.surface === p.surface);
      const wide = detail && detail.body;
      const pill = el("span", "pill" + (needsInput(p) ? " needs" : "") + (wide ? " wide" : ""));
      const dot = el("i");
      dot.style.background = p.color || "var(--muted)";
      const key = p.surface && agents === 1 ? (p.key || "").split(" ")[0] : p.key;
      pill.append(dot, el("span", "key", `${key} · ${p.value}`));
      if (wide) pill.append(el("span", "detail", detail.body));
      pills.append(pill);
    }
    b.append(pills);
  }
  const where = [w.branch, tilde(w.cwd)].filter(Boolean);
  if (where.length) {
    const m = el("div", "ws-meta");
    if (w.branch) m.append(icon("git-branch"));
    m.append(el("span", "", where.join(" • ")));
    b.append(m);
  }
  b.onclick = () => {
    const last = store.get("tab:" + w.ref, null);
    go(w.ref, last);
    setSidebar(false);
  };
  return b;
}

function surfacesOf(w) {
  const out = [];
  (w.panes || []).forEach((p, i) => {
    for (const s of p.surfaces || []) out.push({ ...s, pane: p, paneIndex: i });
  });
  return out;
}

function kindOf(s) {
  if (s.claude) return "claude";
  if (s.type === "terminal") return "terminal";
  if (s.type === "browser" && /^abromux-md:/.test(s.url || "")) return "markdown";
  if (s.type === "browser") return "browser";
  return s.type || "other";
}

const KIND_ICON = { terminal: "terminal", claude: "claude", browser: "globe", markdown: "markdown", gitlab: "gitlab" };

// A terminal titled by its folder shows just the folder's name.
function tabTitle(s, kind) {
  if (s.label) return s.label;
  const t = s.title || "";
  if (kind === "terminal" && /^[~/]/.test(t) && !/\s/.test(t)) return t === "~" ? "~" : t.replace(/\/+$/, "").split("/").pop();
  return t || (kind === "claude" ? "Claude" : s.ref);
}

function renderMain() {
  const w = state.workspaces.find((x) => x.ref === state.route.ws);
  $("add-btn").style.visibility = w ? "" : "hidden";
  $("tabs").hidden = !w;
  if (!w) {
    $("title").textContent = "abromux";
    $("subtitle").textContent = "";
    showView("placeholder");
    closeTerm();
    closeChat();
    const missing = state.route.ws && state.loaded;
    placeholder("panel-left", missing ? "That workspace is gone" : "Pick a workspace",
      missing ? "It was closed on the Mac." : "Your Mac's workspaces are in the sidebar.",
      wide() ? null : { label: "Show workspaces", run: () => setSidebar(true) });
    if (!wide() && state.loaded && !state.route.ws && !renderMain.opened) {
      renderMain.opened = true;
      setSidebar(true);
    }
    return;
  }
  $("title").textContent = w.name || "Workspace";
  $("subtitle").textContent = w.branch || tilde(w.cwd);

  const surfaces = surfacesOf(w);
  let current = surfaces.find((s) => s.ref === state.route.surface);
  if (!current) {
    current = surfaces.find((s) => s.pane.focused && s.active) || surfaces[0];
    if (current) state.route.surface = current.ref;
  }
  if (current) store.set("tab:" + w.ref, current.ref);

  // Each tab's agent state, from the workspace's pills.
  const agent = new Map();
  for (const p of w.status || []) if (p.surface) agent.set(p.surface, p);

  const tabs = $("tabs");
  tabs.replaceChildren();
  let lastPane = -1;
  for (const s of surfaces) {
    if (lastPane !== -1 && s.paneIndex !== lastPane) tabs.append(el("span", "tab-sep"));
    lastPane = s.paneIndex;
    const kind = kindOf(s);
    const b = el("button", "tab" + (current && s.ref === current.ref ? " on" : ""));
    const p = agent.get(s.ref);
    const chatState = s.claude && s.claude.state;
    if ((p && running(p)) || chatState === "running") b.append(spinner());
    else if (p && p.surface && /claude/i.test(p.key || "")) b.append(icon("claude", "claude-ic"));
    else b.append(icon(KIND_ICON[kind] || "terminal", kind === "claude" ? "claude-ic" : ""));
    b.append(el("span", "", tabTitle(s, kind)));
    if ((p && needsInput(p)) || chatState === "waiting") {
      const d = el("span", "state");
      d.style.background = "var(--warn)";
      b.append(d);
    }
    b.onclick = () => go(w.ref, s.ref);
    tabs.append(b);
    if (current && s.ref === current.ref) requestAnimationFrame(() => b.scrollIntoView({ inline: "nearest", block: "nearest" }));
  }

  if (!current) {
    closeTerm();
    closeChat();
    showView("placeholder");
    placeholder("terminal", "No tabs", "This workspace has nothing open.", { label: "New tab", run: newTabSheet });
    return;
  }
  const kind = kindOf(current);
  if (kind === "terminal") {
    closeChat();
    showView("term-view");
    openTerm(current.ref);
  } else if (kind === "claude") {
    closeTerm();
    showView("chat-frame");
    openChat(current.ref);
  } else {
    closeTerm();
    closeChat();
    showView("placeholder");
    const url = current.url || "";
    const web = /^https?:/i.test(url);
    placeholder(KIND_ICON[kind] || "globe", current.title || "Tab",
      web ? url : kind === "markdown" ? "A Markdown tab. It shows on the Mac." : `A ${current.type} tab. It shows on the Mac.`,
      web ? { label: "Open on this phone", run: () => window.open(url, "_blank", "noopener") } : null);
  }
  if (w.unread) call("notification.mark_read", { surface: current.ref }).then(refreshSoon).catch(() => {});
}

function showView(id) {
  for (const v of ["term-view", "chat-frame", "placeholder"]) $(v).hidden = v !== id;
}

function placeholder(ic, title, text, action) {
  $("ph-icon").style.setProperty("--ic", `url(/icons/${ic}.svg)`);
  $("ph-icon").style.transform = ic === "panel-left" ? "scaleX(-1)" : "";
  $("ph-title").textContent = title;
  $("ph-text").textContent = text || "";
  const b = $("ph-action");
  b.hidden = !action;
  if (action) {
    b.textContent = action.label;
    b.onclick = action.run;
  }
}

// ---- sheets ------------------------------------------------------------------

function openSheet(build) {
  const root = $("sheet");
  root.classList.remove("closing");
  const body = $("sheet-body");
  body.replaceChildren();
  build(body);
  root.hidden = false;
}
function closeSheet() {
  const root = $("sheet");
  if (root.hidden) return;
  root.classList.add("closing");
  setTimeout(() => { root.hidden = true; root.classList.remove("closing"); }, 280);
}
document.querySelector(".sheet-scrim").onclick = closeSheet;
// Pull the sheet down by its grabber to close it.
(() => {
  let y0 = null;
  const sheet = document.querySelector(".sheet");
  sheet.addEventListener("touchstart", (e) => {
    const list = e.target.closest(".sheet-list");
    y0 = !list || list.scrollTop <= 0 ? e.touches[0].clientY : null;
  }, { passive: true });
  sheet.addEventListener("touchmove", (e) => {
    if (y0 == null) return;
    const dy = e.touches[0].clientY - y0;
    sheet.style.transform = dy > 0 ? `translateY(${dy}px)` : "";
  }, { passive: true });
  sheet.addEventListener("touchend", (e) => {
    if (y0 == null) return;
    const dy = e.changedTouches[0].clientY - y0;
    y0 = null;
    sheet.style.transform = "";
    if (dy > 90) closeSheet();
  });
})();

function actionSheet(title, actions) {
  openSheet((body) => {
    const wrap = el("div", "actions");
    const g = el("div", "action-group");
    if (title) g.append(el("div", "action-title", title));
    for (const a of actions) {
      const b = el("button", "action");
      if (a.icon) b.append(icon(a.icon));
      const t = el("div");
      t.append(el("div", "", a.label));
      if (a.desc) t.append(el("small", "", a.desc));
      b.append(t);
      b.onclick = () => { closeSheet(); a.run(); };
      g.append(b);
    }
    const cancel = el("div", "action-group");
    const c = el("button", "action cancel", "Cancel");
    c.onclick = closeSheet;
    cancel.append(c);
    wrap.append(g, cancel);
    body.append(wrap);
  });
}

function newTabSheet() {
  const w = state.workspaces.find((x) => x.ref === state.route.ws);
  if (!w) return;
  const make = async (type) => {
    try {
      const pane = (w.panes || []).find((p) => p.focused) || (w.panes || [])[0];
      const s = await call("surface.create", { workspace: w.ref, pane: pane && pane.ref, type, focus: false });
      await refresh();
      if (s && s.ref) go(w.ref, s.ref);
    } catch (e) { toast(e.message); }
  };
  actionSheet(w.name, [
    { icon: "claude", label: "New Claude chat", desc: "Claude Code in " + (tilde(w.cwd) || "this workspace"), run: () => make("claude") },
    { icon: "terminal", label: "New terminal", run: () => make("terminal") },
  ]);
}
$("add-btn").onclick = newTabSheet;

function age(s) {
  if (s < 60) return "now";
  if (s < 3600) return Math.floor(s / 60) + "m";
  if (s < 86400) return Math.floor(s / 3600) + "h";
  return Math.floor(s / 86400) + "d";
}

function notesSheet() {
  openSheet((body) => {
    const head = el("div", "sheet-head");
    head.append(el("h3", "", "Notifications"));
    const bell = el("button", "text-btn bell-btn");
    bell.id = "bell";
    bell.onclick = toggleAlerts;
    head.append(bell);
    showBell();
    if (state.notes.length) {
      const clear = el("button", "text-btn", "Clear All");
      clear.onclick = async () => {
        try { await call("notification.clear"); closeSheet(); refresh(); } catch (e) { toast(e.message); }
      };
      head.append(clear);
    }
    const list = el("div", "sheet-list");
    if (!state.notes.length) list.append(el("div", "sheet-empty", "Nothing new."));
    const wsName = new Map(state.workspaces.map((w) => [w.ref, w.name]));
    for (const n of state.notes) {
      const b = el("button", "note" + (n.read ? " read" : ""));
      const top = el("div", "note-top");
      if (!n.read) top.append(el("span", "unread-dot"));
      top.append(el("span", "note-title", n.title || "Notification"), el("span", "note-age", age(n.age_seconds || 0)));
      b.append(top);
      if (n.body) b.append(el("div", "note-body", n.body));
      if (wsName.get(n.workspace)) b.append(el("div", "note-ws", wsName.get(n.workspace)));
      b.onclick = () => {
        closeSheet();
        call("notification.mark_read", { id: n.id }).then(refreshSoon).catch(() => {});
        if (n.workspace) { go(n.workspace, n.surface); setSidebar(false); }
      };
      list.append(b);
    }
    body.append(head, list);
  });
}
$("open-notes").onclick = notesSheet;

// ---- terminal --------------------------------------------------------------

let term = null; // the open terminal view
let zoom = store.get("zoom", 0);

function termNote(text) {
  const n = $("term-note");
  n.textContent = text || "";
  n.hidden = !text;
}

function b64bytes(b64) {
  const s = atob(b64);
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

function closeTerm() {
  const t = term;
  if (!t) return;
  term = null;
  t.sub.cancel();
  t.xterm.dispose();
  $("term").replaceChildren();
}

function openTerm(surface) {
  if (term && term.surface === surface) return;
  closeTerm();
  termNote("Connecting…");
  const xterm = new Terminal({
    cols: 80,
    rows: 24,
    fontFamily: 'ui-monospace, Menlo, "SF Mono", monospace',
    fontSize: 11,
    scrollback: 5000,
    cursorBlink: false,
    macOptionIsMeta: true,
    theme: { background: "#101114" },
  });
  xterm.open($("term"));
  const t = { surface, xterm, cols: 80, rows: 24, sub: null };
  term = t;
  xterm.onData((d) => typeIn(surface, d));
  t.sub = subscribe("surface.stream", { surface }, {
    onStart(r) {
      termNote("");
      xterm.reset();
      xterm.options.theme = { background: r.bg, foreground: r.fg, cursor: r.cursor || r.fg };
      $("term-wrap").style.background = r.bg;
      t.cols = r.cols;
      t.rows = r.rows;
      t.alt = !!r.alt;
      xterm.resize(r.cols, r.rows);
      fit();
      xterm.write(r.vt);
    },
    onData(d) {
      if (d.output) {
        const bytes = b64bytes(d.output);
        xterm.write(bytes);
        // Started on a full-screen program's screen: the normal screen
        // under it never came, so take a fresh start once it's left.
        if (t.alt && /\x1b\[\?(1049|1047|47)l/.test(String.fromCharCode.apply(null, bytes.subarray(0, 65536)))) {
          t.alt = false;
          setTimeout(() => term === t && t.sub.restart(), 150);
        }
      } else if (d.resize) {
        [t.cols, t.rows] = d.resize;
        xterm.resize(t.cols, t.rows);
        fit();
      } else if (d.closed) {
        t.closed = true;
        termNote("This terminal closed.");
        refreshSoon();
      } else if (d.behind) t.sub.restart();
    },
    onEnd() {
      if (term === t && conn.up && !t.closed) setTimeout(() => term === t && t.sub.restart(), 300);
    },
    onError(e) { termNote(e.message); },
  });
}

// The Mac's terminal size stays; the font shrinks to fit it in the width,
// down to a floor, past which the view scrolls sideways. A/A adjust.
function fit() {
  const t = term;
  if (!t) return;
  const width = $("term-wrap").clientWidth - 6;
  const per = 0.6021; // Menlo's advance per point
  let size = Math.floor((width / (t.cols * per)) * 10) / 10;
  size = Math.max(7, Math.min(15, size)) + zoom;
  size = Math.max(5, Math.min(24, size));
  if (t.xterm.options.fontSize !== size) t.xterm.options.fontSize = size;
}
$("zoom-in").onclick = () => setZoom(zoom + 1);
$("zoom-out").onclick = () => setZoom(zoom - 1);
function setZoom(z) {
  zoom = Math.max(-4, Math.min(10, z));
  store.set("zoom", zoom);
  fit();
}
window.addEventListener("resize", fit);

// xterm's own input (a hardware keyboard, or typing with the terminal
// focused): keys it encodes become abromux keys, so they're sent in the
// modes the Mac's terminal is in; the rest is text.
const SEQ_KEYS = {
  "\r": "enter", "\x7f": "backspace", "\x1b": "escape", "\t": "tab",
  "\x1b[A": "up", "\x1b[B": "down", "\x1b[C": "right", "\x1b[D": "left",
  "\x1bOA": "up", "\x1bOB": "down", "\x1bOC": "right", "\x1bOD": "left",
  "\x1b[Z": "shift-tab", "\x1b[H": "home", "\x1b[F": "end",
  "\x1b[5~": "pageup", "\x1b[6~": "pagedown", "\x1b[3~": "delete",
};
function typeIn(surface, d) {
  if (SEQ_KEYS[d]) return key(surface, SEQ_KEYS[d]);
  if (d.length === 1 && d.charCodeAt(0) < 27) {
    return key(surface, "ctrl-" + String.fromCharCode(d.charCodeAt(0) + 96));
  }
  call("surface.send_text", { surface, text: d, raw: true }).catch((e) => toast(e.message));
}
function key(surface, k) {
  return call("surface.send_key", { surface, key: k }).catch((e) => toast(e.message));
}

for (const b of $("keys").querySelectorAll("button[data-key]")) {
  // Keep the keyboard up: don't take focus from the text box.
  b.addEventListener("mousedown", (e) => e.preventDefault());
  b.addEventListener("touchstart", () => {}, { passive: true });
  b.onclick = () => term && key(term.surface, b.dataset.key);
}
for (const b of document.querySelectorAll(".zoom button")) b.addEventListener("mousedown", (e) => e.preventDefault());

const input = $("input");
function autosize() {
  input.style.height = "auto";
  input.style.height = Math.min(input.scrollHeight, window.innerHeight * 0.3) + "px";
}
input.addEventListener("input", autosize);
input.addEventListener("keydown", (e) => {
  // Return sends with a hardware keyboard; ⇧Return is a new line.
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing && matchMedia("(pointer: fine)").matches) {
    e.preventDefault();
    $("composer").requestSubmit();
  }
});
$("send").addEventListener("mousedown", (e) => e.preventDefault());
$("composer").onsubmit = async (e) => {
  e.preventDefault();
  const t = term;
  if (!t) return;
  const text = input.value;
  try {
    if (text) await call("surface.send_text", { surface: t.surface, text, raw: true });
    await call("surface.send_key", { surface: t.surface, key: "enter" });
    input.value = "";
    autosize();
  } catch (err) {
    toast(err.message);
  }
};

// ---- Claude tabs: the Mac's chat page in a frame --------------------------
//
// chat.html is the Mac tab's own page; chat-shim.js in it hands what it
// posts to `chatPost` here, and its events come from `surface.stream`.

let chat = null; // { surface, frame window, sub }

// Each Claude tab gets a new frame. Reusing one, by clearing it and then
// setting a URL that differs only after the "#", can leave it blank: the
// second load counts as a jump within the page, which never loads the chat.
function freshFrame(src) {
  const old = $("chat-frame");
  const f = document.createElement("iframe");
  f.id = "chat-frame";
  f.className = "view";
  f.title = "Claude";
  f.hidden = old.hidden;
  if (src) f.src = src;
  old.replaceWith(f);
}

function closeChat() {
  if (!chat) return;
  chat.sub && chat.sub.cancel();
  chat = null;
  freshFrame(null);
}

function openChat(surface) {
  if (chat && chat.surface === surface) return;
  closeChat();
  chat = { surface, sub: null, win: null };
  freshFrame("/chat.html#" + encodeURIComponent(surface));
}

function chatRecv(batch) {
  const w = chat && chat.win;
  try { w && w.__chat && w.__chat.recv(batch); } catch (e) { console.error(e); }
}

window.__abromux = {
  chatPost(win, body) {
    const c = chat;
    if (!c || win !== $("chat-frame").contentWindow) return;
    let msg;
    try { msg = JSON.parse(body); } catch { return; }
    if (msg.chat === "ready") {
      // The page (re)loaded: start it off with the conversation so far.
      c.win = win;
      c.sub && c.sub.cancel();
      c.sub = subscribe("surface.stream", { surface: c.surface }, {
        onStart(r) { if (chat === c) chatRecv(r.events || []); },
        onData(d) {
          if (chat !== c) return;
          if (d.events) chatRecv(d.events);
          else if (d.closed) { toast("This Claude tab closed on the Mac."); refreshSoon(); }
          else if (d.behind) c.sub.restart();
        },
        onEnd() { if (chat === c && conn.up) setTimeout(() => chat === c && c.sub.restart(), 300); },
        onError(e) { toast(e.message); },
      });
      return;
    }
    // Links open on the phone, not as tabs on the Mac.
    if (msg.chat === "open") {
      if (/^https?:/i.test(msg.url || "")) window.open(msg.url, "_blank", "noopener");
      return;
    }
    call("surface.chat", { surface: c.surface, message: msg }).catch((e) => toast(e.message));
  },
};

// ---- keyboard: keep the bars above it ---------------------------------------

if (window.visualViewport) {
  const vv = window.visualViewport;
  const apply = () => {
    const kb = vv.height < window.innerHeight - 80;
    document.documentElement.style.setProperty("--vh", vv.height + "px");
    app.classList.toggle("kb", kb);
    window.scrollTo(0, 0);
    fit();
  };
  vv.addEventListener("resize", apply);
  apply();
}

// ---- phone notifications ---------------------------------------------------

let info = {};
let registration = null;

function b64urlBytes(s) {
  const b = atob(s.replace(/-/g, "+").replace(/_/g, "/") + "===".slice((s.length + 3) % 4));
  return Uint8Array.from(b, (c) => c.charCodeAt(0));
}

async function pushState() {
  if (!registration || !("PushManager" in window)) return "unsupported";
  if (Notification.permission === "denied") return "denied";
  const sub = await registration.pushManager.getSubscription();
  return sub ? "on" : "off";
}

async function showBell() {
  const s = await pushState().catch(() => "unsupported");
  const b = $("bell");
  if (!b) return;
  b.classList.toggle("on", s === "on");
  b.textContent = s === "on" ? "Alerts On" : "Turn On Alerts";
}

async function toggleAlerts() {
  const standalone = navigator.standalone || matchMedia("(display-mode: standalone)").matches;
  if (!registration || !("PushManager" in window)) {
    return toast(standalone ? "This browser can't get notifications." : "Add abromux to the Home Screen (Share › Add to Home Screen), then open it from there.", 4500);
  }
  try {
    const existing = await registration.pushManager.getSubscription();
    if (existing) {
      actionSheet("Alerts come when you're away from the Mac.", [
        { icon: "bell", label: "Send a test alert", run: () => fetch("/api/push/test", { method: "POST" }).then(() => toast("Sent.")) },
        { icon: "x", label: "Turn alerts off", desc: "On this phone", run: async () => {
          await fetch("/api/push/unsubscribe", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ endpoint: existing.endpoint }) }).catch(() => {});
          await existing.unsubscribe().catch(() => {});
          showBell();
        } },
      ]);
      return;
    }
    const perm = await Notification.requestPermission();
    if (perm !== "granted") return toast("Notifications weren't allowed.");
    if (!info.push_key) info = await (await fetch("/api/info")).json();
    const sub = await registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: b64urlBytes(info.push_key),
    });
    const r = await fetch("/api/push/subscribe", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(sub.toJSON()),
    });
    if (!r.ok) throw new Error((await r.json()).error || "subscribe failed");
    toast("Alerts are on: you'll get them when you're away from the Mac.", 3500);
  } catch (e) {
    toast(e.message);
  }
  showBell();
}

// ---- start -----------------------------------------------------------------

onConnect(refresh);

subscribe("events.subscribe", {}, {
  onData(ev) {
    if (!ev || !ev.event) return;
    if (ev.event === "command.finished" || ev.event === "surface.focused") return;
    refreshSoon();
  },
});

state.route = parseRoute();
if (!state.route.ws && !wide()) {
  const last = store.get("last", null);
  if (last && last.ws) state.route = last;
}
render();
connect();

fetch("/api/info").then((r) => r.json()).then((i) => {
  info = i;
}).catch(() => {});

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").then((r) => {
    registration = r;
    showBell();
  }).catch(() => showBell());
} else {
  showBell();
}

// A tap on a notification while the app is open.
navigator.serviceWorker && navigator.serviceWorker.addEventListener("message", (e) => {
  if (!e.data || !e.data.open) return;
  location.hash = e.data.open;
  setSidebar(false);
});
