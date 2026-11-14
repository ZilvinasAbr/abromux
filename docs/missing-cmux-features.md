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
- [x] **Flash the focused panel**: ⌘⇧H, plus `trigger-flash` in the CLI.
- [x] **Pane zoom**: ⌘⇧↩ makes the focused pane fill the workspace, and again restores the split. *(also `zoom-pane [on|off|toggle]`, which cmux doesn't have)*
- [x] **Jump to tab N**: ⌃1–8, and ⌃9 for the last tab.
- [x] **Tab cycling with ⌃Tab / ⌃⇧Tab.**
- [x] **Renaming tabs**: `rename-tab`, overriding the shell's title. *(⌘R, double-click a tab, the palette, or `rename-tab`; saved with the session)*
- [x] **Dragging tabs**: reorder them within a pane, move them to another pane, or drop them on an edge to make a new split. The CLI equivalents are `move-surface`, `reorder-surface`, `split-off` and `drag-surface-to-split`. *(also drop a tab on a sidebar workspace to move it there)*
- [x] **Moving tabs to a new workspace** (`move-tab-to-new-workspace`, `break-pane`). *(also `detach-tab`, and Move Tab to New Workspace in the palette)*
- [x] **Joining or swapping panes** (`join-pane`, `swap-pane`), and switching to the last pane (`last-pane`). *(`swap-pane` and `last-pane` are also in the palette)*
- [x] **Focused-pane border**: an active-pane border color and a configurable divider color. *(`panes.focusBorderColor` and `panes.dividerColor` in the config file)*
- [x] **Keeping the workspace open when its last tab closes** (a setting). *(`app.keepWorkspaceOpenWhenClosingLastSurface`, as in cmux only for ⌘W: a fresh terminal takes the tab's place)*
- [ ] **Canvas layout**: a freeform canvas workspace with pane gaps and snapping. This is beta in cmux.
- [x] **Markdown viewer as a surface**: live reload (`cmux markdown`). *(`abromux markdown <file>`, `open <file>.md` or ⌘-click a `.md` path: a browser tab locked to local files, rendered GFM with relative links and images, updated in place as the file changes; no syntax highlighting in code blocks)*

## 3. Workspaces and sidebar

- [x] **Session restore**: save on quit and restore on relaunch. That covers windows, workspaces, the split layout, working directories, scrollback (best effort) and browser URLs with their history. It also covers reopening the previous session (⌘⇧O, `restore-session`). *(⌘⇧O reopens the launch session's workspaces that aren't open; `restore-session --from stable|nightly|<file>` and `--export`)*
- [x] **Workspace descriptions**: edit with ⌥⌘E and show them in the sidebar. *(also `new-workspace --description`, `workspace-action set-description`)*
- [x] **Workspace colors**: per-workspace color, indicator style and selection color. *(partial: `leftRail` and `solidFill` styles only; `workspace-action set-color`, the right-click menu)*
- [x] **Reordering workspaces**: drag in the sidebar, plus `reorder-workspace(s)` in the CLI. *(also `workspace-action move-up/move-down/move-top`)*
- [x] **Workspace groups**: grouping by cwd and choosing where new workspaces are placed. *(partial: groups you make, or Group Workspaces by Folder, with `workspaceGroups.newWorkspacePlacement`; no `byCwd` icons or header context-menu entries from the config; `abromux group …` instead of cmux's group flags beyond `new-workspace --group`)*
- [x] **More sidebar metadata**:
  - [x] the latest notification's text
  - [x] status pills set by scripts (`set-status`, `clear-status`, `list-status`) *(partial: `--icon` only knows abromux's bundled icons; not saved with the session)*
  - [x] a progress bar (`set-progress`, `clear-progress`)
  - [x] a per-workspace log (`log`, `list-log`, `clear-log`) *(partial: the sidebar shows only the newest entry)*
- [x] **Sidebar settings**: font size, hiding details, wrapping titles, branch layout, last path segment only, tint and opacity. *(cmux's `sidebar`, `sidebarAppearance` and `workspaceColors` keys; not `matchTerminalBackground`, light/dark tints, badge or spinner positions, compact status)*
- [x] **New workspaces inherit the cwd** (a setting). *(`app.workspaceInheritWorkingDirectory`, and `app.newWorkspacePlacement`)*
- [x] **Auto-naming workspaces** from what the agent is doing. *(`automation.workspaceAutoNaming`, off by default: Claude Code and Codex name it from their prompts and keep it current; ⇧⌘I workspaces on a made-up name always; not tabs)*
- [ ] **Custom sidebars**: user-written, SwiftUI-style sidebar files. Beta in cmux.

## 4. Notifications and attention

- [x] **OSC 9 / 99 / 777 desktop notifications** from terminal programs.
- [x] **Unread ring on the pane** and lit-up tabs.
- [x] **Notifications panel**: ⌘I lists pending notifications.
- [x] **Reorder on notification**: a workspace with a new notification moves to the top. *(`app.reorderOnNotification`, on by default as in cmux; within its group for a grouped workspace; `"agentActivity"` behaves as `"notifications"`)*
- [x] **Jump to the latest unread**: ⌘⇧U. Also ⌥⌘U to toggle unread, and ⌃⌘U to mark one as oldest and move to the next.
- [x] **Native macOS notifications**: banners, dock badge, menu-bar item, sounds, and suppression while the app is focused. *(partial: banners only from AbroMux.app; the menu-bar item (`notifications.showInMenuBar`) has the unread list and actions, not cmux's search or task manager)*
- [x] **CLI**: `notify`, `list-notifications`, `dismiss-notification`, `mark-notification-read`, `open-notification`, `jump-to-unread`, `clear-notifications`.
- [x] **Notification hooks**: run a command when a notification fires. *(cmux's `notifications.hooks`: JSON in and out, with `record`, `markUnread`, `desktop` and `sound` effects; only the global config's hooks, not a project's)*

## 5. Browser

- [x] **Find in page**: ⌘F in browser panes, with the same bar and keys as find in terminal. cmux does it with injected JavaScript that wraps matches in `<mark>`; so does abromux. See [browser-find.md](browser-find.md). *(partial: the bar is a row under the address bar, not floating over the page)*
- [x] **Browser shortcuts**: ⌘[ / ⌘] for back and forward, ⌘R to reload, ⌥⌘I for devtools, ⌥⌘C for the JS console.
- [x] **Passkeys, WebAuthn and security keys**: a JS bridge to AuthenticationServices, as in cmux. *(partial: needs Apple's web-browser entitlement in a provisioning profile, see "Passkeys" in guide.md; no conditional/autofill UI)*
- [x] **Shortcuts while a webview has focus**: tested with real key events, WebKit kept every ⌘ key once the page had focus. A key monitor now offers ⌘/⌃ keys to gpui first while a page has focus; ones the app handles (⌘T, ⌘W, ⌘=, ⌘⇧P…) stop there, the rest (⌘C, ⌘A, a site's own shortcuts) still reach the page.
- [x] **Popovers over webviews**: native webviews draw on top of everything, so UI can't overlap a visible browser. *(a page under the palette, ⌘I, the update popover, sidebar menus, address-bar suggestions, the downloads list or a tab drag is swapped for a bitmap of itself while it's open)*
- [x] **Zoom**: ⌘= / ⌘- / ⌘0 in browser panes, in Safari's steps (50%–300%), saved with the session. *(also `browser zoom in|out|reset|<factor>|<n>%`)*
- [x] **Downloads**: ask where to save, list downloads, wait for one to finish. *(partial: saved to ~/Downloads, `Content-Disposition: attachment` too; a toolbar button with progress and the session's list; `browser downloads` and `wait-download [--path]`; no asking where to save)*
- [ ] **Reload a page whose web process was killed**: macOS ends background WebContent processes under memory pressure, which leaves the tab blank. Handle `webViewWebContentProcessDidTerminate` and reload the page when its tab is shown again (right away if it's visible).
- [x] **Link-hover preview** of the URL. *(at the page's bottom-left corner, as in Safari, iframes included)*
- [x] **Browser automation in the CLI**: *(partial: events are synthesized with JavaScript, so they aren't trusted input)*
  - [x] `snapshot`: an accessibility or DOM snapshot with element refs (`--interactive`, `--compact`, `--selector`)
  - [x] `click`, `dblclick`, `hover`, `focus`, `check`, `uncheck`, `scroll-into-view`
  - [x] `type`, `fill`, `press`, `keydown`, `keyup`, `select`, `scroll`
  - [x] `wait`: for a selector, text, URL, load state or JS condition
  - [x] `get`: url, title, text, html, value, attr, count, box or styles; and `is`: visible, enabled or checked
  - [x] `find`: by role, text, label, placeholder, alt, title or testid
  - [x] `screenshot` *(partial: the page must be on screen)*
  - [x] `console`, `errors` and network activity *(network: fetch/XHR with status, other loads from Resource Timing)*
  - [x] `cookies`, `storage`, `state save/load`, `frame` *(frame: cross-origin iframes too; state load sets localStorage only for the page's origin)*
  - [ ] `addinitscript`, `dialog`, `highlight`
  - [x] `tab`: new, list, switch or close *(the browser tabs of the browser's pane)*
  - [ ] `devtools toggle`, `focus-mode`, `design-mode`

## 6. CLI and socket API gaps

What's missing beyond the browser commands above:

- [ ] **UUID handles** (`--id-format uuids|both`). abromux has only short refs and indexes.
- [ ] **Socket auth**: control modes, socket password, and only allowing processes started inside the app.
- [ ] **`capabilities`, `events`**: an event stream with cursors, filters and reconnect.
- [x] **`wait-for`**: named signals between scripts. *(signal files in `$TMPDIR`, as in cmux)*
- [x] **`read-selection`**, and `read-screen --selection`. *(also in browser tabs)*
- [x] **Starting a workspace from a JSON layout**: `new-workspace --layout <json>` and `--description`. *(`--layout` from JSON or `@file`, and `--description`)*
- [x] **`workspace-action`, `tab-action`**: one-shot actions with title, color, description and URL. *(partial: no pinned tabs, so no `pin`/`unpin` in either)*
- [ ] **`top` / `memory`**: CPU, memory and process trees per workspace or surface.
- [ ] **`surface-health`, `refresh-surfaces`, `debug-terminals`.**
- [ ] **tmux-style buffers and hooks**: `set-buffer`, `list-buffers`, `paste-buffer`, `set-hook`, `display-message`, `find-window`.
- [x] **Shell completion scripts** for the CLI. *(`completion zsh|bash|fish`: commands and subcommands, generated from the help text)*
- [x] **`guide` / `--skill`**: an agent-facing usage guide. *(as agent skills: `abromux skills install`, `abromux skills show`)*

## 7. Configuration

- [x] **A `cmux.json`-style config file** with a JSON schema, `config doctor`/`check`/`validate`, and `reload-config` (⌘⇧,), which reloads in place with no restart. *(partial: `~/.config/abromux/config.json` with `config check`, `config path` and `reload-config`, plus a project file (`.abromux/config.json`, or cmux's `.cmux/cmux.json` / `cmux.json`) for commands; no JSON schema)*
- [x] **Settings window**: ⌘,. *(partial: pages for general, sidebar, workspace colors, panes, shortcuts (with a recorder) and commands, writing `config.json`; chords, `"when"` contexts, palette colors and commands are edited in the file)*
- [x] **Customizable keyboard shortcuts**, including two-step chords and `when` conditions. *(`"shortcuts"` in config.json, listed by `abromux config shortcuts`; `when` takes a key context)*
- [x] **Custom commands and actions**: project-specific commands and workspace layouts that you launch from the palette or the plus-button menu. Also packs and setting presets. *(partial: `"commands"` run text in a terminal, open a URL or build a workspace layout in cmux's format, from the palette or their own shortcut, globally or per project with a trust prompt; no packs or presets. Tab-bar buttons are descoped, below)*
- [x] **Command palette**: ⌘⇧P for commands, ⌘P to switch workspaces. *(partial: the switcher lists workspaces, not surfaces; no custom commands or rename/description modes)*
- [ ] **App preferences**: light/dark/system appearance, accent color, app icon, window-title template, menu-bar-only mode, minimal mode. *(partial: light/dark/system appearance, `app.appearance`; not the rest)*
- [ ] **Themes**: `themes list/set/clear`.

## 8. Remote, persistence and process lifetime

- [ ] **SSH workspaces**: `ssh`, `mosh`, with agent forwarding and identity options.
- [ ] **Remote tmux**: `ssh-tmux`, `mosh-tmux`, and mirroring remote tmux sessions as native panes.
- [x] **Local tmux/zellij ownership**: `local-tmux`, `local-zellij`, so shells survive an app quit, crash or update. *(partial: with abromux's own holder, the default `"persistence": "abromux"`; not tmux or zellij)*
- [ ] **SSH session management**: `ssh-session-list`, `ssh-session-attach`, `ssh-session-cleanup`, and a remote daemon.

## 9. Coding-agent integrations

- [x] **Agent hooks**: `hooks setup` for Claude Code, Codex, OpenCode, Gemini, Kiro, Amp, Cursor, Pi and others. They feed notifications, status and session IDs. *(`hooks install` for Claude Code, Codex, Gemini CLI, Droid, Cursor, OpenCode and Pi; it feeds session IDs, the sidebar status and notifications. Kiro, Amp and Copilot are descoped, below)*
- [x] **Agent session resume**: `restore` and `fork` from checkpoints, plus `surface resume` commands. *(session restore resumes the sessions of every agent with hooks; `fork` and the CLI commands are descoped, below)*
- [ ] **Computer use**: `cmux-cua`, giving agents control of native apps.

## 10. App and platform

- [x] **App bundle and distribution**: a `.app` with an icon, DMG, Homebrew cask, auto-update (Sparkle) and a nightly channel. *(partial: `scripts/bundle-app.sh` builds the `.app`, a notarized DMG with `--dist`, and the Nightly app; updates work like cmux's (daily check, update pill, Install and Relaunch) without Sparkle; no cask)*
- [x] **CLI installed on PATH**: abromux's CLI lives inside the binary. *(partial: the README's `ln -sf` puts it on PATH; nothing does it for you)*
- [x] **Menu bar menus**: File, Edit, View, Window and Help. *(partial: abromux has these plus Notifications, built from its existing actions; no History or "Open in…" items)*
- [ ] **Sleepy Mode**: an idle screen. Cosmetic.

## 11. Descoped until asked

Left out on purpose (2026-11-01). Don't pick these up for parity work until the user asks for them again.

- **Hooks for Kiro, Amp and Copilot.** *(from 9)*
- **Multiple windows**: ⌘⇧N, and the window commands in the CLI: `list-windows`, `current-window`, `new-window`, `focus-window`, `close-window`, `rename-window`, `move-workspace-to-window`, `next-window` / `previous-window` / `last-window`. abromux has a single window. *(from 6 and 10)*
- **Right sidebar**: toggle it (⌥⌘B), focus it (⌘⇧E), and its modes: file explorer, find in directory (⌘⇧F, a ripgrep-backed project search), sessions, feed, and the dock of custom terminal controls defined in `.cmux/dock.json` or `~/.config/cmux/dock.json`. *(from 1 and 3)*
- **Built-in viewers**: a diff viewer for unstaged, staged, branch or last-turn changes (`cmux diff`), and a file editor with syntax highlighting. The Markdown viewer stays in scope. *(from 2)*
- **Agents** *(from 9)*:
  - Feed: an approval inbox for agent permission prompts and turn-complete events.
  - Forking agent sessions, and the `surface resume` commands in the CLI.
  - Agent hibernation: put idle agents to sleep under memory pressure and wake them later.
  - Multi-agent teams as panes: `claude-teams`, `codex-teams`, and oh-my-opencode (`omo`/`omx`/`omc`). Subagents become native splits.
  - Agent-to-agent messages.
  - Vault: search agent sessions, checkpoint them and fork them.
  - A todo list per workspace: `todo add/list/check/...`.
- **Sidebar metadata**: linked PR status and number, and listening ports. *(from 3)*
- **Importing from other browsers**: cookies, history and sessions from Chrome, Firefox, Arc and others. *(from 5)*
- **Browser profiles and search engines**: separate cookie jars (list, add, rename, clear, delete), and search-engine settings with search suggestions (dropped 2026-11-07). *(from 5)*
- **Discarding hidden webviews**: cmux unloads a browser tab hidden for 5 minutes and reloads it when shown. abromux keeps them alive, so returning to a tab is instant and keeps its state; the reload above covers pages macOS kills itself (dropped 2026-11-07). *(from 5)*
- **Tab-bar buttons**: cmux's `surfaceTabBarButtons`, commands as buttons in the pane tab bar (dropped 2026-11-03; abromux's tab bar has no buttons). *(from 7)*

## 12. cmux Cloud and paid extras (probably out of scope)

- [ ] Cloud VMs and workspaces (⌘Y, ⌘⇧Y, `cloud …`), remotes, AI accounts, CodeRouter.
- [ ] An iOS companion app that syncs terminals, browser tunnelling and artifact folder access.
- [ ] Notifications relayed from other Macs or cloud machines.
- [ ] The "Chief" home agent, agent chat GUI, voice mode, `sudo run` with Touch ID, and `feedback`.

---

## Suggested order

If the goal is daily-driver parity, a reasonable sequence is:

Done so far: a Markdown viewer tab; notifications (hooks, a menu-bar item, `dismiss-notification` / `mark-notification-read`, ⌥⌘U and ⌃⌘U); small CLI gaps (`read-selection`, `tab-action`, `wait-for`, shell completions, `browser zoom`); browser zoom; moving tabs between panes and workspaces from the CLI (`move-surface`, `split-off`, `join-pane`, `break-pane`), `zoom-pane`, `trigger-flash` and keeping a workspace open on ⌘W; workspace descriptions, colors, groups, drag to reorder, sidebar settings, auto-naming and reopening the previous session; customizable shortcuts and custom palette commands, including workspace layouts and project commands; copy-on-select, a password indicator, a scrollbar and scroll speed, `last-pane` / `swap-pane`; a config file with reload and a Settings window; renaming tabs, focused-pane border, safe paste, respawning a pane, dropping files on a terminal; command palette; Ghostty config, font size and clear scrollback; IME and mouse reporting; session restore; notifications; find, directional focus and resize; browser automation; agent status, notifications and resume for Claude Code, Codex, Gemini CLI, Droid, Cursor, OpenCode and Pi; sidebar status, progress and log.

Next:

1. **Browser**: reloading a page whose web process was killed, and the rest of automation (`addinitscript`, `dialog`, `highlight`).
2. **A JSON schema for the config file**, for editor completion and a stricter `config check`.
3. **SSH/tmux**, as the workspace model matures.
