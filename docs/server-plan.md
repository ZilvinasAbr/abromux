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
| Where the state lives | **In the server** (option B). Workspaces, layouts, terminals, agent status, notifications, pins and folders belong to the server; the app shows them and sends requests. *As built (phase 2):* the server owns the terminals, the control socket and a copy of the workspaces' state; the app still runs the handlers while it's attached, and the server stands in while it's away. |
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

## Wire protocol (revision 3)

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
| `DETACH` 7 | app → server | the app quits: what's left goes out, then `DETACHED`, and output after it goes to the backlog |
| `DETACHED` 8 | server → app | the last frame of a detached stream, so the app's saved screen and the backlog meet exactly |

A stream that closes without `KILL` lets go, like quitting: the server keeps
reading the PTY into the backlog (16 MiB) until someone attaches again. A
streamed `attach` with `"take":true` takes a terminal from another streamed
connection: the app coming back over a new connection after the old one
broke, before the server noticed.

`abromux-server --stdio` connects to the local server's socket (starting it if
needed) and copies stdin and stdout to it, so any byte pipe (`ssh`) can reach
it. Descriptors can't cross it, which is why remote terminals use `stream`.

Revision 3 adds (`abromux_server`'s module docs have the details):

- `hello` reports `host_id`, `control` (the control socket) and `app`.
- `app` (the app's link: `{"open":N}`, `{"release":id}`, `{"reclaim":id}`
  from the server; `{"op":"state"}`, `{"op":"terms"}` from the app),
  `control` (`conn` N: the app's side of a control connection; none: this
  connection is one), `get-state`, `terms`, `connect` (a TCP port there,
  spliced), `ports`.
- A streamed `attach` takes `owner` (which machine), `fresh`, and `vt` (the
  screen as VT text); its reply adds `screen` and `parsed` (the snapshot is
  the server's own).
- Several streamed clients per terminal; a streamed attach to a terminal a
  local app holds waits while the app is asked to share it.

## Phases

Each phase lands as small PRs, with macOS behaving the same after each.

### Phase 1: the holder becomes the server; remote terminals

- [x] `crates/abromux-server`: the holder's loop moves here, with no gpui
      and no libghostty. Binary `abromux-server`. The app's `abromux server`
      (and the `abromux hold` alias) runs it.
- [x] Socket `server.sock` (`$ABROMUX_SERVER_SOCKET`, `$ABROMUX_HOLD_SOCKET`
      still honoured). Attaching falls back to `hold.sock`, so terminals held
      by the previous version come back after the update.
- [x] `hello`, revision 2.
- [x] `stream` spawn/attach, frames, snapshot handed to the server.
- [x] `--stdio` bridge.
- [x] The server sleeps in `poll` with no timeout unless an idle exit is due
      (the holder wakes every second today).
- [x] App: `Pty::Remote`, a terminal streamed from a server at an endpoint
      (`ssh://[user@]host[:port]`, `unix:///path`).
- [x] App: a workspace can be bound to a host (`abromux new-workspace --host
      ENDPOINT`); its terminals start there and come back after a restart.
      Interim: until phase 3 the workspace itself is still the app's.
- [x] Docs: running `abromux-server` on a VPS (build, PATH, linger).

**Done when:** a workspace on a Linux box over ssh runs agents that keep
going when the app quits, and come back with their screens; `perf.py run`
shows no change for local terminals.

Status: done on branch `server`. Checked against a Debian container over
real ssh (`ABROMUX_E2E_HOST=ssh://box cargo test -p abromux-term e2e --
--ignored`) and in the app: a ticker kept counting while the app was closed
and came back with no gap. `perf.py run`: startup, throughput, app CPU and
per-terminal memory within the range of earlier runs.

Left for later in this phase: a reconnect after a broken connection
replays only what the server had kept, so a full-screen program may need a
resize to redraw; the server's `exit_code` waits up to 300 ms on its loop
for a program to be reaped.

### Phase 2: the model leaves GPUI; the server owns the control socket

- [x] `crates/abromux-model`: workspaces, the pane/tab tree (surface ids,
      not GPUI entities), groups, status, progress, unread, selection;
      events (`Workspace`, `Removed`, `Selected`) with a gapless `seq`,
      `diff` and `apply`. Unit-tested without a window: any change's diff,
      applied, gives the new model; a gap is refused, a resend ignored.
- [x] Model methods vs view methods: every control method is one or the
      other (`api::kind`, shown by `abromux rpc describe`); a test keeps
      what the server takes for an absent app to model methods.
- [x] **The server is the source of the model.** It orders every change:
      the app *proposes* what changed in its views (`{"op":"change"}`,
      unstamped events from `diff`), the server stamps them (a gapless
      `seq`), applies them and sends them back (`applied`). The app keeps a
      replica of the server's model, changed only by what the server sends
      (its model when the app attaches or asks, `sync`; the stamped
      events), on its main thread together with its views. Remote servers
      have a replica of the workspaces with something on them.
- [x] **The server runs the model methods it owns itself**
      (`abromux_server::owned`): it answers `workspace.list`,
      `workspace.current`, `status.list` and `log.list` from its model,
      and changes it for status, progress, the log, a workspace's name,
      description and colour (`workspace.action`), the selection, the
      order (`workspace.reorder`) and a tab's name (`surface.rename`),
      whether or not the app is attached: it reads the first request on each
      control connection, changes its model, answers the caller, and has the
      app mirror the change in its views (`{"mirror":..}`, with the stamped
      events), or keeps it for when the app is back. Everything else is
      spliced to the app's handlers, which propose their changes as above.
- [x] Any client gets the model (`{"op":"model"}`) or a snapshot then every
      event (`{"op":"subscribe"}`).
- [x] The control socket (`abromux.sock`) is the server's. While the app is
      attached, each connection is handed to it and spliced to its
      handlers; while it's away, the server answers model methods from its
      state (`list-workspaces`, status/log/notification lists), keeps the
      ones that change it (status, progress, log, notifications, agent
      reports) and replays them when the app is back; view methods are
      refused (`app_closed`). One app at a time; a server without a control
      socket leaves the app binding it itself.
- [x] The server saves its state, the replica and the app's session
      (`server-state.json`). The app bootstraps from that snapshot at
      launch when it's newer than its own session file or the file is gone
      (checked: deleting `session.json` and relaunching brought every
      workspace back from the server, terminals and all); a lost workspace
      is opened again from a remote server too (phase 3).
- [x] Server-side folders (`abromux_server::folder`, the same files and
      rules as the app's Lua module): `abromux-server folder new|add|list|
      all|path|remove`, and `abromux new-workspace --host H --folder NAME`
      makes the folder on H and puts the workspace's terminals in it.

What stays in the app: the methods that make or close views (a workspace,
a split, a tab, a terminal, a browser, and browser automation, input and
focus): the app does them and proposes the change, and the server stamps it.
A view can only be made where it's shown. Which methods the server owns is
a list (`owned::OWNED`, `owned::READS`); a test keeps them model methods.

**Done when:** quitting the app leaves every workspace's state (status pills,
progress, notifications, agents' state) live in the server, and the app picks
it back up at launch. *Met:* checked in the app (status, notification and log
reported while it was closed came back on the right workspace and tab); the
replica followed a rename live on a VPS's server (`subscribe`).

### Phase 3: the app as a client of several servers

- [x] Servers by name: Settings › Servers (add, check, remove), ⌘⇧P "New
      Workspace on …" / "New Tab on …", `abromux servers`, `--host NAME`.
      In `servers.json`, not `config.json` (app versions share that file and
      refuse keys they don't know).
- [x] Several addresses per server (`also`), tried in order by terminals,
      links and checks; the first answer's host id is kept, a different
      machine at the address is reported.
- [x] A control link to every host with something of the app's on it
      (`control_link`, over ssh `ControlMaster`), reconnecting with backoff;
      a host's workspaces fade while it's away.
- [x] The sidebar groups by host: a new workspace on a server joins that
      server's group (made, named after it, if there's none), unless it's
      put in another; ⌘⇧P Group Workspaces by Server (`group by-server`)
      for older ones. ⌘P lists every host's workspaces, says which server
      each is on, and finds them by the server's name or address.
- [x] `abromux --server NAME …`; agents' hooks on a host reach the app
      through that host's control socket (remote shells get
      `ABROMUX_SOCKET_PATH`, `ABROMUX_BIN`, `ABROMUX_SURFACE_ID`). On a
      host without the app, `abromux-server` is the `abromux` they call
      (`hooks install`, `notify`, `set-status`, ..., `call`).
- [x] The remote server keeps the app's workspaces there (also ones the app
      lost, while their terminals run): ⌘⇧P Open Workspaces from <server>.

### Phase 4: several clients per terminal; server-side parsing

- [x] `abromux-vt` (libghostty-vt in its own crate); the server parses a
      terminal whenever no local app holds it (feature `vt`, on in the app's
      server): titles, folders, answers to the program's queries, and
      notifications printed while nobody watches, kept for the app. Clients
      coming back get the server's whole screen.
- [x] A streamed attach to a terminal a local app holds: the app is asked
      (`{"release":id}`) and streams it from its own server; the waiting
      client is served; once it leaves, the app holds it again
      (`{"reclaim":id}`). Several streamed clients each get the output, any
      may type; `take` replaces only the same machine's connection.
- [x] The app reports the titles and folders of the terminals it holds
      (`{"op":"terms"}`), which the server lists.

### Phase 5: browser panes and ports

- [x] `abromux forward <port>`: 127.0.0.1:<local> here reaches <port> on the
      workspace's host through its server (`{"op":"connect"}`); per
      workspace, saved with the session (same local port), closed with it.
- [x] `abromux forward ports`: what listens on the host.
- [x] A dev server starting on a host is offered: the app has each remote
      server watch its listening ports (`watch-ports`, a look every 3 s,
      only while asked), and a new one shows a notification and ⌘⇧P
      "Forward Port N of <server>", which forwards it in that server's
      workspace.

### Phase 6: Tailscale and other clients

- [x] `--tailnet [PORT]` (7297): on the tailnet addresses, callers
      identified by `tailscale whois`; the owner, plus `--tailnet-allow`
      logins or tags. Apps use `tailnet://host`. (The CLI rather than the
      LocalAPI socket: one less thing to get right per OS.)
- [x] `abromux-server install [--tailnet ..]`: a systemd user unit;
      `loginctl enable-linger` reminder.
- [x] `scripts/package-server.sh` (tarball, `.deb`), AUR `abromux-server`.
- [x] A web client on the tailnet listener for a phone's browser: the
      terminals, each in xterm.js over a WebSocket streaming it.

## Decided along the way

- **TERM on hosts:** `xterm-256color`, the same as local terminals (the
  app's shells use it too); nothing to install on a host.
- **The `abromux` CLI on a host without the app:** `abromux-server` takes
  what agents and scripts use there (hooks, `notify`, `set-status`,
  `set-progress`, `log`, `list-workspaces`, `folder`, `ping`), and `call
  <method> [json]` for any other method; the full CLI is the app's binary,
  which needs a desktop.
- **The phone:** the Mac's phone access (main's) and the server's web client
  serve different machines; the web client is for a host without a Mac in
  front of it. They share the HTTP and WebSocket code.
- **Multi-user servers** (Rex's login server): out of scope; one server per
  user account, which is what ssh and Tailscale identity give.
