# cmux features abromux doesn't have yet

A gap list between abromux and [cmux](https://cmux.com). It was built on 2026-10-24 from cmux's README, keyboard-shortcut data, `cmux.json` settings schema, docs, and the help output of its bundled CLI, then checked against abromux at `6fcf3db`.

Each section is ordered roughly by how much it matters for everyday use. Items marked *(partial)* exist in abromux in a smaller form. cmux's paid and cloud-only extras are grouped at the end.

## What abromux already covers

So the list below reads in context:

- Workspaces in a vertical sidebar showing name, focused tab title, git branch and cwd. You can create, rename, close and select them (⌘N, ⌘⇧R, ⌘⇧W, ⌘1–9, ⌃⌘[ / ⌃⌘]).
- Split panes (right/down from the UI, all four directions from the CLI) with draggable dividers.
- Per-pane tabs of terminals and browsers (⌘T, ⌘⇧L, ⌘W, ⌘⇧[ / ⌘⇧]).
- libghostty-vt terminals with selection, copy and paste.
- WKWebView browser panes with an address bar, back, forward and reload.
- A blue unread dot when a terminal rings the bell (BEL) in a background workspace.
- The `abromux` CLI and socket: workspaces, panes, surfaces, splits, `send`, `send-key`, `read-screen`, `tree`, `identify`, `browser open/navigate/back/forward/reload/url/eval`, and `rpc`.

---

## 1. Terminal

- [x] **IME and dead-key composition**: CJK input and accented characters (e.g. ⌥e e).
- [x] **Mouse reporting to TUI apps**: clicks, drags and wheel events sent to vim, htop, lazygit and similar. Today the wheel only scrolls scrollback.
- [x] **Reading the Ghostty config** (`~/.config/ghostty/config`): theme, font family and size, colors, cursor style, background opacity and blur, keybinds, including leader sequences and key tables. *(partial: theme, font, colors, palette, cursor and `text:`/`csi:`/`esc:` keybinds; not opacity, blur, padding, leader sequences or other actions)*
- [x] **Font size controls**: ⌘+ / ⌘- / ⌘0.
- [x] **Clear scrollback**: ⌘K, plus `clear-history` in the CLI.
- [x] **Find in terminal**: ⌘F, find next/previous (⌘G / ⌥⌘G), use selection for find (⌘E), hide the find bar. *(terminal panes only, not browser pages)*
- [ ] **Find in directory**: ⌘⇧F, a ripgrep-backed project search.
- [ ] **Scrollbar**, plus a scroll-speed setting.
- [ ] **Copy options**: copy-on-select, a copy confirmation, reflowing hard wraps on copy.
- [ ] **Safe paste**: confirm before pasting multi-line or otherwise unsafe text.
- [ ] **Clickable links**: URLs and file paths, opening in a browser pane.
- [ ] **Password input indicator**: shows when the terminal is in no-echo mode.
- [ ] **Text-editing gestures**: ⌥←/→ word movement and ⌘←/→ line movement in the shell prompt.
- [ ] **Max content width and alignment**, for wide displays.
- [ ] **Terminal drag & drop**: drop files to paste their paths.
- [ ] **Respawning a pane**: restart a dead shell in the same surface (`respawn-pane`).
- [ ] **Piping pane output**: tee terminal output to a command (`pipe-pane`).

## 2. Panes, tabs and layout

- [x] **Directional pane focus**: ⌥⌘ ← → ↑ ↓.
- [x] **Shortcut hints**: holding ⌘ shows each workspace's ⌘N, holding ⌃ shows the focused pane's ⌃N tab numbers.
- [ ] **Keyboard pane resizing**: ⌃⇧ H J K L with a configurable step, plus `resize-pane` in the CLI.
- [x] **Flash the focused panel**: ⌘⇧H, plus `trigger-flash` in the CLI. *(partial: no `trigger-flash`)*
- [x] **Jump to tab N**: ⌃1–8, and ⌃9 for the last tab.
- [x] **Tab cycling with ⌃Tab / ⌃⇧Tab.**
- [ ] **Renaming tabs**: `rename-tab`, overriding the shell's title.
- [ ] **Dragging tabs**: reorder them within a pane, move them to another pane, or drop them on an edge to make a new split. The CLI equivalents are `move-surface`, `reorder-surface`, `split-off` and `drag-surface-to-split`.
- [ ] **Moving tabs to a new workspace** (`move-tab-to-new-workspace`, `break-pane`).
- [ ] **Joining or swapping panes** (`join-pane`, `swap-pane`), and switching to the last pane (`last-pane`).
- [ ] **Focused-pane border**: an active-pane border color and a configurable divider color.
- [ ] **Keeping the workspace open when its last tab closes** (a setting).
- [ ] **Canvas layout**: a freeform canvas workspace with pane gaps and snapping. This is beta in cmux.
- [ ] **Built-in viewers as surfaces**: a Markdown viewer with live reload (`cmux markdown`), a diff viewer for unstaged, staged, branch or last-turn changes (`cmux diff`), and a file editor with syntax highlighting.

## 3. Workspaces and sidebar

- [x] **Session restore**: save on quit and restore on relaunch. That covers windows, workspaces, the split layout, working directories, scrollback (best effort) and browser URLs with their history. It also covers reopening the previous session (⌘⇧O, `restore-session`). *(partial: no browser back/forward history, and no ⌘⇧O / `restore-session`)*
- [ ] **Workspace descriptions**: edit with ⌥⌘E and show them in the sidebar.
- [ ] **Workspace colors**: per-workspace color, indicator style and selection color.
- [ ] **Reordering workspaces**: drag in the sidebar, plus `reorder-workspace(s)` in the CLI.
- [ ] **Workspace groups**: grouping by cwd and choosing where new workspaces are placed.
- [ ] **More sidebar metadata**:
  - [ ] linked PR status and number
  - [ ] listening ports
  - [ ] the latest notification's text
  - [ ] status pills set by scripts (`set-status`, `clear-status`, `list-status`)
  - [ ] a progress bar (`set-progress`, `clear-progress`)
  - [ ] a per-workspace log (`log`, `list-log`, `clear-log`)
- [ ] **Sidebar settings**: font size, hiding details, wrapping titles, branch layout, last path segment only, tint and opacity.
- [ ] **New workspaces inherit the cwd** (a setting). *(partial: ⌘N already uses the focused terminal's cwd)*
- [ ] **Auto-naming workspaces** from what the agent is doing.
- [ ] **Right sidebar**: toggle it (⌥⌘B), focus it (⌘⇧E), and switch between modes such as file explorer, find, sessions, feed and dock.
- [ ] **Dock**: custom terminal controls in the right sidebar, defined in `.cmux/dock.json` or `~/.config/cmux/dock.json`.
- [ ] **Custom sidebars**: user-written, SwiftUI-style sidebar files. Beta in cmux.

## 4. Notifications and attention

- [ ] **OSC 9 / 99 / 777 desktop notifications** from terminal programs. abromux only handles BEL.
- [ ] **Unread ring on the pane** and lit-up tabs. abromux only shows a sidebar dot.
- [ ] **Notifications panel**: ⌘I lists pending notifications.
- [ ] **Jump to the latest unread**: ⌘⇧U. Also ⌥⌘U to toggle unread, and ⌃⌘U to mark one as oldest and move to the next.
- [ ] **Native macOS notifications**: banners, dock badge, menu-bar item, sounds, and suppression while the app is focused.
- [ ] **CLI**: `notify`, `list-notifications`, `dismiss-notification`, `mark-notification-read`, `open-notification`, `jump-to-unread`, `clear-notifications`.
- [ ] **Notification hooks**: run a command when a notification fires.

## 5. Browser

- [ ] **Find in page**: ⌘F in browser panes, with the same bar and keys as find in terminal. cmux does it with injected JavaScript that wraps matches in `<mark>`. See [browser-find.md](browser-find.md) for how cmux does it and a plan to copy it.
- [ ] **Browser shortcuts**: ⌘[ / ⌘] for back and forward, ⌘R to reload, ⌥⌘I for devtools, ⌥⌘C for the JS console.
- [ ] **Shortcuts while a webview has focus**: keystrokes currently go to the webview, so ⌘ shortcuts can miss the app. This is a known POC limit.
- [ ] **Popovers over webviews**: native webviews draw on top of everything, so UI can't overlap a visible browser.
- [ ] **Zoom**: in, out, reset, or a set factor.
- [ ] **Search-engine settings and search suggestions.**
- [ ] **Downloads**: ask where to save, list downloads, wait for one to finish.
- [ ] **Profiles**: separate cookie jars, with list, add, rename, clear and delete.
- [ ] **Importing from other browsers**: cookies, history and sessions from Chrome, Firefox, Arc and others.
- [ ] **Discarding hidden webviews**: memory budgeting for background tabs.
- [ ] **Link-hover preview** of the URL.
- [ ] **Browser automation in the CLI** (abromux only has `open`, `navigate`, `back`, `forward`, `reload`, `url` and `eval`):
  - [ ] `snapshot`: an accessibility or DOM snapshot with element refs (`--interactive`, `--compact`, `--selector`)
  - [ ] `click`, `dblclick`, `hover`, `focus`, `check`, `uncheck`, `scroll-into-view`
  - [ ] `type`, `fill`, `press`, `keydown`, `keyup`, `select`, `scroll`
  - [ ] `wait`: for a selector, text, URL, load state or JS condition
  - [ ] `get`: url, title, text, html, value, attr, count, box or styles; and `is`: visible, enabled or checked
  - [ ] `find`: by role, text, label, placeholder, alt, title or testid
  - [ ] `screenshot`
  - [ ] `console`, `errors` and network activity
  - [ ] `cookies`, `storage`, `state save/load`, `addinitscript`, `dialog`, `frame`, `highlight`
  - [ ] `tab`: new, list, switch or close
  - [ ] `devtools toggle`, `focus-mode`, `design-mode`

## 6. CLI and socket API gaps

What's missing beyond the browser commands above:

- [ ] **Windows**: `list-windows`, `current-window`, `new-window`, `focus-window`, `close-window`, `rename-window`, `move-workspace-to-window`, `next-window` / `previous-window` / `last-window`. abromux has a single window.
- [ ] **UUID handles** (`--id-format uuids|both`). abromux has only short refs and indexes.
- [ ] **Socket auth**: control modes, socket password, and only allowing processes started inside the app.
- [ ] **`capabilities`, `events`**: an event stream with cursors, filters and reconnect.
- [ ] **`wait-for`**: named signals between scripts.
- [ ] **`read-selection`**, and `read-screen --selection`.
- [ ] **Starting a workspace from a JSON layout**: `new-workspace --layout <json>` and `--description`.
- [ ] **`workspace-action`, `tab-action`**: one-shot actions with title, color, description and URL.
- [ ] **`top` / `memory`**: CPU, memory and process trees per workspace or surface.
- [ ] **`surface-health`, `refresh-surfaces`, `debug-terminals`.**
- [ ] **tmux-style buffers and hooks**: `set-buffer`, `list-buffers`, `paste-buffer`, `set-hook`, `display-message`, `find-window`.
- [ ] **Shell completion scripts** for the CLI.
- [ ] **`guide` / `--skill`**: an agent-facing usage guide.

## 7. Configuration

- [ ] **A `cmux.json`-style config file** with a JSON schema, `config doctor`/`check`/`validate`, and `reload-config` (⌘⇧,), which reloads in place with no restart.
- [ ] **Settings window**: ⌘,.
- [ ] **Customizable keyboard shortcuts**, including two-step chords and `when` conditions.
- [ ] **Custom commands and actions**: project-specific commands and workspace layouts that you launch from the palette, tab-bar buttons or the plus-button menu. Also packs and setting presets.
- [ ] **Command palette.**
- [ ] **App preferences**: light/dark/system appearance, accent color, app icon, window-title template, menu-bar-only mode, minimal mode.
- [ ] **Themes**: `themes list/set/clear`.

## 8. Remote, persistence and process lifetime

- [ ] **SSH workspaces**: `ssh`, `mosh`, with agent forwarding and identity options.
- [ ] **Remote tmux**: `ssh-tmux`, `mosh-tmux`, and mirroring remote tmux sessions as native panes.
- [ ] **Local tmux/zellij ownership**: `local-tmux`, `local-zellij`, so shells survive an app quit, crash or update.
- [ ] **SSH session management**: `ssh-session-list`, `ssh-session-attach`, `ssh-session-cleanup`, and a remote daemon.

## 9. Coding-agent integrations

- [x] **Agent hooks**: `hooks setup` for Claude Code, Codex, OpenCode, Gemini, Kiro, Amp, Cursor, Pi and others. They feed notifications, status and session IDs. *(partial: `hooks install` for Claude Code only, and it feeds session IDs only)*
- [x] **Agent session resume**: `restore` and `fork` from checkpoints, plus `surface resume` commands. *(partial: session restore resumes Claude Code sessions; no `fork` or CLI commands)*
- [ ] **Agent hibernation**: put idle agents to sleep under memory pressure and wake them later.
- [ ] **Feed**: an approval inbox for agent permission prompts and turn-complete events.
- [ ] **Multi-agent teams as panes**: `claude-teams`, `codex-teams`, and oh-my-opencode (`omo`/`omx`/`omc`). Subagents become native splits.
- [ ] **Agent-to-agent messages.**
- [ ] **Vault**: search agent sessions, checkpoint them and fork them.
- [ ] **Todo list per workspace**: `todo add/list/check/...`.
- [ ] **Computer use**: `cmux-cua`, giving agents control of native apps.

## 10. App and platform

- [ ] **Multiple windows**: ⌘⇧N.
- [ ] **App bundle and distribution**: a `.app` with an icon, DMG, Homebrew cask, auto-update (Sparkle) and a nightly channel. abromux currently runs from `cargo run`.
- [ ] **CLI installed on PATH**: abromux's CLI lives inside the binary but isn't symlinked anywhere.
- [ ] **Menu bar menus**: File, Edit, View, Window and Help.
- [ ] **Sleepy Mode**: an idle screen. Cosmetic.

## 11. cmux Cloud and paid extras (probably out of scope)

- [ ] Cloud VMs and workspaces (⌘Y, ⌘⇧Y, `cloud …`), remotes, AI accounts, CodeRouter.
- [ ] An iOS companion app that syncs terminals, browser tunnelling and artifact folder access.
- [ ] Notifications relayed from other Macs or cloud machines.
- [ ] The "Chief" home agent, agent chat GUI, voice mode, `sudo run` with Touch ID, and `feedback`.

---

## Suggested order

If the goal is daily-driver parity, a reasonable sequence is:

1. **Ghostty config, font size and clear scrollback.** These are cheap and visible straight away.
2. **IME and mouse reporting.** Without them, vim, htop and non-English input are broken.
3. **Session restore.**
4. **OSC notifications, the pane ring, the notifications panel and jump-to-unread.** This is the core of cmux's agent workflow.
5. **Find in terminal, and directional focus and resize.**
6. **Browser automation** (`snapshot`/`click`/`type`/`wait`/`screenshot`), which makes `abromux browser` useful to agents.
7. **Config file, customizable shortcuts, command palette.**
8. **Agent hooks and SSH/tmux**, as the workspace model matures.
