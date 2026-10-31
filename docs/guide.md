# AbroMux guide

Everything AbroMux does, in detail. For a quick start, see the [README](../README.md).

## Features

- **Terminal panes** use [libghostty-vt](https://github.com/ghostty-org/ghostty) for all terminal state: VT parsing, screen and scrollback, colors, selection and key encoding. It is linked statically through a bindgen FFI layer. A shell runs in a PTY (`portable-pty`) and GPUI draws each cell. Text goes through the macOS input system, so IMEs and dead keys compose as usual. Mouse clicks, drags and the wheel go to TUI apps that ask for them (vim, htop, lazygit); hold Shift to select or scroll locally. Full-screen apps without mouse support get arrow keys from the wheel, like Ghostty's alternate scroll.
- **Browser panes** are native WKWebViews (wry, via `gpui-component`). Each has an address bar with back, forward and reload. Typing in the address bar suggests pages from your browsing history, ranked as in cmux (where the text matches, plus how recently, often and directly you've visited them). ↑/↓ pick one, Return opens it, Esc closes the list, ⇧⌫ deletes the highlighted entry, and ↓ on the page's own URL lists your most typed and recent ones. History is shared by all browser panes and kept in `browser-history.json` under Application Support. ⌘-click or middle-click on a link opens it in a new tab after the current one (⇧ to switch to it). Passkeys and security keys work in them once the app is signed with Apple's web-browser entitlement (see "Passkeys").
- **Workspaces** sit in a left sidebar, cmux style. Each entry shows the name, the focused tab's title, the git branch and the cwd, plus whatever scripts report through the CLI: status pills (`set-status`), a progress bar (`set-progress`) and the newest `log` line.
- **Coding agents** (Claude Code, Codex, Gemini CLI, Droid, Cursor CLI, OpenCode and Pi) show up in the sidebar once you've run `abromux hooks install`: a "Running" pill while the agent works and "Needs input" when it asks for permission (Cursor's hooks can't say when it asks). When it asks, or finishes a turn, its terminal gets a notification ("Codex: Asks to run …", "Claude Code: <the start of its reply>"). Their tabs show the agent's logo (Claude Code and Codex) or a robot.
- **Notifications** come from terminal programs (OSC 9, OSC 99 and OSC 777, as Claude Code, Codex and others send them), from the bell, and from `abromux notify`. One you aren't looking at rings its pane, puts a dot on its tab and on its workspace, shows its text under the workspace in the sidebar, counts on the dock icon, and, while AbroMux is in the background, shows a macOS banner that takes you to it when clicked (banners need AbroMux.app; a `cargo run` build has the rest). It's read once you look at that terminal. ⌘I lists them all; ⌘⇧U jumps to the newest unread one.
- **Links** in terminal output underline while you hold ⌘ over them: URLs, OSC 8 hyperlinks, and paths to files that exist (relative ones from the shell's directory, `path:line` too). ⌘-click opens web links in a browser to the right of the terminal (a new tab there if that pane already shows one) and anything else with its default app.
- **Panes** split right or down, and each pane has a tab bar of terminal and browser tabs. Drag a divider to resize. Drag a tab to reorder it, onto another pane's tab bar or middle to move it there, onto a pane's edge to split that pane, or onto the outer edge of the pane area to split the whole workspace (full width or height). A highlight shows where it will land.

## Build from source

Requirements:

- macOS on Apple Silicon (the libghostty build script fetches an aarch64 Zig)
- Rust stable (pinned via `rust-toolchain.toml`)
- Xcode Command Line Tools (clang for bindgen, `codesign`)

Build and run:

```bash
./scripts/build-libghostty.sh   # once: local Zig 0.17 + libghostty-vt into vendor/
cargo run --release
```

`ABROMUX_DEMO=1 cargo run` opens with a sample layout.

To install it as a Mac app:

```bash
scripts/bundle-app.sh                                          # builds /Applications/AbroMux.app
ln -sf /Applications/AbroMux.app/Contents/MacOS/abromux ~/.cargo/bin/abromux   # the CLI on PATH
```

Run from the bundle, a bare `abromux` brings the running app to the front, or opens AbroMux.app if it isn't running. A dev build (`cargo run`) always starts its own app.

### Nightly build

To try the working tree as an app without replacing the release AbroMux.app:

```bash
scripts/nightly.sh                 # builds /Applications/AbroMux Nightly.app (or pass another folder)
abromux-nightly list-workspaces    # its CLI, symlinked into ~/.cargo/bin
scripts/nightly.sh --name zoom     # "AbroMux Nightly zoom.app" and `abromux-nightly-zoom`
```

A named nightly (bundle id `dev.abromux.nightly.<name>`, support folder `abromux-nightly-<name>/`) runs beside the release and the shared nightly, so agents testing different branches don't replace each other's build.

AbroMux Nightly runs next to AbroMux. It has a neon icon (`assets/app/abromux-nightly.svg`), its own name, the bundle id `dev.abromux.nightly` (so separate notification permissions and settings), and its own socket and session in `~/Library/Application Support/abromux-nightly/`. It's signed like the release but never notarized: `scripts/bundle-app.sh --nightly` is what `nightly.sh` calls, and it refuses `--dist`. The binary decides which one it is at runtime from the bundle it sits in (`src/channel.rs`); `cargo run` counts as the release. Install agent hooks from the release `abromux`, not `abromux-nightly`: hooks point at the binary you install them from, and do nothing once that app is gone.

## Passkeys

Browser panes sign in with passkeys (iCloud Keychain, a phone over Bluetooth) and security keys, the way cmux does. WKWebView only allows WebAuthn for an app's own associated domains, so `src/webauthn.js` replaces `navigator.credentials.create/get` for public-key requests in the top frame and hands them to `src/webauthn.m`, which runs them through AuthenticationServices' web-browser APIs with the page's origin and challenge, and passes the credential back. Conditional (autofill) requests stay pending, since there's no autofill UI; sites show their "Sign in with a passkey" button instead. Frames and U2F `appid` requests are left to WebKit.

The web-browser APIs need the restricted `com.apple.developer.web-browser.public-key-credential` entitlement, which Apple grants per App ID and only a provisioning profile can carry. Without it the bridge stays out of the way and WebKit handles WebAuthn as before (for most sites, not at all). To turn it on:

1. In the Apple Developer portal, under Certificates, Identifiers & Profiles, register explicit App IDs `dev.abromux.app` and `dev.abromux.nightly`, and request the "Web Browser Public Key Credential Requests" capability for each (a request Apple reviews).
2. Once it's granted, make a Developer ID provisioning profile for each App ID with your Developer ID Application certificate.
3. Save them as `assets/app/dev.abromux.app.provisionprofile` and `assets/app/dev.abromux.nightly.provisionprofile` (or point `$ABROMUX_PROVISION_PROFILE` at one), then run `scripts/nightly.sh` or `scripts/bundle-app.sh`. It embeds the profile and signs its entitlements in, and says "Passkeys on".

The first passkey request asks to let AbroMux use passkeys (System Settings › Privacy & Security › Passkey Access for Web Browsers); a request that may use a phone also asks for Bluetooth. If passkeys are denied, security keys still work.

## Session restore

Quitting saves the session to `~/Library/Application Support/abromux/session.json` (the layout is also saved every few seconds, so a crash loses little). The next launch restores the window frame, workspaces with their names, descriptions, colors and groups, the split layout and ratios, tabs and their names, each terminal's working directory and font size, browser pages with their back/forward history (saved on quit), and up to 2000 lines of each terminal's output (with colors). Shells start fresh: running programs aren't kept alive. Coding agents' sessions are the exception. Run `abromux hooks install` once and abromux learns, through the agents' hooks, which session runs in which terminal (the same hooks feed the sidebar status and notifications; rerun `install` after updating abromux to pick up new hook events). On restore, it types the agent's resume command into that terminal's new shell: `claude --resume <id>` (with the same permission mode), `codex resume <id>`, `gemini --resume <id>`, `droid --resume <id>`, `cursor-agent --resume <id>`, `opencode --session <id>` or `pi --session <id>`. A Codex, OpenCode or Pi you had quit isn't resumed. The hooks go after any hooks already there and do nothing outside abromux: `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`), `~/.codex/hooks.json` (or `$CODEX_HOME`), `~/.gemini/settings.json`, `~/.factory/hooks.json` (or the `hooks` in `~/.factory/settings.json` when there's no hooks.json) and `~/.cursor/hooks.json`. OpenCode and Pi get a plugin file instead: `~/.config/opencode/plugins/abromux.js` and `~/.pi/agent/extensions/abromux.js`. Agents whose folder doesn't exist are skipped; `abromux hooks install gemini pi` does just those. Codex asks you once to review new hooks: trust them when it does. They point at the binary you ran `install` from, so run it from the installed app; `abromux hooks uninstall` removes them. `ABROMUX_NO_RESTORE=1` skips it; `ABROMUX_SESSION_PATH` uses another file. `ABROMUX_DEMO` and `ABROMUX_SCRIPT` runs don't touch the session unless `ABROMUX_SESSION_PATH` is set.

Reopen Previous Session (⌘⇧O, File menu, or `abromux restore-session`) brings back the workspaces of the session this launch restored that aren't open any more, such as ones you closed by mistake. Workspaces still open as they were are skipped, so it never doubles one. `abromux restore-session --from nightly` (or `stable`, or a file) adds another session's workspaces, for example after trying the nightly app; `abromux restore-session --export <file> [--force]` writes the current one to a file.

## Sidebar

Each workspace's row shows its name, a description if it has one, the focused tab's title, the latest unread notification, status pills, progress and the newest log line from scripts, and the git branch and directory. Right-click a row for its menu: rename, add or edit a description, pick a color, move it, put it in a group, or close it (or every other one).

- **Description**: ⌥⌘E (or right-click → Add Description…) edits it under the name. Return saves, ⇧↩ adds a line, Esc cancels, and an empty one clears it.
- **Color**: right-click → Color, from cmux's sixteen (Red, Crimson, Orange, Amber, Olive, Green, Teal, Aqua, Blue, Navy, Indigo, Purple, Magenta, Rose, Brown, Charcoal) or your own palette (`workspaceColors.colors`). It shows as a bar down the row's left edge, or fills the row with `"indicatorStyle": "solidFill"`. A layout's `"color"` sets it too.
- **Order**: drag a row to move it. A line shows where it will land, between two rows; between two of a group's workspaces puts it in that group, and above a group's header keeps it out. Dropping it on a header makes it the group's first. Right-click → Move Up / Move Down / Move to Top, or ⌘⇧P → Move Workspace Up / Down. ⌘1…⌘9 follow the order.
- **Groups**: a header above workspaces that belong together. Right-click a row → Group → New Group starts one (named after the repository), or ⌘⇧P → Group Workspaces by Folder groups every workspace that shares a repository or folder with another. Click a header to fold it (a folded group still shows the selected workspace, and a dot if one has an unread notification), double-click to rename it, drag it to move the whole group, and use its `+` for a new workspace in the group's folder. Right-click a header to rename, color, ungroup or close it. ⌘N from a workspace in a group puts the new one in the group.
- **Auto-naming** (off by default): with `"automation": { "workspaceAutoNaming": true }`, the first prompt you give a coding agent in a workspace is turned into a short name by that agent's own CLI (`claude -p` on Haiku, or `codex exec`), once per workspace. That runs a request on your account. A name you give the workspace always wins, and without one the name otherwise follows the agent's terminal title.

## Settings window

⌘, (or the app menu › Settings…) opens Settings: a page each for General, Sidebar, Workspace Colors, Panes, Keyboard Shortcuts and Commands, with a search field over all of them. It's a front end for the config file below, not a separate store. Each change writes just that key to `config.json` and applies at once. A setting put back to its default is removed from the file, and the rest of the file (other keys, their order, commands) stays as you wrote it. The undo arrow next to a setting resets it, and the one by a page's title resets the whole page.

On Keyboard Shortcuts, click a shortcut and press the new keys; Esc cancels. A shortcut needs ⌘, ⌃ or ⌥, unless it's a function key. If another action had that key, a toast says the new one takes it over. × removes a shortcut. Chords (`ctrl-a h`), per-context keys (`"when"`), workspace palette colors and commands are edited in the file; Settings shows them, and General › Config Files opens the file. If the file doesn't parse, Settings says why and saves nothing until it's fixed.

## Config file

abromux's own settings live in `~/.config/abromux/config.json` (or `$XDG_CONFIG_HOME/abromux/config.json`), shared by the release and nightly apps. Every key is optional:

```json
{
  "panes": {
    "resizeStep": 20,
    "dividerColor": "#3a3935",
    "focusBorderColor": "#c96442b0"
  }
}
```

`resizeStep` is how many pixels ⌃⇧H/J/K/L and `resize-pane` move a divider. `dividerColor` is the line between split panes, and `focusBorderColor` the border around the focused one. Colors are `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`. Unknown keys are errors, so a typo doesn't go unnoticed.

### Browser engine

```json
{ "browser": { "engine": "terminalBrowser" } }
```

`engine` is `builtIn` (the default, the WebKit browser pane) or `terminalBrowser` (`"terminal-browser"` works too); Settings has it under General › Browser. With `terminalBrowser`, every new browser tab (⌘⇧L, the globe, ⌘-clicked links, `url` commands, layouts, `abromux new-surface --type browser`) is a terminal running `terminal-browser open --no-merge <url>` in your login shell, and closing the browser closes the tab. abromux looks for `terminal-browser` on PATH, in `~/.local/bin`, `/opt/homebrew/bin` and `/usr/local/bin`; if it isn't there, the tab falls back to the built-in browser with a warning. The setting applies to tabs opened after it changes; open ones stay as they are. A terminal-browser tab is restored as a browser tab on the page it opened, since abromux doesn't see where it went from there. The `abromux browser` automation commands only drive built-in tabs, so `abromux browser open` always opens one (use `terminal-browser action` to drive a terminal-browser).

### Sidebar and workspace settings

These use cmux's key names. Shown with their defaults:

```json
{
  "app": {
    "workspaceInheritWorkingDirectory": true,
    "newWorkspacePlacement": "end"
  },
  "sidebar": {
    "fontSize": 12.5,
    "hideAllDetails": false,
    "wrapWorkspaceTitles": false,
    "showWorkspaceDescription": true,
    "workspaceDescriptionColor": null,
    "showNotificationMessage": true,
    "showBranchDirectory": true,
    "stackBranchDirectory": false,
    "pathLastSegmentOnly": false,
    "showProgress": true,
    "showLog": true,
    "showCustomMetadata": true,
    "selectionColor": null
  },
  "sidebarAppearance": { "tintColor": null, "tintOpacity": 0.18 },
  "workspaceColors": {
    "indicatorStyle": "leftRail",
    "selectionColor": null,
    "unselectedFillOpacity": 0.7,
    "colors": { "Red": "#C0392B", "Blue": "#1565C0" }
  },
  "workspaceGroups": { "newWorkspacePlacement": "afterCurrent" },
  "automation": { "workspaceAutoNaming": false }
}
```

- `workspaceInheritWorkingDirectory`: ⌘N and the `+` start in the focused terminal's directory; `false` starts in your home folder.
- `newWorkspacePlacement`: where a new workspace goes, `top`, `afterCurrent` (after the selected one, or after its group) or `end`. `workspaceGroups.newWorkspacePlacement` is the same inside a group, where `top` means right after the group's first workspace.
- `fontSize` sizes the workspace names (8–24); the smaller lines scale with it. `hideAllDetails` shows names only. `wrapWorkspaceTitles` lets long names wrap.
- `showWorkspaceDescription`, `showNotificationMessage`, `showBranchDirectory`, `showProgress`, `showLog` and `showCustomMetadata` (status pills) hide those lines when `false`. `stackBranchDirectory` puts the branch and the directory on lines of their own, and `pathLastSegmentOnly` shows `…/api` instead of `~/code/api`.
- `selectionColor` (in `sidebar` or `workspaceColors`) is the selected row's fill; it's the accent color by default. `sidebarAppearance.tintColor` lays a tint of that opacity over the sidebar.
- `workspaceColors.colors` replaces the palette the Color menu offers and `set-color` names (cmux's sixteen by default). `indicatorStyle` is `leftRail` or `solidFill`; with `solidFill`, rows other than the selected one show their color at `unselectedFillOpacity`.

### Shortcuts and commands

`"shortcuts"` changes the keys of any action, by name. `abromux config shortcuts` lists every name with its keys as they are now, marking the ones your file sets:

```json
{
  "shortcuts": {
    "splitRight": "cmd-\\",
    "closeTab": null,
    "lastPane": "ctrl-a l",
    "flashPane": ["⌘⇧H", "ctrl-a f"],
    "find": { "keys": "cmd-shift-f", "when": "Terminal" }
  },
  "commands": [
    { "title": "Run Tests", "run": "cargo test", "shortcut": "cmd-alt-t" },
    { "title": "Dev Server", "run": "npm run dev", "newTab": true },
    { "title": "Open Docs", "url": "https://docs.rs" }
  ]
}
```

- A shortcut is a keystroke like `cmd-shift-p` (modifiers `cmd`, `alt`/`option`, `ctrl`, `shift`) or `⌘⇧P`, or several separated by spaces for a two-step chord (`ctrl-a l`). A list gives an action several; `null` or `[]` unbinds it.
- Setting an action's keys replaces all of its defaults. A key you give one action is taken from whatever else had it by default, so `"splitRight": "cmd-k"` also frees ⌘K from Clear Scrollback.
- `when` binds it only in a key context: `Terminal`, `Browser`, `Browser > Input` (the address bar and find field), or a gpui context expression. By default an action keeps the contexts its defaults had (find works in terminals and browsers), and a command's shortcut works everywhere.
- Shift with punctuation is written either way: `cmd-shift-]` and `cmd-}` are the same key.

`"commands"` adds your own entries to the command palette (⌘⇧P), each with an optional `shortcut`. `run` types the text into the focused terminal and presses Return; with `"newTab": true`, or when a browser has focus, it runs in a new terminal tab instead. `url` opens a page in a new browser tab. `description` shows under the title in the palette, `keywords` are more words it finds the command by, and `"confirm": true` asks before every run.

`workspace` opens a new workspace built from a layout, in cmux's format:

```json
{ "title": "Web Dev", "restart": "ignore", "workspace": {
  "name": "Web", "cwd": "~/code/web", "env": { "PORT": "3000" }, "setup": "nvm use",
  "layout": { "direction": "horizontal", "split": 0.6, "children": [
    { "pane": { "surfaces": [
      { "name": "Dev", "command": "npm run dev", "focus": true },
      { "name": "Tests", "cwd": "./tests" }
    ] } },
    { "pane": { "surfaces": [{ "type": "browser", "url": "localhost:3000" }] } }
  ] }
} }
```

- A layout node is a split (`direction` `horizontal` for side by side or `vertical` for stacked, `split` the first child's share from 0.1 to 0.9, and exactly two `children`) or a `pane` with its tabs (`surfaces`). Without `layout` it's one terminal.
- A tab is a `terminal` (the default `type`) or a `browser`. A terminal can have a `command` to run, a `cwd` and an `env`; a browser has a `url`. `name` names the tab, and `focus` picks the tab that has focus when it's built.
- `cwd` is `~/…` under your home folder, `/…` absolute, or relative: a tab's to the workspace's, and the workspace's to the project (below) or the focused terminal's directory.
- `env` on the workspace goes to every shell in it; `setup` is typed into the first terminal before its command.
- `restart` says what to do when a workspace of that name is already open: `new` opens another (the default), `ignore` switches to it, `recreate` closes it and builds it again in its place, and `confirm` asks first.
- `abromux new-workspace --layout '<json>'` (or `--layout @layout.json`) builds a layout node from the CLI, with `--name` and `--cwd`.

A project can have its own commands. The palette looks from the focused terminal's directory upward for `.abromux/config.json`, or cmux's `.cmux/cmux.json` or `cmux.json`, and adds the `"commands"` of the first it finds (other keys in it are ignored). cmux's `name` and `command` work for `title` and `run`, so a `cmux.json` works as it is. A project command replaces one of yours with the same title, its palette entry names the project, and it has no shortcut. Since anyone who can commit to a repository can add one, a project command asks before it runs the first time, showing what it types; after you say Run it doesn't ask again until the command changes. `abromux config project` shows the project file found from the current directory and which of its commands will ask.

Reloading the config applies new shortcuts at once, in the menus too.

⌘⇧, (or ⌘⇧P → Reload Config, or `abromux reload-config`) reads the file again without a restart. If it doesn't parse, the current settings stay and a toast shows the error with its line and column, and it does the same at launch. `abromux config check [<file>]` validates a file without the app, and `abromux config path` prints where it's read from. The Ghostty config below is separate and is still read only at launch.

## Ghostty config

Terminals read your Ghostty config: `~/.config/ghostty/config` (or `$XDG_CONFIG_HOME`), then `~/Library/Application Support/com.mitchellh.ghostty/config`, plus their `config-file` includes. abromux uses `theme` (from `~/.config/ghostty/themes` or Ghostty.app), `font-family`, `font-size`, `foreground`, `background`, `palette`, `cursor-color`, `cursor-text`, `cursor-style`, `selection-background`/`-foreground`, `keybind`s whose action is `text:`, `csi:` or `esc:`, and `clipboard-paste-protection` / `clipboard-paste-bracketed-safe` (on by default, as in Ghostty: pasting text that could run commands, such as a newline when the program hasn't turned on bracketed paste, asks first; Enter pastes, Esc cancels). `copy-on-select = clipboard` copies a selection to the clipboard when you let go of the mouse (Ghostty's default `true` uses a selection pasteboard macOS doesn't have, so it does nothing here). `mouse-scroll-multiplier` scales scrolling, as one number or `precision:<n>,discrete:<n>` for the trackpad and the mouse wheel. `scrollbar = never` hides the overlay scrollbar. `shell-integration = none` turns off abromux's shell integration (below). Other keys are ignored for now. Changes apply to new terminals.

## Shell integration

zsh starts with a small integration (`src/shell/zsh`, written to `~/Library/Application Support/abromux/shell-integration/zsh`) that marks the prompt with OSC 133. abromux sets `ZDOTDIR` to that folder; its `.zshenv` puts your `ZDOTDIR` back, runs your `.zshenv`, and zsh reads the rest of your config as usual. With the marks, resizing a pane blanks the prompt for zsh to redraw instead of rewrapping it, which used to pile up pieces of the prompt line. One copy can still stay behind when a narrower pane wraps the prompt, until the pane widens again: since Ghostty `dde3d4d6b`, libghostty-vt clears the prompt after the reflow, from its last wrapped row only (Ghostty 1.3.1 cleared first). A `zsh` started inside a pane doesn't get the integration. Other shells have none yet.

## CLI

The same `abromux` binary controls a running app over a Unix socket (`~/Library/Application Support/abromux/abromux.sock`, or `$ABROMUX_SOCKET_PATH`). Command names follow cmux's CLI; `abromux --help` lists them all.

```bash
abromux ~/code/project                      # new workspace there (launches abromux if needed)
abromux list-workspaces
abromux tree --all
abromux new-split right --command 'cargo watch -x test'
abromux send --surface surface:3 'ls -la\n'
abromux send-key --surface surface:3 ctrl-c
abromux read-screen --surface surface:3 --scrollback --lines 50
abromux rename-tab --surface surface:3 'dev server'   # no title: back to the program's own
abromux respawn-pane --surface surface:3 --command 'npm run dev'   # kill the shell, start a fresh one
abromux last-pane                           # back to the pane focused before
abromux swap-pane -L                        # swap the focused pane's tabs with the pane to its left
abromux browser open https://example.com
abromux browser eval 'document.title'
abromux notify --title 'Tests' --body 'all green'   # rings the pane it runs in
abromux list-notifications
abromux jump-to-unread
abromux set-status build compiling --color '#e0a84f'   # a pill under the workspace
abromux set-progress 0.4 --label 'Building'
abromux log --level error --source build 'Build failed'
abromux workspace-action set-description 'Ship the release'   # cmux's workspace actions
abromux workspace-action --workspace workspace:2 set-color Teal
abromux reorder-workspace --workspace workspace:4 --index 0
abromux group create --name api             # a sidebar group holding this workspace
abromux group add --group group:1 --workspace workspace:5
abromux new-workspace --cwd ~/code/api --group group:1 --description 'Fix the 500s'
abromux restore-session --from nightly
abromux --json list-panes
```

`abromux browser` also automates pages, for agents and scripts. `snapshot` prints the page as an accessibility tree with a ref per element, and the action commands take a ref from the latest snapshot (`e3`) or a CSS selector:

```bash
abromux browser snapshot --interactive        # - textbox "Username" [ref=e2] ...
abromux browser fill e2 'alice@example.com'
abromux browser click 'button[type=submit]'
abromux browser wait --text 'Welcome' --timeout 5000
abromux browser get text '#result'
abromux browser screenshot page.png
```

There are also `dblclick`, `hover`, `focus`, `check`/`uncheck`, `type`, `press`, `select`, `scroll`, `get`, `is`, `find` (by role, text, label, placeholder, alt, title or test id) and `wait` for selectors, URLs and load states. The events are synthesized in the page with JavaScript, so sites that only react to real input won't respond to them.

Coding agents running in abromux can learn this CLI from two agent skills in [`skills/`](../skills): `abromux` (splits, tabs, running and reading other terminals, sidebar status and notifications) and `abromux-browser` (the browser automation above). `abromux skills install` copies them into `~/.claude/skills` (or `$CLAUDE_CONFIG_DIR`) and, if Codex is set up, `~/.codex/skills` (or `$CODEX_HOME`). They're built into the binary, so rerun it after updating abromux; `abromux skills uninstall` removes them, and `abromux skills show [name]` prints one.

Targets are refs (`workspace:2`, `pane:5`, `surface:3`) or 0-based indexes. Each shell gets `ABROMUX_SOCKET_PATH`, `ABROMUX_WORKSPACE_ID` and `ABROMUX_SURFACE_ID`, so `abromux` run inside a pane acts on that pane by default (otherwise on what's focused). The wire format is one JSON object per line (`{"id":1,"method":"workspace.list","params":{}}`); `abromux rpc <method> [json]` sends one directly.

## Keyboard and mouse

| | Mouse | Keys |
|---|---|---|
| New workspace | `+` in sidebar header / footer | ⌘N |
| Rename workspace | double-click its name (Enter saves, Esc cancels) | ⌘⇧R |
| Workspace description | right-click → Add Description… (Return saves, ⇧↩ new line) | ⌥⌘E |
| Workspace color, group, move, close others | right-click the row | |
| Reorder workspaces | drag a row (onto a group to join it) or a group header | ⌘⇧P → Move Workspace Up / Down |
| Fold a group | click its header (double-click renames, right-click for more) | ⌘⇧P → Collapse or Expand Group |
| Reopen workspaces closed since launch | | ⌘⇧O |
| Delete workspace | hover → `×` | ⌘⇧W |
| Select workspace | click | ⌘1…⌘8, ⌘9 for the last, ⌃⌘[ / ⌃⌘] (hold ⌘ to see the numbers) |
| New terminal tab | terminal icon in pane tab bar | ⌘T |
| New browser tab | globe icon | ⌘⇧L (⌘L focuses address bar) |
| Split right / down | split icons | ⌘D / ⌘⇧D |
| Move tab / split by dragging | drag a tab onto a tab bar, a pane's edge or the window's edge | |
| Rename tab | double-click it (Enter saves, Esc cancels; an empty name goes back to the program's title) | ⌘R (in a browser tab ⌘R reloads; use ⌘⇧P → Rename Tab…) |
| Close tab | tab `×` / middle-click | ⌘W (last tab closes the pane; last pane closes the workspace) |
| Cycle tabs | | ⌘⇧[ / ⌘⇧], ⌃Tab / ⌃⇧Tab |
| Select tab in focused pane | click | ⌃1…⌃8, ⌃9 for the last (hold ⌃ to see the numbers) |
| Focus pane in a direction | click | ⌥⌘← → ↑ ↓ |
| Focus the previously focused pane | | ⌘⇧P → Focus Last Pane |
| Swap the focused pane with its neighbour | | ⌘⇧P → Swap Pane Left / Right / Up / Down (focus moves with it) |
| Resize focused pane | drag a divider | ⌃⇧H / J / K / L (20 px a press, `panes.resizeStep` in the [config file](#config-file)) |
| Flash focused pane | | ⌘⇧H |
| Zoom focused pane to fill the workspace, or unzoom | | ⌘⇧↩ (focusing another pane unzooms) |
| Command palette: every command that applies to the focused terminal or browser, with its shortcut | | ⌘⇧P, type to filter, ↑/↓ or ⌃N/⌃P, Enter runs, Esc closes |
| Switch workspace by name | | ⌘P |
| Notifications list | bell icon in the titlebar | ⌘I (Esc closes) |
| Jump to newest unread notification | click it in the list | ⌘⇧U |
| Open a link in a terminal | ⌘-click | |
| Open a page link in a new tab | ⌘-click (⇧⌘-click to switch to it) | |
| Toggle sidebar | sidebar icon | ⌘B |
| Copy / paste in terminal | drag to select, double-click selects a word (with `copy-on-select = clipboard` it's copied right away) | ⌘C / ⌘V (asks first if the text could run commands) |
| Scroll back | wheel or trackpad; the scrollbar shows while scrolled back and can be dragged | |
| Password prompt | a lock icon in the corner while the terminal doesn't echo what you type | |
| Paste file paths | drop files on a terminal (shell-escaped) | |
| Restart the shell (kills what runs in it) | | ⌘⇧P → Restart Shell |
| Font size (per terminal) | | ⌘+ / ⌘- / ⌘0 |
| Clear screen and scrollback | | ⌘K |
| Find in terminal (screen and scrollback) | | ⌘F, then ⌘G / ⇧⌘G or Enter / ⇧Enter, Esc to close; ⌘E finds the selection |
| Find in page (browser) | | ⌘F, same keys as find in terminal |
| Browser back / forward / reload | arrows and ↻ next to the address bar | ⌘[ / ⌘], ⌘R |
| Web inspector / JS console | | ⌥⌘I / ⌥⌘C |
| Settings | app menu › Settings… | ⌘, |
| Reload the config file | | ⌘⇧, |
| Change any of these keys | | `"shortcuts"` in the [config file](#shortcuts-and-commands) |

## Performance

`scripts/perf.py run` builds a release binary and measures it:

- binary size
- headless VT parse throughput (`abromux bench-vt`)
- headless kitty graphics frame cost: parse, snapshot, BGRA conversion (`abromux bench-kitty`)
- startup time
- idle memory and CPU
- memory per terminal
- in-app throughput
- frame draw time and late frames during in-app throughput
- kitty graphics in-app: 1920×1200 frames per second through shared memory (as terminal-browser sends them) and as base64 through the pty, frame draw p99 while streaming, and the footprint they add
- UI actions, each until the frame showing its result is on screen: new tab, split and workspace (until the prompt), closing a tab, switching workspaces, focusing tabs and panes, keystroke echo, scrolling, and a browser pane until its page has loaded. Each one also records that frame's draw time, which is abromux's own part. The terminal timings use a bare zsh so your shell config doesn't count, and one more times a new tab with your own login shell.

Each run is recorded with its commit, branch and machine, then compared with the last 10 runs on the same machine. A metric is flagged only when it's worse by more than both its own threshold and 3× the baseline's spread. Run it after each change; leave the Mac alone for the ~1.5 minutes it takes.

```bash
scripts/perf.py run                       # full suite (~1.5 min, opens the app)
scripts/perf.py run --headless            # no window: size + VT parse (~10 s)
scripts/perf.py run --label "try X"       # note stored with the run
scripts/perf.py history                   # recent runs with trend sparklines
scripts/perf.py history vt_color_mbps     # one metric over time
scripts/perf.py compare <id>              # re-compare a recorded run
scripts/perf.py report                    # HTML report with a chart per metric (opens it)
scripts/perf.py report --md -o perf.md    # the same as Markdown, e.g. for a PR
scripts/perf.py install-hook [--block]    # headless suite before every git push
```

History lives in `<git common dir>/abromux-perf/history.jsonl`, outside the tree. All worktrees share it and commits never touch it; `$ABROMUX_PERF_HISTORY` overrides the location. [`perf-vs-cmux.md`](perf-vs-cmux.md) compares abromux with cmux, measured with `scripts/bench.sh`.

## Layout

- `src/ghostty/`: FFI bindings and a safe wrapper (`Terminal`, `Frame` snapshots, key encoding, selection)
- `src/terminal.rs`: PTY plumbing plus a custom GPUI `Element` that paints the grid
- `src/browser.rs`: the webview pane; `src/webauthn.{rs,js,m}`: the passkey bridge
- `src/workspace.rs`: the workspace, split-tree and surface model
- `src/main.rs`: the app shell (pane tab bars, splits, actions); `src/sidebar.rs`: the sidebar's rows, groups, menus and drag to reorder
- `src/bench.rs`: `abromux bench-vt` and `abromux bench-kitty`, the headless parse benchmarks used by `scripts/perf.py`
- `src/server.rs`: the control socket's method handlers; `src/cli.rs`: the `abromux` commands; `src/ipc.rs`: the shared wire format

## Known POC limits

- Native webviews always draw above GPUI content. Hidden tabs and workspaces are hidden explicitly, but popovers can't overlap a visible browser.
- While a webview has keyboard focus, it gets the keystrokes, so ⌘-shortcuts may not reach the app.
