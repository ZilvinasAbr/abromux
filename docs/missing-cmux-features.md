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
- [x] **Find in terminal**: ⌘F, find next/previous (⌘G / ⌥⌘G), use selection for find (⌘E), hide the find bar. *(browser pages have their own, see Find in page)*
- [ ] **Find in directory**: ⌘⇧F, a ripgrep-backed project search.
- [x] **Scrollbar**, plus a scroll-speed setting. *(Ghostty's `scrollbar` and `mouse-scroll-multiplier`; an overlay bar that shows while scrolled back)*
- [x] **Copy options**: copy-on-select, a copy confirmation, reflowing hard wraps on copy. *(partial: `copy-on-select = clipboard` from the Ghostty config; no copy confirmation or reflowing)*
- [x] **Safe paste**: confirm before pasting multi-line or otherwise unsafe text. *(Ghostty's rule and its `clipboard-paste-protection` / `clipboard-paste-bracketed-safe` settings: a newline only counts as unsafe when the program hasn't turned on bracketed paste)*
- [x] **Clickable links**: URLs and file paths, opening in a browser pane. *(partial: ⌘-click; a URL wrapped onto the next row isn't found, and `path:line` opens the file without going to the line)*
- [x] **Password input indicator**: shows when the terminal is in no-echo mode.
- [x] **Text-editing gestures**: ⌥←/→ word movement and ⌘←/→ line movement in the shell prompt. *(Ghostty's default macOS text keybinds, plus ⌘⌫)*
- [ ] **Max content width and alignment**, for wide displays.
- [x] **Terminal drag & drop**: drop files to paste their paths.
- [x] **Respawning a pane**: restart a dead shell in the same surface (`respawn-pane`). *(partial: always kills and starts the login shell again, like tmux's `respawn-pane -k`, with an optional `--command`; a shell that exits still closes its tab)*
- [ ] **Piping pane output**: tee terminal output to a command (`pipe-pane`).

## 2. Panes, tabs and layout

- [x] **Directional pane focus**: ⌥⌘ ← → ↑ ↓.
- [x] **Shortcut hints**: holding ⌘ shows each workspace's ⌘N, holding ⌃ shows the focused pane's ⌃N tab numbers.
- [x] **Keyboard pane resizing**: ⌃⇧ H J K L with a configurable step, plus `resize-pane` in the CLI. *(the step is `panes.resizeStep` in the config file)*
- [x] **Flash the focused panel**: ⌘⇧H, plus `trigger-flash` in the CLI. *(partial: no `trigger-flash`)*
- [x] **Pane zoom**: ⌘⇧↩ makes the focused pane fill the workspace, and again restores the split. *(partial: no `zoom-pane` in the CLI)*
- [x] **Jump to tab N**: ⌃1–8, and ⌃9 for the last tab.
- [x] **Tab cycling with ⌃Tab / ⌃⇧Tab.**
- [x] **Renaming tabs**: `rename-tab`, overriding the shell's title. *(double-click a tab, the palette, or `rename-tab`; saved with the session)*
- [ ] **Dragging tabs**: reorder them within a pane, move them to another pane, or drop them on an edge to make a new split. The CLI equivalents are `move-surface`, `reorder-surface`, `split-off` and `drag-surface-to-split`.
- [ ] **Moving tabs to a new workspace** (`move-tab-to-new-workspace`, `break-pane`).
- [x] **Joining or swapping panes** (`join-pane`, `swap-pane`), and switching to the last pane (`last-pane`). *(partial: `swap-pane` and `last-pane`, in the CLI and the palette; no `join-pane`)*
- [x] **Focused-pane border**: an active-pane border color and a configurable divider color. *(`panes.focusBorderColor` and `panes.dividerColor` in the config file)*
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
  - [x] the latest notification's text
  - [x] status pills set by scripts (`set-status`, `clear-status`, `list-status`) *(partial: `--icon` only knows abromux's bundled icons; not saved with the session)*
  - [x] a progress bar (`set-progress`, `clear-progress`)
  - [x] a per-workspace log (`log`, `list-log`, `clear-log`) *(partial: the sidebar shows only the newest entry)*
- [ ] **Sidebar settings**: font size, hiding details, wrapping titles, branch layout, last path segment only, tint and opacity.
- [ ] **New workspaces inherit the cwd** (a setting). *(partial: ⌘N already uses the focused terminal's cwd)*
- [ ] **Auto-naming workspaces** from what the agent is doing.
- [ ] **Right sidebar**: toggle it (⌥⌘B), focus it (⌘⇧E), and switch between modes such as file explorer, find, sessions, feed and dock.
- [ ] **Dock**: custom terminal controls in the right sidebar, defined in `.cmux/dock.json` or `~/.config/cmux/dock.json`.
- [ ] **Custom sidebars**: user-written, SwiftUI-style sidebar files. Beta in cmux.

## 4. Notifications and attention

- [x] **OSC 9 / 99 / 777 desktop notifications** from terminal programs.
- [x] **Unread ring on the pane** and lit-up tabs.
- [x] **Notifications panel**: ⌘I lists pending notifications.
- [x] **Jump to the latest unread**: ⌘⇧U. Also ⌥⌘U to toggle unread, and ⌃⌘U to mark one as oldest and move to the next. *(partial: no ⌥⌘U or ⌃⌘U)*
- [x] **Native macOS notifications**: banners, dock badge, menu-bar item, sounds, and suppression while the app is focused. *(partial: no menu-bar item; banners only from AbroMux.app)*
- [x] **CLI**: `notify`, `list-notifications`, `dismiss-notification`, `mark-notification-read`, `open-notification`, `jump-to-unread`, `clear-notifications`. *(partial: no `dismiss-notification` or `mark-notification-read`)*
- [ ] **Notification hooks**: run a command when a notification fires.

## 5. Browser

- [x] **Find in page**: ⌘F in browser panes, with the same bar and keys as find in terminal. cmux does it with injected JavaScript that wraps matches in `<mark>`; so does abromux. See [browser-find.md](browser-find.md). *(partial: the bar is a row under the address bar, not floating over the page)*
- [x] **Browser shortcuts**: ⌘[ / ⌘] for back and forward, ⌘R to reload, ⌥⌘I for devtools, ⌥⌘C for the JS console.
- [x] **Passkeys, WebAuthn and security keys**: a JS bridge to AuthenticationServices, as in cmux. *(partial: needs Apple's web-browser entitlement in a provisioning profile, see "Passkeys" in guide.md; no conditional/autofill UI)*
- [ ] **Shortcuts while a webview has focus**: gpui's view gets ⌘ keys first (`performKeyEquivalent:`) even when the WKWebView is first responder, and a click into a page moves gpui focus to its `Browser` context, so ⌘ shortcuts should reach the app. Not yet confirmed with real keystrokes; keys without ⌘ still go only to the page.
- [ ] **Popovers over webviews**: native webviews draw on top of everything, so UI can't overlap a visible browser.
- [ ] **Zoom**: in, out, reset, or a set factor.
- [ ] **Search-engine settings and search suggestions.**
- [ ] **Downloads**: ask where to save, list downloads, wait for one to finish.
- [ ] **Profiles**: separate cookie jars, with list, add, rename, clear and delete.
- [ ] **Importing from other browsers**: cookies, history and sessions from Chrome, Firefox, Arc and others.
- [ ] **Discarding hidden webviews**: memory budgeting for background tabs.
- [ ] **Link-hover preview** of the URL.
- [x] **Browser automation in the CLI**: *(partial: events are synthesized with JavaScript, so they aren't trusted input; no iframes)*
  - [x] `snapshot`: an accessibility or DOM snapshot with element refs (`--interactive`, `--compact`, `--selector`)
  - [x] `click`, `dblclick`, `hover`, `focus`, `check`, `uncheck`, `scroll-into-view`
  - [x] `type`, `fill`, `press`, `keydown`, `keyup`, `select`, `scroll` *(partial: no separate `keydown` / `keyup`)*
  - [x] `wait`: for a selector, text, URL, load state or JS condition
  - [x] `get`: url, title, text, html, value, attr, count, box or styles; and `is`: visible, enabled or checked
  - [x] `find`: by role, text, label, placeholder, alt, title or testid
  - [x] `screenshot` *(partial: the page must be on screen)*
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
- [x] **`guide` / `--skill`**: an agent-facing usage guide. *(as agent skills: `abromux skills install`, `abromux skills show`)*

## 7. Configuration

- [x] **A `cmux.json`-style config file** with a JSON schema, `config doctor`/`check`/`validate`, and `reload-config` (⌘⇧,), which reloads in place with no restart. *(partial: `~/.config/abromux/config.json` with `config check`, `config path` and `reload-config`; only pane settings so far, no JSON schema, no project-level file)*
- [ ] **Settings window**: ⌘,.
- [x] **Customizable keyboard shortcuts**, including two-step chords and `when` conditions. *(`"shortcuts"` in config.json, listed by `abromux config shortcuts`; `when` takes a key context)*
- [x] **Custom commands and actions**: project-specific commands and workspace layouts that you launch from the palette, tab-bar buttons or the plus-button menu. Also packs and setting presets. *(partial: `"commands"` in config.json run text in a terminal or open a URL, from the palette or their own shortcut; no layouts, buttons, project-level file, packs or presets)*
- [x] **Command palette**: ⌘⇧P for commands, ⌘P to switch workspaces. *(partial: the switcher lists workspaces, not surfaces; no custom commands or rename/description modes)*
- [ ] **App preferences**: light/dark/system appearance, accent color, app icon, window-title template, menu-bar-only mode, minimal mode.
- [ ] **Themes**: `themes list/set/clear`.

## 8. Remote, persistence and process lifetime

- [ ] **SSH workspaces**: `ssh`, `mosh`, with agent forwarding and identity options.
- [ ] **Remote tmux**: `ssh-tmux`, `mosh-tmux`, and mirroring remote tmux sessions as native panes.
- [ ] **Local tmux/zellij ownership**: `local-tmux`, `local-zellij`, so shells survive an app quit, crash or update.
- [ ] **SSH session management**: `ssh-session-list`, `ssh-session-attach`, `ssh-session-cleanup`, and a remote daemon.

## 9. Coding-agent integrations

- [x] **Agent hooks**: `hooks setup` for Claude Code, Codex, OpenCode, Gemini, Kiro, Amp, Cursor, Pi and others. They feed notifications, status and session IDs. *(partial: `hooks install` for Claude Code, Codex, Gemini CLI, Droid, Cursor, OpenCode and Pi; it feeds session IDs, the sidebar status and notifications. No Kiro, Amp or Copilot yet)*
- [x] **Agent session resume**: `restore` and `fork` from checkpoints, plus `surface resume` commands. *(partial: session restore resumes the sessions of every agent with hooks; no `fork` or CLI commands)*
- [ ] **Agent hibernation**: put idle agents to sleep under memory pressure and wake them later.
- [ ] **Feed**: an approval inbox for agent permission prompts and turn-complete events.
- [ ] **Multi-agent teams as panes**: `claude-teams`, `codex-teams`, and oh-my-opencode (`omo`/`omx`/`omc`). Subagents become native splits.
- [ ] **Agent-to-agent messages.**
- [ ] **Vault**: search agent sessions, checkpoint them and fork them.
- [ ] **Todo list per workspace**: `todo add/list/check/...`.
- [ ] **Computer use**: `cmux-cua`, giving agents control of native apps.

## 10. App and platform

- [ ] **Multiple windows**: ⌘⇧N.
- [ ] **App bundle and distribution**: a `.app` with an icon, DMG, Homebrew cask, auto-update (Sparkle) and a nightly channel. *(partial: `scripts/bundle-app.sh` builds the `.app`, a notarized DMG with `--dist`, and the Nightly app; no cask or updates)*
- [x] **CLI installed on PATH**: abromux's CLI lives inside the binary. *(partial: the README's `ln -sf` puts it on PATH; nothing does it for you)*
- [x] **Menu bar menus**: File, Edit, View, Window and Help. *(partial: abromux has these plus Notifications, built from its existing actions; no Settings, Check for Updates, History or "Open in…" items)*
- [ ] **Sleepy Mode**: an idle screen. Cosmetic.

## 11. cmux Cloud and paid extras (probably out of scope)

- [ ] Cloud VMs and workspaces (⌘Y, ⌘⇧Y, `cloud …`), remotes, AI accounts, CodeRouter.
- [ ] An iOS companion app that syncs terminals, browser tunnelling and artifact folder access.
- [ ] Notifications relayed from other Macs or cloud machines.
- [ ] The "Chief" home agent, agent chat GUI, voice mode, `sudo run` with Touch ID, and `feedback`.

---

## Suggested order

If the goal is daily-driver parity, a reasonable sequence is:

Done so far: customizable shortcuts and custom palette commands; copy-on-select, a password indicator, a scrollbar and scroll speed, `last-pane` / `swap-pane`; a config file with reload; renaming tabs, focused-pane border, safe paste, respawning a pane, dropping files on a terminal; command palette; Ghostty config, font size and clear scrollback; IME and mouse reporting; session restore; notifications; find, directional focus and resize; browser automation; agent status, notifications and resume for Claude Code, Codex, Gemini CLI, Droid, Cursor, OpenCode and Pi; sidebar status, progress and log.

Next:

1. **Custom commands that build layouts** (cmux's workspace commands), and a project-level config file.
2. **Hooks for the remaining agents** (Kiro, Amp, Copilot), once their hook formats settle.
3. **Dragging tabs, multiple windows, SSH/tmux**, as the workspace model matures.
