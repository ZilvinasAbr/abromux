/* abromux: terminal-browser-panes v2 (begin) */
// Put into terminal-browser's main.js by abromux (`browser.terminalBrowserPanes`,
// src/terminal_browser_panes.rs): opens what would be a new terminal-browser
// tab as an abromux browser tab in the browser's pane. `env` is the browser session's
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
    // A browser tab abromux makes itself, as for a link from a terminal: a
    // new tab in this browser's pane, or a split beside it on the side
    // TB_ABROMUX_SPLIT names.
    const page = ["--surface", env.ABROMUX_SURFACE_ID, "--type", "browser", "--url", url];
    const side = env.TB_ABROMUX_SPLIT;
    const args = ["right", "left", "down", "up"].includes(side)
      ? ["--socket", env.ABROMUX_SOCKET_PATH, "new-split", side, ...page]
      : ["--socket", env.ABROMUX_SOCKET_PATH, "new-surface", ...page];
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
