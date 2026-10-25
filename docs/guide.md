# AbroMux guide

Everything AbroMux does, in detail. For a quick start, see the [README](../README.md).

## Features

- **Terminal panes** use [libghostty-vt](https://github.com/ghostty-org/ghostty) for all terminal state: VT parsing, screen and scrollback, colors, selection and key encoding. It is linked statically through a bindgen FFI layer. A shell runs in a PTY (`portable-pty`) and GPUI draws each cell. Text goes through the macOS input system, so IMEs and dead keys compose as usual. Mouse clicks, drags and the wheel go to TUI apps that ask for them (vim, htop, lazygit); hold Shift to select or scroll locally. Full-screen apps without mouse support get arrow keys from the wheel, like Ghostty's alternate scroll.
- **Browser panes** are native WKWebViews (wry, via `gpui-component`). Each has an address bar with back, forward and reload.
- **Workspaces** sit in a left sidebar, cmux style. Each entry shows the name, the focused tab's title, the git branch and the cwd, plus whatever scripts report through the CLI: status pills (`set-status`), a progress bar (`set-progress`) and the newest `log` line.
- **Claude Code and Codex** show up in the sidebar once you've run `abromux hooks install`: a "Running" pill while the agent works and "Needs input" when it asks for permission. When it asks, or finishes a turn, its terminal gets a notification ("Codex: Asks to run …", "Claude Code: <the start of its reply>").
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
```

AbroMux Nightly runs next to AbroMux. It has a neon icon (`assets/app/abromux-nightly.svg`), its own name, the bundle id `dev.abromux.nightly` (so separate notification permissions and settings), and its own socket and session in `~/Library/Application Support/abromux-nightly/`. It's signed like the release but never notarized: `scripts/bundle-app.sh --nightly` is what `nightly.sh` calls, and it refuses `--dist`. The binary decides which one it is at runtime from the bundle it sits in (`src/channel.rs`); `cargo run` counts as the release. Install agent hooks from the release `abromux`, not `abromux-nightly`: hooks point at the binary you install them from, and do nothing once that app is gone.

## Session restore

Quitting saves the session to `~/Library/Application Support/abromux/session.json` (the layout is also saved every few seconds, so a crash loses little). The next launch restores the window frame, workspaces and their names, the split layout and ratios, tabs, each terminal's working directory and font size, browser URLs, and up to 2000 lines of each terminal's output (with colors). Shells start fresh: running programs aren't kept alive. Claude Code and Codex sessions are the exception. Run `abromux hooks install` once and abromux learns, through the agents' hooks, which session runs in which terminal (the same hooks feed the sidebar status and notifications; rerun `install` after updating abromux to pick up new hook events). On restore, it types `claude --resume <id>` (with the same permission mode) or `codex resume <id>` into that terminal's new shell; a Codex you had quit isn't resumed. The hooks go into `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`) and `~/.codex/hooks.json` (or `$CODEX_HOME`; skipped if Codex isn't set up), after any hooks already there, and do nothing outside abromux. Codex asks you once to review new hooks: trust them when it does. They point at the binary you ran `install` from, so run it from the installed app; `abromux hooks uninstall` removes them. `ABROMUX_NO_RESTORE=1` skips it; `ABROMUX_SESSION_PATH` uses another file. `ABROMUX_DEMO` and `ABROMUX_SCRIPT` runs don't touch the session unless `ABROMUX_SESSION_PATH` is set.

## Ghostty config

Terminals read your Ghostty config: `~/.config/ghostty/config` (or `$XDG_CONFIG_HOME`), then `~/Library/Application Support/com.mitchellh.ghostty/config`, plus their `config-file` includes. abromux uses `theme` (from `~/.config/ghostty/themes` or Ghostty.app), `font-family`, `font-size`, `foreground`, `background`, `palette`, `cursor-color`, `cursor-text`, `cursor-style`, `selection-background`/`-foreground`, and `keybind`s whose action is `text:`, `csi:` or `esc:`. Other keys are ignored for now. Changes apply to new terminals.

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
abromux browser open https://example.com
abromux browser eval 'document.title'
abromux notify --title 'Tests' --body 'all green'   # rings the pane it runs in
abromux list-notifications
abromux jump-to-unread
abromux set-status build compiling --color '#e0a84f'   # a pill under the workspace
abromux set-progress 0.4 --label 'Building'
abromux log --level error --source build 'Build failed'
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

Targets are refs (`workspace:2`, `pane:5`, `surface:3`) or 0-based indexes. Each shell gets `ABROMUX_SOCKET_PATH`, `ABROMUX_WORKSPACE_ID` and `ABROMUX_SURFACE_ID`, so `abromux` run inside a pane acts on that pane by default (otherwise on what's focused). The wire format is one JSON object per line (`{"id":1,"method":"workspace.list","params":{}}`); `abromux rpc <method> [json]` sends one directly.

## Keyboard and mouse

| | Mouse | Keys |
|---|---|---|
| New workspace | `+` in sidebar header / footer | ⌘N |
| Rename workspace | double-click its name (Enter saves, Esc cancels) | ⌘⇧R |
| Delete workspace | hover → `×` | ⌘⇧W |
| Select workspace | click | ⌘1…⌘8, ⌘9 for the last, ⌃⌘[ / ⌃⌘] (hold ⌘ to see the numbers) |
| New terminal tab | terminal icon in pane tab bar | ⌘T |
| New browser tab | globe icon | ⌘⇧L (⌘L focuses address bar) |
| Split right / down | split icons | ⌘D / ⌘⇧D |
| Move tab / split by dragging | drag a tab onto a tab bar, a pane's edge or the window's edge | |
| Close tab | tab `×` / middle-click | ⌘W (last tab closes the pane; last pane closes the workspace) |
| Cycle tabs | | ⌘⇧[ / ⌘⇧], ⌃Tab / ⌃⇧Tab |
| Select tab in focused pane | click | ⌃1…⌃8, ⌃9 for the last (hold ⌃ to see the numbers) |
| Focus pane in a direction | click | ⌥⌘← → ↑ ↓ |
| Resize focused pane | drag a divider | ⌃⇧H / J / K / L (20 px a press) |
| Flash focused pane | | ⌘⇧H |
| Command palette: every command that applies to the focused terminal or browser, with its shortcut | | ⌘⇧P, type to filter, ↑/↓ or ⌃N/⌃P, Enter runs, Esc closes |
| Switch workspace by name | | ⌘P |
| Notifications list | bell icon in the titlebar | ⌘I (Esc closes) |
| Jump to newest unread notification | click it in the list | ⌘⇧U |
| Open a link in a terminal | ⌘-click | |
| Toggle sidebar | sidebar icon | ⌘B |
| Copy / paste in terminal | drag to select, double-click selects a word | ⌘C / ⌘V |
| Font size (per terminal) | | ⌘+ / ⌘- / ⌘0 |
| Clear screen and scrollback | | ⌘K |
| Find in terminal (screen and scrollback) | | ⌘F, then ⌘G / ⇧⌘G or Enter / ⇧Enter, Esc to close; ⌘E finds the selection |
| Find in page (browser) | | ⌘F, same keys as find in terminal |
| Browser back / forward / reload | arrows and ↻ next to the address bar | ⌘[ / ⌘], ⌘R |
| Web inspector / JS console | | ⌥⌘I / ⌥⌘C |

## Performance

`scripts/perf.py run` builds a release binary and measures it:

- binary size
- headless VT parse throughput (`abromux bench-vt`)
- startup time
- idle memory and CPU
- memory per terminal
- in-app throughput
- frame draw time and late frames during in-app throughput
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
- `src/browser.rs`: the webview pane
- `src/workspace.rs`: the workspace, split-tree and surface model
- `src/main.rs`: the app shell (sidebar, pane tab bars, splits, actions)
- `src/bench.rs`: `abromux bench-vt`, the headless parse benchmark used by `scripts/perf.py`
- `src/server.rs`: the control socket's method handlers; `src/cli.rs`: the `abromux` commands; `src/ipc.rs`: the shared wire format

## Known POC limits

- Native webviews always draw above GPUI content. Hidden tabs and workspaces are hidden explicitly, but popovers can't overlap a visible browser.
- While a webview has keyboard focus, it gets the keystrokes, so ⌘-shortcuts may not reach the app.
