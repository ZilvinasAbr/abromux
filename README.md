# AbroMux

A proof-of-concept of [cmux](https://cmux.com) built with Rust and [GPUI](https://gpui.rs).

- **Terminal panes** use [libghostty-vt](https://github.com/ghostty-org/ghostty) for all terminal state: VT parsing, screen and scrollback, colors, selection and key encoding. It is linked statically through a bindgen FFI layer. A shell runs in a PTY (`portable-pty`) and GPUI draws each cell. Text goes through the macOS input system, so IMEs and dead keys compose as usual. Mouse clicks, drags and the wheel go to TUI apps that ask for them (vim, htop, lazygit); hold Shift to select or scroll locally. Full-screen apps without mouse support get arrow keys from the wheel, like Ghostty's alternate scroll.
- **Browser panes** are native WKWebViews (wry, via `gpui-component`). Each has an address bar with back, forward and reload.
- **Workspaces** sit in a left sidebar, cmux style. Each entry shows the name, the focused tab's title, the git branch and the cwd. A blue dot means a bell rang in that workspace while you were elsewhere.
- **Panes** split right or down, and each pane has a tab bar of terminal and browser tabs. Drag a divider to resize.

## Requirements

- macOS on Apple Silicon (the libghostty build script fetches an aarch64 Zig)
- Rust stable (pinned via `rust-toolchain.toml`)
- Xcode Command Line Tools (clang for bindgen, `codesign`)

## Run

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

## Session restore

Quitting saves the session to `~/Library/Application Support/abromux/session.json` (the layout is also saved every few seconds, so a crash loses little). The next launch restores the window frame, workspaces and their names, the split layout and ratios, tabs, each terminal's working directory and font size, browser URLs, and up to 2000 lines of each terminal's output (with colors). Shells start fresh: running programs aren't kept alive. `ABROMUX_NO_RESTORE=1` skips it; `ABROMUX_SESSION_PATH` uses another file. `ABROMUX_DEMO` and `ABROMUX_SCRIPT` runs don't touch the session unless `ABROMUX_SESSION_PATH` is set.

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
abromux --json list-panes
```

Targets are refs (`workspace:2`, `pane:5`, `surface:3`) or 0-based indexes. Each shell gets `ABROMUX_SOCKET_PATH`, `ABROMUX_WORKSPACE_ID` and `ABROMUX_SURFACE_ID`, so `abromux` run inside a pane acts on that pane by default (otherwise on what's focused). The wire format is one JSON object per line (`{"id":1,"method":"workspace.list","params":{}}`); `abromux rpc <method> [json]` sends one directly.

## CRUD

| | Mouse | Keys |
|---|---|---|
| New workspace | `+` in sidebar header / footer | ⌘N |
| Rename workspace | double-click its name (Enter saves, Esc cancels) | ⌘⇧R |
| Delete workspace | hover → `×` | ⌘⇧W |
| Select workspace | click | ⌘1…⌘8, ⌘9 for the last, ⌃⌘[ / ⌃⌘] (hold ⌘ to see the numbers) |
| New terminal tab | terminal icon in pane tab bar | ⌘T |
| New browser tab | globe icon | ⌘⇧L (⌘L focuses address bar) |
| Split right / down | split icons | ⌘D / ⌘⇧D |
| Close tab | tab `×` / middle-click | ⌘W (last tab closes the pane; last pane closes the workspace) |
| Cycle tabs | | ⌘⇧[ / ⌘⇧], ⌃Tab / ⌃⇧Tab |
| Select tab in focused pane | click | ⌃1…⌃8, ⌃9 for the last (hold ⌃ to see the numbers) |
| Focus pane in a direction | click | ⌥⌘← → ↑ ↓ |
| Flash focused pane | | ⌘⇧H |
| Toggle sidebar | sidebar icon | ⌘B |
| Copy / paste in terminal | drag to select, double-click selects a word | ⌘C / ⌘V |
| Font size (per terminal) | | ⌘+ / ⌘- / ⌘0 |
| Clear screen and scrollback | | ⌘K |

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

History lives in `<git common dir>/abromux-perf/history.jsonl`, outside the tree. All worktrees share it and commits never touch it; `$ABROMUX_PERF_HISTORY` overrides the location. [`docs/perf-vs-cmux.md`](docs/perf-vs-cmux.md) compares abromux with cmux, measured with `scripts/bench.sh`.

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

## License

[MIT](LICENSE). libghostty-vt is part of [Ghostty](https://github.com/ghostty-org/ghostty) (MIT).
