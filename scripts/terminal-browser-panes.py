#!/usr/bin/env python3
"""Make terminal-browser open new tabs as abromux panes instead of its own tabs.

Patches only app/browser/dist/main.js (outside the signed Electron .app), so
code signing, entitlements and WebAuthn are untouched.

    scripts/terminal-browser-panes.py            apply (idempotent; re-run after `terminal-browser upgrade`)
    scripts/terminal-browser-panes.py --revert   restore the backup
    scripts/terminal-browser-panes.py --check    report whether it is applied

What changes (only when terminal-browser runs inside abromux, i.e. ABROMUX_SOCKET_PATH
is set; otherwise behaviour is stock):
  * links that ask for a new tab (target=_blank, cmd/middle-click) -> new abromux split
  * context menu "Open link in new tab"                             -> new abromux split
  * the new-tab palette (cmd+t) submit                               -> new abromux split
window.open popups (OAuth / SSO / WebAuthn flows that talk to their opener) stay popups.
"""
import os
import shutil
import sys

ROOT = os.path.expanduser("~/.local/share/terminal-browser/app")
MAIN = os.path.join(ROOT, "browser/dist/main.js")
BACKUP = MAIN + ".orig"
MARK = "/* abromux-panes patch */"

HELPER = MARK + r"""
// `env` is the browser session's env: that of the `terminal-browser open` that
// made it, i.e. of its own abromux pane. (The daemon's process.env is shared by
// every pane and belongs to whichever started it first.)
function __abxOpenPane(url, env) {
  try {
    if (!env || !env.ABROMUX_SOCKET_PATH || !env.ABROMUX_SURFACE_ID || !url) return false;
    const fs = require("node:fs");
    const home = require("node:os").homedir();
    const abromux = [env.ABROMUX_BIN, home + "/.cargo/bin/abromux", "/opt/homebrew/bin/abromux", "/usr/local/bin/abromux"]
      .find((p) => p && fs.existsSync(p)) || "abromux";
    const tb = env.TERMINAL_BROWSER_DIST_ROOT ? env.TERMINAL_BROWSER_DIST_ROOT + "/bin/terminal-browser" : "terminal-browser";
    const q = (s) => "'" + String(s).replace(/'/g, "'\\''") + "'";
    const cmd = "TERMINAL_BROWSER_SKIP_GRAPHICS_CHECK=1 PIXEL_DISPLAY_SCALE=2 exec " + q(tb) + " open --no-merge " + q(url);
    const dir = env.TB_ABROMUX_SPLIT || "right";
    const args = ["--socket", env.ABROMUX_SOCKET_PATH, "new-split", dir, "--surface", env.ABROMUX_SURFACE_ID, "--command", cmd];
    const child = require("node:child_process").spawn(abromux, args, { detached: true, stdio: "ignore", env: { ...process.env, ...env } });
    child.on("error", () => {});
    child.unref();
    return true;
  } catch {
    return false;
  }
}
"""

# (original, replacement) pairs; each original must occur exactly once.
EDITS = [
    (
        '    this.create(details.url, details.disposition === "foreground-tab");\n'
        "    this.host.onTabOpened(tab2, details.url);\n"
        '    return "deny";\n',
        "    if (__abxOpenPane(details.url, this.__abxEnv)) return \"deny\";\n"
        '    this.create(details.url, details.disposition === "foreground-tab");\n'
        "    this.host.onTabOpened(tab2, details.url);\n"
        '    return "deny";\n',
    ),
    (
        '      case "open-link-tab":\n        this.tabs.create(menu.linkURL);\n',
        '      case "open-link-tab":\n        if (!__abxOpenPane(menu.linkURL, this.ctx.env)) this.tabs.create(menu.linkURL);\n',
    ),
    (
        "      if (text13.trim()) this.tabs.create(this.resolveInput(text13));\n",
        "      if (text13.trim()) { const u = this.resolveInput(text13); if (!__abxOpenPane(u, this.ctx.env)) this.tabs.create(u); }\n",
    ),
    (
        # hand each session's env to its TabManager, for openWindow above
        "      this.defaultUrl\n    );\n  }\n  lastZoom = 1;",
        "      this.defaultUrl\n    );\n    this.tabs.__abxEnv = ctx.env;\n  }\n  lastZoom = 1;",
    ),
]


def main():
    arg = sys.argv[1] if len(sys.argv) > 1 else ""
    src = open(MAIN, encoding="utf-8").read()
    applied = MARK in src

    if arg == "--check":
        print("applied" if applied else "not applied")
        return 0
    if arg == "--revert":
        if not applied:
            print("not applied; nothing to revert")
            return 0
        shutil.copy2(BACKUP, MAIN)
        print("reverted from", BACKUP)
        return 0
    if applied and arg == "--force":
        src = open(BACKUP, encoding="utf-8").read()  # re-apply a newer patch version
    elif applied:
        print("already applied (use --force to re-apply)")
        return 0

    out = src
    for old, new in EDITS:
        n = out.count(old)
        if n != 1:
            print(f"patch point found {n} times (expected 1); terminal-browser changed, not patching:\n{old}", file=sys.stderr)
            return 1
        out = out.replace(old, new)
    # after the leading "use strict"; so the helper is a plain top-level function
    head = '"use strict";\n'
    if not out.startswith(head):
        print("unexpected main.js header; not patching", file=sys.stderr)
        return 1
    out = head + HELPER + out[len(head):]

    if not applied:
        shutil.copy2(MAIN, BACKUP)
    tmp = MAIN + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(out)
    os.replace(tmp, MAIN)
    print("patched", MAIN, "(backup:", BACKUP + ")")
    return 0


if __name__ == "__main__":
    sys.exit(main())
