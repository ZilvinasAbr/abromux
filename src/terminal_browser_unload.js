/* abromux: terminal-browser-unload v1 (begin) */
// Put into terminal-browser's main.js by abromux (src/terminal_browser_panes.rs):
// a page with unsaved changes (a `beforeunload` handler) asks "Leave site?"
// on an abromux sheet over its tab before a navigation, reload or close.
// terminal-browser makes its pages with `disableDialogs` and doesn't handle
// Electron's `will-prevent-unload`, so the page could never be left and
// nothing said why. Outside an abromux pane it does what it did before.
const __abxSessions = new Set();
function __abxTrackSession(session) {
  __abxSessions.add(new WeakRef(session));
}
// The env of the `terminal-browser open` whose tabs hold `contents`.
function __abxEnvFor(contents) {
  for (const ref of __abxSessions) {
    const session = ref.deref();
    if (!session) {
      __abxSessions.delete(ref);
      continue;
    }
    try {
      if (session.tabs.all().some((tab) => tab.ref.current?.webContents === contents)) return session.ctx.env;
    } catch {}
  }
  return null;
}
// `will-prevent-unload`: preventDefault leaves the page; otherwise it stays.
// Electron needs the answer before the handler returns, so this waits for
// it, as Chrome's own dialog does.
function __abxConfirmUnload(event, contents) {
  try {
    const env = __abxEnvFor(contents);
    if (!env || !env.ABROMUX_SOCKET_PATH || !env.ABROMUX_SURFACE_ID) return;
    const fs = require("node:fs");
    const home = require("node:os").homedir();
    const abromux =
      [env.ABROMUX_BIN, home + "/.cargo/bin/abromux", "/opt/homebrew/bin/abromux", "/usr/local/bin/abromux"].find(
        (p) => p && fs.existsSync(p),
      ) || "abromux";
    const args = ["--socket", env.ABROMUX_SOCKET_PATH, "confirm", "--json", "--surface", env.ABROMUX_SURFACE_ID];
    args.push("--info", "Changes you made may not be saved.", "--button", "Leave", "Leave site?");
    const out = require("node:child_process").spawnSync(abromux, args, {
      encoding: "utf8",
      env: { ...process.env, ...env },
    });
    if (out.status === 0 && JSON.parse(out.stdout).confirmed === true) event.preventDefault();
  } catch {}
}
/* abromux: terminal-browser-unload (end) */
