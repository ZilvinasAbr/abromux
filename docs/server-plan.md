# AbroMux Server plan

Goal: AbroMux runs as a **server** that can live on a Linux VPS (or any
machine) and keep workspaces, terminals and their agents going there, while
the AbroMux app on a Mac (or Linux desktop, later a phone) is one of its
**clients**. A client can be connected to several servers at once. The local
case, where the app and the server are on the same Mac, must stay as fast as
it is today.

The model is [Rex](https://www.superlogical.com/rex/docs)'s: every machine
runs its own server, servers don't know about each other, and clients keep a
list of hosts.

## Decisions

| Question | Decision |
|---|---|
| Where the state lives | **In the server** (option B). Workspaces, layouts, terminals, agent status, notifications, pins and folders belong to the server; the app shows them and sends requests. |
| The holder | **Grows into the server.** `abromux hold` becomes `abromux server`: the same long-lived process per machine, on the Mac and on a VPS. `abromux hold` stays as an alias. |
| Local terminals | **The server stays out of the data path.** An app on the same machine gets the PTY's descriptor (`SCM_RIGHTS`), exactly as from the holder today, and reads and writes it itself. |
| Remote terminals | Streamed over the connection, in frames. The app parses them with libghostty-vt as it does local ones, so scrollback, find and selection stay local. |
| Headless build | `crates/abromux-server`, a crate and a binary (`abromux-server`) with no gpui, no Wayland, no libghostty: it builds on any Linux box with just cargo. The app's `abromux server` runs the same code. |
| Transport | **SSH first**: `ssh HOST abromux-server --stdio` bridges to the server's socket there. Auth is SSH's (keys, agent, Tailscale SSH), one connection per host with `ControlMaster`. A Tailscale listener (`tailnet://`, callers identified with tailscaled's whois) comes later, for clients that can't run ssh (a phone). No embedded tsnet. |
| Several servers | Independent. A workspace belongs to exactly one server and can't move (its processes run there). The app groups workspaces by host. Hosts are a label and endpoints tried in order; the list lives in the app's settings first, a shared "home" list later. |
| Version skew | Every connection starts with `hello`, which reports the protocol revision. An older server answers `bad request`, which means revision 1 (the holder). Features check the revision. |
| Upgrades | A server from an older app keeps running its terminals until they end; the new app takes those terminals back over the old protocol, which revision 2 still speaks. |

## Performance rules

The local app is the case that matters most. Every phase checks these with
`scripts/perf.py run` (and `--headless`) against the last run on `main`.

1. **The local data path doesn't change.** PTY bytes go from the PTY to the
   app's reader thread, as today. The server never reads, copies or parses a
   terminal while a local app holds it. *Checked by:* throughput (plain,
   colored, CJK) and per-terminal memory within noise.
2. **No round trip while drawing.** The app keeps a replica of the server's
   state (a snapshot, then events in order) and draws from it. Requests
   (split, switch, rename) go to the server and come back as events; none
   blocks a frame.
3. **Events are coalesced.** A burst of status, progress or log events from
   agents costs one redraw per frame, not one per event.
4. **The local wire is lean.** JSON for requests and the CLI is fine; terminal
   bytes are never JSON, and the local snapshot at connect is read once.
5. **The server idles at 0%.** No polling loops: it sleeps in `poll` until
   something happens. Timers only while something is pending (the idle exit).
6. **Startup stays as fast.** Launch → socket answers and launch → prompt are
   measured at each phase. An already-running server should make startup
   faster, not slower.
7. **Remote costs stay remote.** Double parsing (server and app), framing and
   backlogs are only paid for terminals that are actually streamed.

## Architecture (target)

```
   AbroMux app (Mac)                         VPS
 ┌──────────────────────────┐       ┌──────────────────────────┐
 │ GPUI views               │       │ abromux-server           │
 │ replica per host  ◄──────┼─ ssh ─┼► model, PTYs, folders,   │
 │ libghostty-vt per term   │stream │  agent status, control   │
 └────▲──────────┬──────────┘       │  socket for its shells   │
  fd  │          │ requests/events  └──────────────────────────┘
 ┌────┴──────────▼──────────┐
 │ abromux server (local)   │  same code, same protocol; hands
 │ model, PTYs, control sock│  the local app PTY descriptors
 └──────────────────────────┘
```

Crates:

```
crates/
  abromux-core/     wire format (frames, hello), settings, ipc, ... (no deps)
  abromux-server/   the server: PTYs, terminals' lifetimes, (later) the model,
                    listeners (unix, stdio, tailnet). Binary: abromux-server.
  abromux-term/     libghostty-vt, the app's PTY side: Held (descriptor),
                    Remote (stream)
  abromux-model/    (phase 2) workspaces, panes, surfaces, status: pure Rust,
                    ids instead of gpui entities
```

## Wire protocol (revision 2)

A connection to the server's socket starts in **line mode**: one JSON request
per line, one JSON reply per line.

- `{"op":"hello"}` → `{"ok":true,"server":"abromux","protocol":2,"version":".."}`
- `{"op":"spawn",..}` / `{"op":"attach","id":..}`: as the holder's (revision 1):
  the reply carries the PTY's descriptor, and `attach` is followed by the backlog.
- `{"op":"spawn",..,"stream":true}` / `{"op":"attach","id":..,"stream":true}`:
  the reply has no descriptor. For `spawn`, an empty `argv` means the
  server's user's login shell, and `"inherit_env":true` starts from the
  server's environment with `env` on top (a remote app's PATH means nothing
  there). `attach` replies `{"ok":true,"pid":n,"snapshot":len,"backlog":len}`
  followed by that many bytes of each. Then the connection switches to
  **frames**: `[type: u8][len: u32 big-endian][payload]`.

| Frame | Direction | Payload |
|---|---|---|
| `OUTPUT` 1 | server → app | PTY bytes |
| `EXIT` 2 | server → app | JSON `{"code":n\|null}`; the program ended |
| `INPUT` 3 | app → server | bytes for the PTY |
| `RESIZE` 4 | app → server | cols u16, rows u16, width u16, height u16 (pixels) |
| `SNAPSHOT` 5 | app → server | the screen as the app last saw it (`Terminal::snapshot`); the next `attach` sends it back |
| `KILL` 6 | app → server | end the program and forget the terminal |

A stream that closes without `KILL` lets go, like quitting: the server keeps
reading the PTY into the backlog (16 MiB) until someone attaches again.

`abromux-server --stdio` connects to the local server's socket (starting it if
needed) and copies stdin and stdout to it, so any byte pipe (`ssh`) can reach
it. Descriptors can't cross it, which is why remote terminals use `stream`.

## Phases

Each phase lands as small PRs, with macOS behaving the same after each.

### Phase 1: the holder becomes the server; remote terminals

- [ ] `crates/abromux-server`: the holder's loop moves here, with no gpui
      and no libghostty. Binary `abromux-server`. The app's `abromux server`
      (and the `abromux hold` alias) runs it.
- [ ] Socket `server.sock` (`$ABROMUX_SERVER_SOCKET`, `$ABROMUX_HOLD_SOCKET`
      still honoured). Attaching falls back to `hold.sock`, so terminals held
      by the previous version come back after the update.
- [ ] `hello`, revision 2.
- [ ] `stream` spawn/attach, frames, snapshot handed to the server.
- [ ] `--stdio` bridge.
- [ ] The server sleeps in `poll` with no timeout unless an idle exit is due
      (the holder wakes every second today).
- [ ] App: `Pty::Remote`, a terminal streamed from a server at an endpoint
      (`ssh://[user@]host[:port]`, `unix:///path`).
- [ ] App: a workspace can be bound to a host (`abromux new-workspace --host
      ENDPOINT`); its terminals start there and come back after a restart.
      Interim: until phase 3 the workspace itself is still the app's.
- [ ] Docs: running `abromux-server` on a VPS (build, PATH, linger).

**Done when:** a workspace on a Linux box over ssh runs agents that keep
going when the app quits, and come back with their screens; `perf.py run`
shows no change for local terminals.

### Phase 2: the model leaves GPUI

- [ ] `crates/abromux-model`: `Workspace`, `Node`/`Pane`, groups, status,
      progress, log, links and pins, keyed by surface ids instead of
      `EntityId`. Unit-tested without a window.
- [ ] Split `src/server.rs`'s 131 methods into **model methods** (workspaces,
      panes, tabs, status, notifications, folders, links: they change the
      model) and **view methods** (browser automation, screenshots, focus,
      key input: they need the app's views).
- [ ] The app keeps the model in-process first (no behaviour change; perf
      neutral), driving it through the same apply-event path a replica uses.
- [ ] Events with a gapless `seq`; snapshot + events = state. Coalesced into
      one redraw per frame in the app.
- [ ] Then the model moves into `abromux server`. The app connects,
      bootstraps from a snapshot, and keeps a replica. The control socket
      (`abromux.sock`) moves to the server; view methods are forwarded to the
      app that shows the surface (Rex's "client actions").
- [ ] The server saves the session (layouts, held ids). After a reboot it
      restores layouts; programs start fresh (agents resume as today).
- [ ] Server-side folders: `folder add`, cleanup and Docker resources run
      where the workspace lives.

**Done when:** quitting the app leaves every workspace's state (status pills,
progress, notifications, agents' state) live in the server, and the app picks
it back up at launch; perf numbers as before, startup measured.

### Phase 3: the app as a client of several servers

- [ ] Hosts: label + endpoints tried in order, in `config.json`; host id from
      `hello` to warn when an address leads to another machine.
- [ ] One connection per host (ssh `ControlMaster`); reconnect with backoff;
      a host's workspaces go grey while it's away and come back on reconnect.
- [ ] Sidebar: workspaces grouped by host; "New workspace on…"; ⌘P searches
      all hosts.
- [ ] CLI: `abromux --server LABEL …`; agents' hooks on a host talk to that
      host's server through `ABROMUX_SOCKET_PATH` as they do locally.
- [ ] Phase 1's app-owned remote workspaces become the remote server's.

### Phase 4: several clients per terminal; server-side parsing

- [ ] The server parses a terminal (libghostty-vt) whenever no local app
      holds its descriptor: notifications (OSC 9/99/777), titles, cwd and
      agent state keep updating while the app is closed, and the server can
      make a snapshot itself.
- [ ] A second client on a terminal a local app holds: the server takes the
      descriptor back (the app hands over its snapshot) and streams to both.
      Only that terminal pays for it, only while both are attached.
- [ ] While the local app holds a terminal, it reports what it parses
      (notifications, bell, title, cwd, program status) to the server as
      small events.

### Phase 5: browser panes and ports

- [ ] `abromux forward` (Rex's tunnel blocks): a local port that connects to
      a port on the host, per workspace, removed with the workspace.
- [ ] Optionally detect a dev server listening in a remote workspace and
      offer to forward it; or open `http://host:port` directly on a tailnet.

### Phase 6: Tailscale and other clients

- [ ] `tailnet://:7297` listener: plain HTTP/WebSocket on the tailnet
      addresses, callers identified by tailscaled's LocalAPI whois; only the
      machine's owner by default.
- [ ] systemd user unit (`abromux-server install`), `loginctl enable-linger`.
- [ ] Packages for the server alone (AUR, deb).
- [ ] A phone or web client on the same protocol.

## Open questions

- Remote terminals use `TERM=xterm-256color`; installing the `xterm-ghostty`
  terminfo on the host would be better.
- Agent hooks and `abromux` inside a remote shell can't reach the app until
  phase 2/3 move the control socket into the server: in phase 1 they find no
  socket and do nothing.
- Multi-user servers (Rex's login server) are out of scope: one server per
  user account.
