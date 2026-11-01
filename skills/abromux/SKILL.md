---
name: abromux
description: "Control AbroMux, the macOS terminal app this agent may be running in: open splits, tabs and workspaces, run commands in other terminals and read their output, and report progress in the sidebar (status pills, progress bar, log, notifications). Use when the user mentions abromux, asks to run something in another pane/split/tab, watch a dev server or tests beside you, or to notify them when you're done. Only works inside abromux ($ABROMUX_SURFACE_ID is set). For driving the built-in browser, use the abromux-browser skill."
---

# AbroMux

AbroMux is a terminal app with workspaces (in a left sidebar), split panes, and tabs of terminals and browsers in each pane. The `abromux` CLI controls the running app over a Unix socket.

## Check first

```bash
test -n "${ABROMUX_SURFACE_ID:-}" && abromux identify
```

If `ABROMUX_SURFACE_ID` is empty, you aren't inside abromux: say so and stop. Don't drive the user's app from outside it. `identify` prints which surface you are (`caller`) and what the user has focused.

`abromux --help` is the authority on commands and flags. Read it when something here doesn't cover your case.

## The model

- **workspace** (`workspace:2`): one sidebar entry, with its own split layout.
- **pane** (`pane:5`): one split region. Each pane has a tab bar.
- **surface** (`surface:3`): one tab, a `terminal` or a `browser`.

Refs are stable while the thing exists; use them, not indexes (`--surface 2` is a 0-based index and shifts as tabs come and go).

**Default target is you.** Without `--surface`/`--workspace`, commands act on the terminal you run in. `abromux send 'x'` with no `--surface` types into your own terminal. Always pass `--surface` for another terminal, and never run `close-surface` without one: it closes your own.

```bash
abromux tree                 # this workspace: panes and their surfaces (* = focused/active)
abromux tree --all           # every workspace
abromux list-workspaces
abromux --json tree          # machine-readable; any command takes --json
```

## Run something beside you

Creating a split, tab or workspace focuses it by default, which takes the keyboard away from the user's conversation with you. Pass `--no-focus` unless the user asked to be taken there.

```bash
abromux new-split right --no-focus --command 'npm run dev'
# OK surface:7 pane:4 workspace:1
```

The first ref printed is the new terminal. Keep it; every later command needs it.

- `new-split <left|right|up|down>`: splits your pane (or `--surface`'s).
- `new-surface`: a new tab in your pane (or `--pane`).
- `new-workspace --name <title> --cwd <dir> [--description <text>] [--group group:N]`: a new sidebar entry.
- `new-workspace --name <title> --cwd <dir> --layout '<json>'`: a new workspace already split, in cmux's layout format: `{"direction": "horizontal", "split": 0.5, "children": [{"pane": {"surfaces": [{"command": "npm run dev"}]}}, {"pane": {"surfaces": [{"type": "browser", "url": "localhost:3000"}]}}]}`. `--layout @file.json` reads it from a file.
- All take `--command <text>` (typed into the new shell, then Enter), `--cwd <dir>` and `--type browser --url <url>`.

## Talk to another terminal

```bash
abromux send --surface surface:7 'cargo test\n'     # \n presses Enter; also \t \e \\
abromux send-key --surface surface:7 ctrl-c          # enter, tab, escape, up, down, ctrl-c, alt-b, ...
abromux read-screen --surface surface:7              # what's visible now
abromux read-screen --surface surface:7 --scrollback --lines 200   # include scrollback, last 200 lines
```

`send` without a trailing `\n` types text but doesn't run it. There is no wait-for-output command: poll `read-screen` with a short `sleep` between reads until the prompt or the line you expect appears, and give up after a reasonable time.

Don't type into a terminal the user is working in, or one running another agent, unless the user asked you to. Check its title in `tree` first.

Clean up what you started when you're done with it: `abromux close-surface --surface surface:7`. Leave long-running things the user wants to keep (dev servers) open and say which surface they're in.

## Report to the user

These show up in the sidebar under your workspace, so the user can see progress without looking at your terminal.

```bash
abromux set-status tests running --color '#e0a84f'   # a pill; same key replaces it
abromux clear-status tests
abromux set-progress 0.4 --label 'Migrating 4/10'    # a bar, 0.0-1.0
abromux clear-progress
abromux log --level success --source deploy 'Deployed to staging'   # info|progress|success|warning|error
abromux notify --title 'Done' --body 'All 213 tests pass'
```

`notify` marks your terminal: it rings the pane, dots the tab and workspace, and shows a macOS banner if abromux is in the background. Use it once when a long task finishes or you need the user, not for every step. Coding agents with hooks (`abromux hooks install`: Claude Code, Codex, Gemini CLI, Droid, Cursor, OpenCode, Pi) already notify when they finish a turn or ask for permission, so don't duplicate that.

Clear status pills and progress you set once they no longer apply.

## Other commands

```bash
abromux open ~/code/other-repo           # a directory opens a workspace; a URL opens a browser split
abromux rename-workspace 'auth refactor' # your workspace
abromux workspace-action set-description 'Migrating auth to OAuth'   # a line under its name
abromux workspace-action set-color Teal  # cmux's color names or #rrggbb; clear-color
abromux group list                       # sidebar groups; group add --group group:1 puts yours in one
abromux focus-surface --surface surface:7    # take the user there (only when asked)
abromux move-surface surface:7 --workspace workspace:2   # move a tab; or --pane pane:4 [--index 0]
abromux split-off --surface surface:7 right  # a tab into its own pane beside; focus stays put
abromux move-tab-to-new-workspace --surface surface:7 --title 'dev server'
abromux list-notifications
abromux clear-history --surface surface:7    # like ⌘K
abromux rpc <method> '<json>'            # raw socket call
```

The user may be running AbroMux Nightly. `abromux` still talks to the right app: it uses `$ABROMUX_SOCKET_PATH`, which each terminal gets.
