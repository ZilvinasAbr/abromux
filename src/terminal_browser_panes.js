/* abromux: terminal-browser-panes v1 (begin) */
// Put into terminal-browser's main.js by abromux (`browser.terminalBrowserPanes`,
// src/terminal_browser_panes.rs): opens what would be a new terminal-browser
// tab as an abromux split beside the browser. `env` is the browser session's
// env, that of the `terminal-browser open` that made it, so of its own abromux
// pane; the daemon's process.env is shared by every pane and is the first
// one's. False (open a tab as usual) when it isn't in an abromux pane.
function __abxOpenPane(url, env) {
  try {
    if (!env || !env.ABROMUX_SOCKET_PATH || !env.ABROMUX_SURFACE_ID || !url) return false;
    const fs = require("node:fs");
    const home = require("node:os").homedir();
    const abromux =
      [env.ABROMUX_BIN, home + "/.cargo/bin/abromux", "/opt/homebrew/bin/abromux", "/usr/local/bin/abromux"].find(
        (p) => p && fs.existsSync(p),
      ) || "abromux";
    const tb = process.env.TERMINAL_BROWSER_DIST_ROOT
      ? process.env.TERMINAL_BROWSER_DIST_ROOT + "/bin/terminal-browser"
      : "terminal-browser";
    const q = (s) => "'" + String(s).replace(/'/g, "'\\''") + "'";
    const cmd =
      "TERMINAL_BROWSER_SKIP_GRAPHICS_CHECK=1 PIXEL_DISPLAY_SCALE=2 exec " + q(tb) + " open --no-merge " + q(url);
    const side = ["right", "left", "down", "up"].includes(env.TB_ABROMUX_SPLIT) ? env.TB_ABROMUX_SPLIT : "right";
    const args = ["--socket", env.ABROMUX_SOCKET_PATH, "new-split", side, "--surface", env.ABROMUX_SURFACE_ID, "--command", cmd];
    const child = require("node:child_process").spawn(abromux, args, {
      detached: true,
      stdio: "ignore",
      env: { ...process.env, ...env },
    });
    child.on("error", () => {});
    child.unref();
    return true;
  } catch {
    return false;
  }
}
/* abromux: terminal-browser-panes (end) */
