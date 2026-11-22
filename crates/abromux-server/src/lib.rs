//! `abromux server` (formerly `abromux hold`): the process that keeps
//! terminals running for abromux, on this machine or for an app on another
//! one (docs/server-plan.md).
//!
//! It opens the PTYs and is the shells' parent, so they live as long as it
//! does. One per app channel on a Mac (started on demand, detached), one per
//! user on a server. It parses nothing.
//!
//! An app on this machine takes a terminal's PTY itself: the server hands it
//! the master's descriptor over the Unix socket (`SCM_RIGHTS`) and leaves it
//! alone, so a terminal costs nothing extra while the app runs. An app on
//! another machine can't take a descriptor, so the server streams the
//! terminal to it in frames ([`abromux_core::wire`]). Either way, once the
//! app lets go (quit, crash, lost connection), the server reads the PTY into
//! a backlog, so the program never blocks on a full PTY, and hands it over
//! with the next attach.
//!
//! Protocol: JSON lines, then frames for a streamed terminal. See
//! docs/server-plan.md, "Wire protocol".
//!
//! - `{"op":"hello"}` → `{"ok":true,"server":"abromux","protocol":3,"version":"..","host":"..",
//!   "control":"/path/abromux.sock"|null,"app":bool}`
//! - `{"op":"app"}`: this connection is the app's link (the control socket,
//!   [`control`]): the server sends `{"open":N}` for each control connection
//!   to hand over, and the app sends `{"op":"state",..}` when its
//!   workspaces change.
//! - `{"op":"control","conn":N}`: the app's side of control connection N,
//!   spliced to it (N 0: the calls kept while it was away, replayed).
//! - `{"op":"control"}`: this connection is a control connection, as if it
//!   came in on the control socket (the CLI's `--server`, over ssh).
//! - `{"op":"spawn","argv":[..],"cwd":"..","env":[[k,v],..],"cols":c,"rows":r}`
//!   → `{"ok":true,"id":"..","pid":n}` with the PTY's descriptor
//! - `{"op":"attach","id":".."}` → `{"ok":true,"pid":n,"backlog":len}` with
//!   the descriptor, then `backlog` raw bytes
//! - either with `"stream":true`: no descriptor; `attach` replies
//!   `{"ok":true,"pid":n,"snapshot":len,"backlog":len}` and sends both; then
//!   frames. `spawn` with `"stream":true` also takes `"command"` (run with the
//!   user's shell) and an empty `argv` (the user's login shell), and starts
//!   from the server's environment with `env` on top. A streamed `attach`
//!   with `"take":true` takes the terminal from another streamed connection
//!   (the app's own, broken, that the server hasn't noticed yet).
//! - `{"op":"kill"}` (the connection's terminal) → `{"ok":true}`
//! - `{"op":"status"}` (the connection's terminal, once its output ended)
//!   → `{"ok":true,"code":n|null}`: its exit status, null if a signal ended it

pub mod control;
pub mod service;

use abromux_core::wire::{self, Decoder, Size};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Output kept per terminal while nobody is attached; past it, the oldest
/// goes (and the screen may come back slightly wrong).
pub const BACKLOG_MAX: usize = 16 << 20;
/// With no terminals and no connections for this long, the server exits.
const IDLE_EXIT: Duration = Duration::from_secs(10);
/// Output queued for a streamed app past which the server stops reading the
/// PTY until the app catches up: a slow link slows the program, as a slow
/// terminal would, instead of growing without bound.
const HIGH_WATER: usize = 1 << 20;

/// The socket a server started without `--socket` listens on, and that
/// `--stdio` connects to: `$ABROMUX_SERVER_SOCKET`, else `server.sock` in
/// `~/Library/Application Support/abromux` on macOS (the release app's
/// folder, so an app connecting from elsewhere finds the same server the Mac
/// app uses), or in `$XDG_STATE_HOME/abromux` (`~/.local/state/abromux`) on
/// Linux, which unlike `$XDG_RUNTIME_DIR` outlives the ssh session that
/// started it.
pub fn default_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("ABROMUX_SERVER_SOCKET").filter(|p| !p.is_empty()) {
        return p.into();
    }
    let home = abromux_core::paths::home();
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/abromux")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/state"))
            .join("abromux")
    };
    dir.join("server.sock")
}

/// How the server runs, and what it says about itself in `hello`.
#[derive(Clone, Debug)]
pub struct Options {
    /// The version of the program serving, as `abromux version` prints it.
    pub version: String,
    /// Where to serve the control socket ([`control`]): the app's
    /// `abromux.sock`. None: no control socket.
    pub control: Option<PathBuf>,
    /// Exit once nothing has run and nobody has been connected for a while
    /// (started on demand); false for a service.
    pub idle_exit: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            version: String::new(),
            control: None,
            idle_exit: true,
        }
    }
}

struct Term {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    pid: u32,
    backlog: Vec<u8>,
    /// The screen as a streamed app last saw it ([`wire::SNAPSHOT`]), for
    /// the next streamed attach.
    snapshot: Option<Vec<u8>>,
    /// A local app holding the descriptor: while it does, the server
    /// leaves the PTY alone.
    holder: Option<u64>,
    /// Streamed apps (another machine's, or this one's sharing it): each
    /// gets the output, any may type. The server reads and writes the PTY
    /// for them.
    streams: Vec<u64>,
    /// Streamed attaches waiting for the holder to let go, which the app
    /// was asked to do (`{"release":id}`).
    waiting: Vec<u64>,
    /// Its PTY reported end of file: the program is gone.
    eof: bool,
    /// Input for the PTY it hasn't taken yet: a streamed app's typing, the
    /// parser's answers to the program's queries.
    input: Vec<u8>,
    /// Who it is to the app (`ABROMUX_SURFACE_ID` from its spawn): where a
    /// notification it prints while the app is away goes.
    surface: Option<String>,
    /// What its program last said: its title and folder, as the server
    /// parsed them or the app reported them.
    title: String,
    cwd: String,
    size: (u16, u16),
    /// The server's own copy of the screen, kept while no local app holds
    /// the descriptor (`vt`): notifications while the app is away, answers
    /// to the program's queries, and a snapshot for another client.
    #[cfg(feature = "vt")]
    vt: Option<abromux_vt::Terminal>,
}

impl Term {
    /// A screen to parse into, from `snapshot` (what the app left) or blank.
    #[cfg(feature = "vt")]
    fn start_parsing(&mut self, snapshot: Option<&[u8]>) {
        if self.vt.is_some() {
            return;
        }
        let t = snapshot
            .and_then(|s| abromux_vt::Terminal::from_snapshot(s, true).ok())
            .or_else(|| abromux_vt::Terminal::new(self.size.0, self.size.1, true).ok());
        self.vt = t.map(|mut t| {
            t.resize(self.size.0, self.size.1, 0, 0);
            t.write(&self.backlog);
            let _ = t.take_effects();
            t
        });
    }

    #[cfg(not(feature = "vt"))]
    fn start_parsing(&mut self, _snapshot: Option<&[u8]>) {}

    /// The server's snapshot of the screen, if it parses this terminal.
    fn parsed_snapshot(&mut self) -> Option<Vec<u8>> {
        #[cfg(feature = "vt")]
        if let Some(v) = &mut self.vt {
            return v.snapshot();
        }
        None
    }

    fn parsed_snapshot_available(&self) -> bool {
        #[cfg(feature = "vt")]
        return self.vt.is_some();
        #[cfg(not(feature = "vt"))]
        false
    }

    /// A local app holds it again: it parses for itself.
    fn stop_parsing(&mut self) {
        #[cfg(feature = "vt")]
        {
            self.vt = None;
        }
    }

    /// Parse `bytes`; the notifications the program sent, if any.
    fn parse(&mut self, bytes: &[u8]) -> Vec<(String, String)> {
        #[cfg(feature = "vt")]
        if let Some(v) = &mut self.vt {
            v.write(bytes);
            let fx = v.take_effects();
            self.input.extend_from_slice(&fx.pty_response);
            if fx.title_changed {
                self.title = v.title();
            }
            if fx.pwd_changed {
                self.cwd = v.pwd();
            }
            return fx.notifications;
        }
        let _ = bytes;
        vec![]
    }

    fn resize_to(&mut self, cols: u16, rows: u16) {
        self.size = (cols, rows);
        #[cfg(feature = "vt")]
        if let Some(v) = &mut self.vt {
            v.resize(cols, rows, 0, 0);
        }
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum Mode {
    /// JSON requests, one per line.
    Lines,
    /// A streamed terminal's frames.
    Frames,
    /// The app's link ([`control`]).
    App,
    /// A control connection the server answers itself (the app is away).
    Control,
    /// A control connection waiting for the app to open its side.
    Pending,
    /// Bytes go to and from this other connection as they are.
    Splice(u64),
    /// The app's side of the replay: this many answers still to come.
    Replay(usize),
    /// A streamed attach waiting for a local app to let go of the terminal:
    /// what it sends meanwhile waits too.
    Waiting,
}

struct Client {
    stream: UnixStream,
    /// Bytes read but not handled yet (a partial line).
    buf: Vec<u8>,
    decoder: Decoder,
    mode: Mode,
    term: Option<String>,
    /// Which app it is (a streamed attach's `owner`): `take` only takes
    /// from the same one.
    owner: Option<String>,
    /// Bytes waiting for the socket to take them.
    out: Vec<u8>,
    /// Close once `out` is flushed.
    closing: bool,
}

impl Client {
    fn reply(&mut self, v: Value) {
        self.out.extend_from_slice(format!("{v}\n").as_bytes());
    }

    fn streams(&self) -> bool {
        self.mode == Mode::Frames
    }
}

/// What a polled descriptor is.
enum Polled {
    Listener,
    ControlListener,
    Client(u64),
    Term(String),
}

/// Serve on `path` until idle. Returns at once if a server already listens
/// there.
pub fn serve(path: &Path, opts: &Options) -> io::Result<()> {
    if UnixStream::connect(path).is_ok() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // One server per socket: several starting at once (an app's terminals
    // and its link, all finding none) would each replace the last one's
    // socket and split the terminals between them. The lock is held for as
    // long as this one serves; one that can't take it leaves it to the
    // other, which its starter then connects to.
    let Some(_lock) = lock(&path.with_extension("lock"))? else {
        return Ok(());
    };
    if UnixStream::connect(path).is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    // The control socket, if it's ours to serve: one nothing answers is
    // stale (a crashed app's), one something answers is left alone.
    let control = match &opts.control {
        Some(c) if UnixStream::connect(c).is_err() => {
            if let Some(dir) = c.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let _ = std::fs::remove_file(c);
            let l = UnixListener::bind(c)?;
            std::fs::set_permissions(c, std::fs::Permissions::from_mode(0o600))?;
            l.set_nonblocking(true)?;
            Some(l)
        }
        _ => None,
    };
    let mut s = Server {
        terms: HashMap::new(),
        clients: HashMap::new(),
        next_client: 0,
        next_term: 0,
        opts: Options {
            control: opts.control.clone().filter(|_| control.is_some()),
            ..opts.clone()
        },
        state: control::State::load(control::state_path(path)),
        app: None,
        app_owner: None,
        host_id: host_id(&path.with_file_name("host-id")),
        held_dir: path.with_file_name("held"),
    };
    let mut idle_since = Instant::now();
    let mut buf = vec![0u8; 1 << 16];

    loop {
        // Programs that ended while nobody was attached are dropped.
        s.terms.retain(|_, t| {
            let exited = t.eof || matches!(t.child.try_wait(), Ok(Some(_)));
            !(exited && t.holder.is_none() && t.streams.is_empty())
        });
        // Sleep until something happens; only an idle server wakes on a timer.
        let timeout = if s.opts.idle_exit && s.terms.is_empty() && s.clients.is_empty() {
            let left = IDLE_EXIT.saturating_sub(idle_since.elapsed());
            if left.is_zero() {
                let _ = std::fs::remove_file(path);
                if let Some(c) = &s.opts.control {
                    let _ = std::fs::remove_file(c);
                }
                return Ok(());
            }
            left.as_millis() as i32 + 1
        } else {
            idle_since = Instant::now();
            -1
        };

        let mut fds = vec![pollfd(listener.as_raw_fd(), libc::POLLIN)];
        let mut what = vec![Polled::Listener];
        if let Some(l) = &control {
            fds.push(pollfd(l.as_raw_fd(), libc::POLLIN));
            what.push(Polled::ControlListener);
        }
        for (id, c) in &s.clients {
            let mut ev = 0;
            // Spliced: read while the other side keeps up.
            let keeps_up = match c.mode {
                Mode::Splice(o) => s.clients.get(&o).is_none_or(|o| o.out.len() < HIGH_WATER),
                _ => true,
            };
            if !c.closing && keeps_up {
                ev |= libc::POLLIN;
            }
            if !c.out.is_empty() {
                ev |= libc::POLLOUT;
            }
            fds.push(pollfd(c.stream.as_raw_fd(), ev));
            what.push(Polled::Client(*id));
        }
        for (id, t) in &s.terms {
            let Some(fd) = t.master.as_raw_fd() else {
                continue;
            };
            let mut ev = 0;
            // A local app has the descriptor: hands off. Else read it, for
            // the streamed apps while they keep up, or into the backlog;
            // and write what they typed (or what the parser answers).
            if t.holder.is_none_or(|h| !s.clients.contains_key(&h)) {
                let keeps_up = t
                    .streams
                    .iter()
                    .filter_map(|c| s.clients.get(c))
                    .all(|c| c.out.len() < HIGH_WATER);
                if !t.eof && keeps_up {
                    ev |= libc::POLLIN;
                }
                if !t.input.is_empty() {
                    ev |= libc::POLLOUT;
                }
            }
            if ev != 0 {
                fds.push(pollfd(fd, ev));
                what.push(Polled::Term(id.clone()));
            }
        }
        // SAFETY: `fds` is a valid array of pollfd for its length.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
        if n <= 0 {
            continue;
        }

        for (pfd, w) in fds.iter().zip(what) {
            let re = pfd.revents;
            if re == 0 {
                continue;
            }
            let readable = re & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0;
            match w {
                Polled::Listener => {
                    while let Ok((stream, _)) = listener.accept() {
                        s.add_client(stream);
                    }
                }
                Polled::ControlListener => {
                    while let Some(Ok((stream, _))) = control.as_ref().map(|l| l.accept()) {
                        let cid = s.add_client(stream);
                        s.accept_control(cid);
                    }
                }
                Polled::Client(cid) => {
                    if re & libc::POLLOUT != 0 {
                        s.flush(cid);
                    }
                    if readable {
                        s.read_client(cid, &mut buf);
                    }
                }
                Polled::Term(id) => {
                    if re & libc::POLLOUT != 0 {
                        s.write_input(&id);
                    }
                    if readable {
                        s.read_term(&id, &mut buf);
                    }
                }
            }
        }

        // Connections that are done: let go of their terminals.
        let done: Vec<u64> = s
            .clients
            .iter()
            .filter(|(_, c)| c.closing && c.out.is_empty())
            .map(|(id, _)| *id)
            .collect();
        for cid in done {
            s.drop_client(cid);
        }
    }
}

struct Server {
    terms: HashMap<String, Term>,
    clients: HashMap<u64, Client>,
    next_client: u64,
    next_term: u64,
    opts: Options,
    /// What the control socket answers while the app is away.
    state: control::State,
    /// The app's link, while it's attached.
    app: Option<u64>,
    /// Which app that is (`owner`, as its streams say).
    app_owner: Option<String>,
    /// This machine, as `hello` says: made once, kept beside the socket.
    host_id: String,
    /// Where an app leaves a terminal's screen when it lets go
    /// (abromux_term::hold::snapshot_path), and the server leaves its own.
    held_dir: PathBuf,
}

impl Server {
    fn add_client(&mut self, stream: UnixStream) -> u64 {
        let _ = stream.set_nonblocking(true);
        self.next_client += 1;
        self.clients.insert(
            self.next_client,
            Client {
                stream,
                buf: Vec::new(),
                decoder: Decoder::default(),
                mode: Mode::Lines,
                term: None,
                owner: None,
                out: Vec::new(),
                closing: false,
            },
        );
        self.next_client
    }

    /// A control connection: the app's to answer if it's attached (it
    /// opens its side, `{"op":"control","conn":N}`), else the server's.
    fn accept_control(&mut self, cid: u64) {
        match self.app {
            Some(app) => {
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.mode = Mode::Pending;
                }
                if let Some(a) = self.clients.get_mut(&app) {
                    a.reply(json!({ "open": cid }));
                }
                self.flush(app);
            }
            None => {
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.mode = Mode::Control;
                }
                self.handle_input(cid);
            }
        }
    }

    /// The app attached (`{"op":"app"}`): connections waiting on an older
    /// link are offered again, and what was kept meanwhile is replayed.
    fn attach_app(&mut self, cid: u64) {
        // One app at a time: a second one (another instance) is refused,
        // as it was when the app owned the socket.
        if let Some(other) = self
            .app
            .filter(|o| *o != cid && self.clients.contains_key(o))
        {
            let _ = other;
            let c = self.clients.get_mut(&cid).unwrap();
            c.reply(json!({ "ok": false, "error": "another abromux app is attached" }));
            c.closing = true;
            return self.flush(cid);
        }
        self.app = Some(cid);
        let pending: Vec<u64> = self
            .clients
            .iter()
            .filter(|(_, c)| c.mode == Mode::Pending)
            .map(|(id, _)| *id)
            .collect();
        let c = self.clients.get_mut(&cid).unwrap();
        c.mode = Mode::App;
        c.reply(json!({ "ok": true }));
        for p in pending {
            c.reply(json!({ "open": p }));
        }
        if !self.state.kept.is_empty() {
            c.reply(json!({ "open": 0 }));
        }
        self.flush(cid);
    }

    /// The app's side of control connection `conn`.
    fn open_control(&mut self, cid: u64, conn: u64) {
        if conn == 0 {
            let kept = self.state.take_kept();
            let c = self.clients.get_mut(&cid).unwrap();
            for (i, k) in kept.iter().enumerate() {
                c.reply(json!({ "id": i + 1, "method": k["method"], "params": k["params"] }));
            }
            c.mode = Mode::Replay(kept.len());
            if kept.is_empty() {
                c.closing = true;
            }
            return self.flush(cid);
        }
        let ready = self
            .clients
            .get(&conn)
            .is_some_and(|c| c.mode == Mode::Pending);
        if !ready {
            // The caller is gone already.
            if let Some(c) = self.clients.get_mut(&cid) {
                c.closing = true;
            }
            return self.flush(cid);
        }
        // What the caller sent while it waited goes over first.
        let waiting = {
            let p = self.clients.get_mut(&conn).unwrap();
            p.mode = Mode::Splice(cid);
            std::mem::take(&mut p.buf)
        };
        let c = self.clients.get_mut(&cid).unwrap();
        c.mode = Mode::Splice(conn);
        c.out.extend_from_slice(&waiting);
        let rest = std::mem::take(&mut c.buf);
        if let Some(p) = self.clients.get_mut(&conn) {
            p.out.extend_from_slice(&rest);
        }
        self.flush(cid);
        self.flush(conn);
    }
    fn read_client(&mut self, cid: u64, buf: &mut [u8]) {
        let gone = loop {
            let Some(c) = self.clients.get_mut(&cid) else {
                return;
            };
            match c.stream.read(buf) {
                Ok(0) => break true,
                Ok(n) => match c.mode {
                    Mode::Frames => c.decoder.push(&buf[..n]),
                    Mode::Splice(o) => {
                        if let Some(oc) = self.clients.get_mut(&o) {
                            oc.out.extend_from_slice(&buf[..n]);
                        }
                        self.flush(o);
                    }
                    _ => c.buf.extend_from_slice(&buf[..n]),
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break false,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break true,
            }
        };
        // What it sent before hanging up still counts (input typed just
        // before a quit).
        self.handle_input(cid);
        if gone {
            self.drop_client(cid);
        }
    }

    /// Handle what a connection has sent: requests while in line mode, then
    /// frames once it streams (a request can switch it mid-buffer).
    fn handle_input(&mut self, cid: u64) {
        loop {
            let Some(c) = self.clients.get_mut(&cid) else {
                return;
            };
            match c.mode {
                Mode::Lines => {
                    let Some(nl) = c.buf.iter().position(|&b| b == b'\n') else {
                        return;
                    };
                    let line: Vec<u8> = c.buf.drain(..=nl).collect();
                    let req: Value = serde_json::from_slice(&line).unwrap_or(Value::Null);
                    self.request(&req, cid);
                    // Now streaming: what followed the request is frames.
                    if let Some(c) = self.clients.get_mut(&cid)
                        && c.streams()
                    {
                        let rest = std::mem::take(&mut c.buf);
                        c.decoder.push(&rest);
                    }
                }
                Mode::Frames => match c.decoder.next_frame() {
                    Ok(Some((kind, payload))) => self.frame(cid, kind, payload),
                    Ok(None) => return,
                    Err(_) => return self.drop_client(cid),
                },
                Mode::Control | Mode::App | Mode::Replay(_) => {
                    let Some(nl) = c.buf.iter().position(|&b| b == b'\n') else {
                        return;
                    };
                    let line: Vec<u8> = c.buf.drain(..=nl).collect();
                    match c.mode {
                        Mode::Control => {
                            if line.iter().all(u8::is_ascii_whitespace) {
                                continue;
                            }
                            let reply = match serde_json::from_slice::<Value>(&line) {
                                Ok(req) => self.state.handle(&req),
                                Err(e) => {
                                    json!({ "ok": false, "error": format!("bad request: {e}") })
                                }
                            };
                            let c = self.clients.get_mut(&cid).unwrap();
                            c.reply(reply);
                            self.flush(cid);
                        }
                        Mode::App => {
                            let msg: Value = serde_json::from_slice(&line).unwrap_or(Value::Null);
                            if msg["op"] == "state" {
                                let terms = &self.terms;
                                self.state
                                    .update(&msg, &|id| terms.get(id).is_some_and(|t| !t.eof));
                            } else if msg["op"] == "terms" {
                                // What the app parses of the terminals it
                                // holds: their titles and folders.
                                for r in msg["terms"].as_array().into_iter().flatten() {
                                    if let Some(t) =
                                        r["id"].as_str().and_then(|id| self.terms.get_mut(id))
                                    {
                                        if let Some(v) = r["title"].as_str() {
                                            t.title = v.to_string();
                                        }
                                        if let Some(v) = r["cwd"].as_str() {
                                            t.cwd = v.to_string();
                                        }
                                        if let Some(v) = r["surface"].as_str() {
                                            t.surface = Some(v.to_string());
                                        }
                                    }
                                }
                            }
                        }
                        Mode::Replay(left) => {
                            let left = left.saturating_sub(1);
                            c.mode = Mode::Replay(left);
                            if left == 0 {
                                c.closing = true;
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                Mode::Pending | Mode::Splice(_) | Mode::Waiting => return,
            }
        }
    }

    fn frame(&mut self, cid: u64, kind: u8, payload: Vec<u8>) {
        let Some(id) = self.clients.get(&cid).and_then(|c| c.term.clone()) else {
            return;
        };
        match kind {
            wire::INPUT => {
                if let Some(t) = self.terms.get_mut(&id) {
                    t.input.extend_from_slice(&payload);
                }
                self.write_input(&id);
            }
            wire::RESIZE => {
                if let (Some(t), Some(size)) = (self.terms.get_mut(&id), Size::decode(&payload)) {
                    t.resize_to(size.cols, size.rows);
                    let _ = t.master.resize(PtySize {
                        rows: size.rows,
                        cols: size.cols,
                        pixel_width: size.width,
                        pixel_height: size.height,
                    });
                }
            }
            wire::SNAPSHOT => {
                if let Some(t) = self.terms.get_mut(&id) {
                    t.snapshot = Some(payload);
                }
            }
            wire::KILL => {
                if let Some(t) = self.terms.remove(&id) {
                    // Everyone streaming it is done with it.
                    for o in t.streams.iter().chain(&t.waiting) {
                        if let Some(c) = self.clients.get_mut(o) {
                            c.term = None;
                            c.closing = true;
                        }
                    }
                    hang_up(t);
                }
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.term = None;
                    c.closing = true;
                }
            }
            wire::DETACH => {
                // The app quits: from here on output goes to the backlog,
                // and DETACHED marks where its stream ends.
                if let Some(t) = self.terms.get_mut(&id) {
                    t.streams.retain(|s| *s != cid);
                }
                self.maybe_reclaim(&id);
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.term = None;
                    c.out.extend(wire::frame(wire::DETACHED, b""));
                    c.closing = true;
                }
                self.flush(cid);
            }
            _ => {}
        }
    }

    fn request(&mut self, req: &Value, cid: u64) {
        let stream = req["stream"] == true;
        let c = self.clients.get_mut(&cid).unwrap();
        let fail = |c: &mut Client, e: String| c.reply(json!({ "ok": false, "error": e }));
        match req["op"].as_str() {
            Some("hello") => {
                let reply = json!({
                    "ok": true,
                    "server": "abromux",
                    "protocol": wire::PROTOCOL,
                    "version": self.opts.version,
                    "host": hostname(),
                    "host_id": self.host_id,
                    "control": self.opts.control,
                    "app": self.app.is_some(),
                });
                c.reply(reply);
                self.flush(cid);
            }
            Some("app") => {
                self.app_owner = req["owner"].as_str().map(String::from);
                self.attach_app(cid)
            }
            // The terminals it keeps: who holds or streams each, and what
            // its program last said (title, folder).
            Some("terms") => {
                let list: Vec<Value> = self
                    .terms
                    .iter()
                    .map(|(id, t)| {
                        json!({
                            "id": id,
                            "pid": t.pid,
                            "surface": t.surface,
                            "title": t.title,
                            "cwd": t.cwd,
                            "held": t.holder.is_some(),
                            "streams": t.streams.len(),
                            "parsed": t.parsed_snapshot_available(),
                        })
                    })
                    .collect();
                let c = self.clients.get_mut(&cid).unwrap();
                c.reply(json!({ "ok": true, "terms": list }));
                self.flush(cid);
            }
            // What the app last said about its workspaces here: another app
            // (or this one, without its session) opens them from it.
            Some("get-state") => {
                c.reply(json!({
                    "ok": true,
                    "workspaces": self.state.workspaces,
                    "session": self.state.session,
                }));
                self.flush(cid);
            }
            Some("control") => match req["conn"].as_u64() {
                Some(conn) => self.open_control(cid, conn),
                None => {
                    // What followed the request is the control stream.
                    self.accept_control(cid);
                }
            },
            Some("spawn") if c.term.is_none() => {
                match spawn(req, stream, self.opts.control.as_deref()) {
                    Ok(t) => {
                        self.next_term += 1;
                        let id = format!("{}-{}", std::process::id(), self.next_term);
                        let reply = json!({ "ok": true, "id": id, "pid": t.pid });
                        let fd = t.master.as_raw_fd().unwrap_or(-1);
                        let mut t = t;
                        let sent = if stream {
                            set_nonblocking(fd, true);
                            c.reply(reply);
                            c.mode = Mode::Frames;
                            c.owner = req["owner"].as_str().map(String::from);
                            t.streams.push(cid);
                            t.start_parsing(None);
                            true
                        } else {
                            t.holder = Some(cid);
                            send_with_fd(c, format!("{reply}\n").as_bytes(), fd).is_ok()
                        };
                        if sent {
                            c.term = Some(id.clone());
                            self.terms.insert(id, t);
                        }
                        self.flush(cid);
                    }
                    Err(e) => {
                        fail(c, e);
                        self.flush(cid);
                    }
                }
            }
            Some("attach") if c.term.is_none() => {
                let id = req["id"].as_str().unwrap_or_default().to_string();
                let owner = req["owner"].as_str().map(String::from);
                // `take`: the same app coming back after its connection
                // broke, before the server noticed: its old one goes.
                if stream && req["take"] == true {
                    let old: Vec<u64> = self
                        .terms
                        .get(&id)
                        .map(|t| t.streams.clone())
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|o| *o != cid)
                        .filter(|o| {
                            let theirs = self.clients.get(o).and_then(|c| c.owner.clone());
                            owner.is_none() || theirs.is_none() || theirs == owner
                        })
                        .collect();
                    for o in old {
                        self.drop_client(o);
                    }
                }
                let Some(t) = self.terms.get_mut(&id) else {
                    let c = self.clients.get_mut(&cid).unwrap();
                    fail(c, format!("no terminal {id}"));
                    return self.flush(cid);
                };
                let held = t
                    .holder
                    .is_some_and(|h| h != cid && self.clients.contains_key(&h));
                let c = self.clients.get_mut(&cid).unwrap();
                if t.eof {
                    fail(c, format!("{id} has ended"));
                    return self.flush(cid);
                }
                if stream {
                    c.owner = owner;
                    c.term = Some(id.clone());
                    if held {
                        // A local app holds it: ask it to share (it lets go
                        // of the descriptor and streams it too), and answer
                        // once it has.
                        let Some(app) = self.app else {
                            c.term = None;
                            fail(c, format!("{id} is attached elsewhere"));
                            return self.flush(cid);
                        };
                        t.waiting.push(cid);
                        c.mode = Mode::Waiting;
                        if let Some(a) = self.clients.get_mut(&app) {
                            a.reply(json!({ "release": id }));
                        }
                        return self.flush(app);
                    }
                    let fresh = req["fresh"] == true;
                    self.stream_to(cid, &id, fresh);
                } else {
                    if held {
                        fail(c, format!("{id} is attached elsewhere"));
                        return self.flush(cid);
                    }
                    if !t.streams.is_empty() {
                        // Streamed to another app: this one streams it too.
                        c.reply(json!({ "ok": false, "error": format!("{id} is streamed"), "code": "streamed" }));
                        return self.flush(cid);
                    }
                    // What the server parsed since is the screen to come
                    // back to; the app reads it where it left its own.
                    if let Some(snap) = t.parsed_snapshot() {
                        let path = self.held_dir.join(format!("{id}.snap"));
                        let _ = std::fs::create_dir_all(&self.held_dir);
                        if std::fs::write(&path, snap).is_ok() {
                            t.backlog.clear();
                        }
                    }
                    let backlog = std::mem::take(&mut t.backlog);
                    let fd = t.master.as_raw_fd().unwrap_or(-1);
                    let reply = json!({ "ok": true, "pid": t.pid, "backlog": backlog.len() });
                    if send_with_fd(c, format!("{reply}\n").as_bytes(), fd).is_ok() {
                        c.out.extend_from_slice(&backlog);
                        c.term = Some(id);
                        t.holder = Some(cid);
                        t.stop_parsing();
                    } else {
                        t.backlog = backlog;
                    }
                }
                self.flush(cid);
            }
            Some("status") => {
                let code = c
                    .term
                    .clone()
                    .and_then(|id| self.terms.get_mut(&id))
                    .and_then(exit_code);
                let c = self.clients.get_mut(&cid).unwrap();
                c.reply(json!({ "ok": true, "code": code }));
                self.flush(cid);
            }
            Some("kill") => {
                if let Some(t) = c.term.take().and_then(|id| self.terms.remove(&id)) {
                    hang_up(t);
                }
                let c = self.clients.get_mut(&cid).unwrap();
                c.reply(json!({ "ok": true }));
                self.flush(cid);
            }
            _ => {
                fail(c, "bad request".into());
                self.flush(cid);
            }
        }
    }

    /// Write what's queued for a connection, as much as it takes now.
    fn flush(&mut self, cid: u64) {
        let Some(c) = self.clients.get_mut(&cid) else {
            return;
        };
        while !c.out.is_empty() {
            match c.stream.write(&c.out) {
                Ok(0) => return self.drop_client(cid),
                Ok(n) => {
                    c.out.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return self.drop_client(cid),
            }
        }
    }

    /// The connection is gone: its terminal is nobody's until the next
    /// attach, and the server reads it meanwhile.
    fn drop_client(&mut self, cid: u64) {
        let Some(c) = self.clients.remove(&cid) else {
            return;
        };
        if let Some(id) = c.term.clone()
            && let Some(t) = self.terms.get_mut(&id)
        {
            let was_streamed = t.streams.contains(&cid);
            t.streams.retain(|s| *s != cid);
            t.waiting.retain(|s| *s != cid);
            if was_streamed {
                self.maybe_reclaim(&id);
            }
            let Some(t) = self.terms.get_mut(&id) else {
                return;
            };
            if t.holder == Some(cid) {
                // The local app let go (quit, or to share it): parse from
                // the screen it left, and hand it to whoever was waiting.
                t.holder = None;
                let snap = std::fs::read(self.held_dir.join(format!("{id}.snap"))).ok();
                t.start_parsing(snap.as_deref());
                for w in std::mem::take(&mut t.waiting) {
                    self.stream_to(w, &id, true);
                }
            }
        }
        match c.mode {
            // One side of a splice gone: the other ends once it has the rest.
            Mode::Splice(o) => {
                if let Some(o) = self.clients.get_mut(&o) {
                    o.closing = true;
                }
                self.flush(o);
            }
            // The app went away: the server answers what was waiting for it.
            Mode::App if self.app == Some(cid) => {
                self.app = None;
                let pending: Vec<u64> = self
                    .clients
                    .iter()
                    .filter(|(_, c)| c.mode == Mode::Pending)
                    .map(|(id, _)| *id)
                    .collect();
                for p in pending {
                    self.accept_control(p);
                }
            }
            _ => {}
        }
    }

    fn read_term(&mut self, id: &str, buf: &mut [u8]) {
        let Some(t) = self.terms.get_mut(id) else {
            return;
        };
        let Some(fd) = t.master.as_raw_fd() else {
            return;
        };
        // SAFETY: reads into `buf`, which is valid for its length.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0
            && matches!(
                io::Error::last_os_error().kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            )
        {
            return; // left non-blocking; nothing after all
        }
        let streams: Vec<u64> = t
            .streams
            .iter()
            .copied()
            .filter(|c| self.clients.contains_key(c))
            .collect();
        if n <= 0 {
            t.eof = true;
            // Tell the streamed apps how it ended, then forget the terminal.
            if !streams.is_empty() {
                let code = exit_code(t);
                self.terms.remove(id);
                let exit = wire::frame(wire::EXIT, json!({ "code": code }).to_string().as_bytes());
                for cid in streams {
                    let c = self.clients.get_mut(&cid).unwrap();
                    c.out.extend_from_slice(&exit);
                    c.term = None;
                    c.closing = true;
                    self.flush(cid);
                }
            }
            return;
        }
        let bytes = &buf[..n as usize];
        let notes = t.parse(bytes);
        if streams.is_empty() {
            t.backlog.extend_from_slice(bytes);
            if t.backlog.len() > BACKLOG_MAX {
                let cut = t.backlog.len() - BACKLOG_MAX;
                t.backlog.drain(..cut);
            }
        }
        // Nobody watching: the server says it for the app (kept until it's
        // back, or handed over now if it's attached).
        let surface = t.surface.clone();
        if !t.input.is_empty() {
            self.write_input(id);
        }
        if streams.is_empty() && !notes.is_empty() {
            for (title, body) in notes {
                self.state.keep(
                    "notification.create",
                    json!({ "title": title, "body": body, "caller": { "surface": surface } }),
                );
            }
            self.offer_replay();
        }
        if !streams.is_empty() {
            let frame = wire::frame(wire::OUTPUT, bytes);
            for cid in streams {
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.out.extend_from_slice(&frame);
                }
                self.flush(cid);
            }
        }
    }

    /// Stream terminal `id` to `cid`: the reply, the screen so far, then
    /// frames. `fresh`: the app has no screen of its own (another app, or
    /// one that lost it), so the server's parsed one if it has it.
    fn stream_to(&mut self, cid: u64, id: &str, fresh: bool) {
        let Some(t) = self.terms.get_mut(id) else {
            return;
        };
        let Some(c) = self.clients.get_mut(&cid) else {
            return;
        };
        let parsed = if fresh { t.parsed_snapshot() } else { None };
        let (snapshot, backlog, is_parsed) = match parsed {
            Some(s) => {
                t.backlog.clear();
                (s, vec![], true)
            }
            None => (
                t.snapshot.take().unwrap_or_default(),
                std::mem::take(&mut t.backlog),
                false,
            ),
        };
        set_nonblocking(t.master.as_raw_fd().unwrap_or(-1), true);
        c.reply(json!({
            "ok": true,
            "pid": t.pid,
            "snapshot": snapshot.len(),
            "backlog": backlog.len(),
            "parsed": is_parsed,
        }));
        c.out.extend_from_slice(&snapshot);
        c.out.extend_from_slice(&backlog);
        c.mode = Mode::Frames;
        c.term = Some(id.to_string());
        if !t.streams.contains(&cid) {
            t.streams.push(cid);
        }
        t.start_parsing(None);
        // Bytes it sent after its request are its first frames.
        let rest = std::mem::take(&mut c.buf);
        c.decoder.push(&rest);
        self.flush(cid);
        self.handle_input(cid);
    }

    /// Only the local app streams terminal `id` now (the one sharing it
    /// left): it can hold it again, at no cost to the server.
    fn maybe_reclaim(&mut self, id: &str) {
        let (Some(app), Some(owner)) = (self.app, self.app_owner.clone()) else {
            return;
        };
        let Some(t) = self.terms.get(id) else { return };
        let only_ours = t.holder.is_none()
            && t.streams.len() == 1
            && self
                .clients
                .get(&t.streams[0])
                .is_some_and(|c| c.owner.as_deref() == Some(&owner));
        if only_ours && let Some(a) = self.clients.get_mut(&app) {
            a.reply(json!({ "reclaim": id }));
            self.flush(app);
        }
    }

    /// Tell the attached app there's something kept for it.
    fn offer_replay(&mut self) {
        if let Some(app) = self.app
            && let Some(a) = self.clients.get_mut(&app)
        {
            a.reply(json!({ "open": 0 }));
            self.flush(app);
        }
    }

    /// Give the PTY what a streamed app typed, as much as it takes now.
    fn write_input(&mut self, id: &str) {
        let Some(t) = self.terms.get_mut(id) else {
            return;
        };
        let Some(fd) = t.master.as_raw_fd() else {
            return;
        };
        while !t.input.is_empty() {
            // SAFETY: writes from `input`, valid for its length.
            let n = unsafe { libc::write(fd, t.input.as_ptr().cast(), t.input.len()) };
            if n > 0 {
                t.input.drain(..n as usize);
            } else if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            } else {
                return; // full (or gone): POLLOUT brings us back
            }
        }
    }
}

/// How a terminal's program ended, once its output has: its exit status,
/// None if a signal ended it or it isn't known yet. Waits a moment for it to
/// be reaped.
fn exit_code(t: &mut Term) -> Option<i32> {
    for _ in 0..30 {
        if let Ok(Some(s)) = t.child.try_wait() {
            return s.signal().is_none().then(|| s.exit_code() as i32);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn pollfd(fd: RawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}

fn set_nonblocking(fd: RawFd, on: bool) {
    if fd < 0 {
        return;
    }
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        let flags = if on {
            flags | libc::O_NONBLOCK
        } else {
            flags & !libc::O_NONBLOCK
        };
        libc::fcntl(fd, libc::F_SETFL, flags);
    }
}

/// An exclusive lock on `path`, held while the returned file is open;
/// None if another process holds it.
fn lock(path: &Path) -> io::Result<Option<std::fs::File>> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(Some(f))
    } else {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(e)
        }
    }
}

/// A random id for this machine, made the first time and kept at `path`,
/// so an app can tell when an address it knows leads somewhere else.
fn host_id(path: &Path) -> String {
    if let Ok(id) = std::fs::read_to_string(path)
        && !id.trim().is_empty()
    {
        return id.trim().to_string();
    }
    let mut bytes = [0u8; 12];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let _ = std::fs::write(path, &id);
    id
}

/// This machine's name, for `hello`.
fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `len` bytes into `buf`.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    if ok {
        String::from_utf8_lossy(&buf[..end]).into_owned()
    } else {
        String::new()
    }
}

/// End a terminal the way closing a terminal window does: SIGHUP to its
/// process group, SIGKILL if that's ignored. Reaped off the server's loop.
fn hang_up(t: Term) {
    let Term {
        master,
        mut child,
        pid,
        ..
    } = t;
    drop(master);
    let pid = pid as i32;
    if pid > 0 {
        // SAFETY: plain signals to the shell and its process group.
        unsafe {
            libc::kill(-pid, libc::SIGHUP);
            libc::kill(pid, libc::SIGHUP);
        }
    }
    std::thread::spawn(move || {
        for _ in 0..40 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    });
}

fn spawn(req: &Value, stream: bool, control: Option<&Path>) -> Result<Term, String> {
    let mut argv: Vec<String> = req["argv"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    // A streamed app is elsewhere: its shell's path means nothing here, so
    // an empty argv is this machine's user's login shell.
    if argv.is_empty() && stream {
        argv = vec![abromux_core::paths::user_shell(), "-l".into()];
        if let Some(c) = req["command"].as_str() {
            argv.extend(["-c".into(), c.into()]);
        }
    }
    let (prog, args) = argv.split_first().ok_or("no program")?;
    let size = PtySize {
        rows: req["rows"].as_u64().unwrap_or(24) as u16,
        cols: req["cols"].as_u64().unwrap_or(80) as u16,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pty = native_pty_system()
        .openpty(size)
        .map_err(|e| e.to_string())?;
    let mut cmd = CommandBuilder::new(prog);
    cmd.args(args);
    // A local app's environment, not the server's (which is whatever it
    // started with). A streamed app's environment is another machine's, so
    // its programs start from the server's with the app's variables on top.
    if !(stream && req["inherit_env"] != false) {
        cmd.env_clear();
    }
    // A streamed app's shells reach the app through this server's control
    // socket (agent hooks, `abromux notify`), and this machine's abromux is
    // this binary.
    if stream && let Some(c) = control {
        cmd.env("ABROMUX_SOCKET_PATH", c);
        if let Ok(exe) = std::env::current_exe() {
            cmd.env("ABROMUX_BIN", exe);
        }
    }
    for kv in req["env"].as_array().into_iter().flatten() {
        if let (Some(k), Some(v)) = (kv[0].as_str(), kv[1].as_str()) {
            cmd.env(k, v);
        }
    }
    let home = abromux_core::paths::home();
    let cwd = req["cwd"]
        .as_str()
        .map(abromux_core::paths::expand_tilde)
        .filter(|p| p.is_dir())
        .unwrap_or(home);
    cmd.cwd(cwd);
    let child = pty.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pty.slave);
    let surface = req["env"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|kv| kv[0] == "ABROMUX_SURFACE_ID")
        .and_then(|kv| kv[1].as_str())
        .map(String::from);
    Ok(Term {
        pid: child.process_id().unwrap_or(0),
        master: pty.master,
        child,
        backlog: Vec::new(),
        snapshot: None,
        holder: None,
        streams: Vec::new(),
        waiting: Vec::new(),
        eof: false,
        input: Vec::new(),
        surface,
        title: String::new(),
        cwd: String::new(),
        size: (size.cols, size.rows),
        #[cfg(feature = "vt")]
        vt: None,
    })
}

/// Send `data` with the descriptor `fd` attached, blocking until it's all
/// out (a reply line: small, and nothing else is queued before it).
fn send_with_fd(c: &mut Client, data: &[u8], fd: RawFd) -> io::Result<()> {
    let sock = &c.stream;
    // Anything queued goes first, so replies stay in order.
    sock.set_nonblocking(false)?;
    let res = (|| {
        (&*sock).write_all(&c.out)?;
        c.out.clear();
        let fd_len = std::mem::size_of::<RawFd>() as u32;
        // SAFETY: the message, its iovec and its control buffer all outlive
        // sendmsg, and the control buffer is sized by CMSG_SPACE.
        unsafe {
            let space = libc::CMSG_SPACE(fd_len) as usize;
            let mut control = vec![0u8; space];
            let mut iov = libc::iovec {
                iov_base: data.as_ptr() as *mut _,
                iov_len: data.len(),
            };
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = space as _;
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(fd_len) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd);
            let n = libc::sendmsg(sock.as_raw_fd(), &msg, 0);
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            (&*sock).write_all(&data[n as usize..])
        }
    })();
    sock.set_nonblocking(true)?;
    res
}

// ---- starting one, and reaching it through a pipe ------------------------

/// Connect to the server on `path`, starting `exe args` (detached) if none
/// answers there.
pub fn connect_or_start(path: &Path, exe: &Path, args: &[OsString]) -> io::Result<UnixStream> {
    if let Ok(s) = UnixStream::connect(path) {
        return Ok(s);
    }
    start_detached(exe, args)?;
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        match UnixStream::connect(path) {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() > end => {
                return Err(io::Error::new(e.kind(), format!("{}: {e}", path.display())));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// Run `exe args` in its own session, with no terminal and no output, so it
/// outlives whoever started it.
pub fn start_detached(exe: &Path, args: &[OsString]) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    // Reap it once it has served; it's its own session, not ours to wait on.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// `--stdio`: copy stdin to the server's socket and the socket to stdout,
/// until either side ends. How an app on another machine reaches this one's
/// server (`ssh HOST abromux-server --stdio`).
pub fn stdio(sock: UnixStream) -> io::Result<()> {
    let up = sock.try_clone()?;
    std::thread::spawn(move || {
        let _ = pump(0, up.as_raw_fd());
        let _ = up.shutdown(std::net::Shutdown::Write);
    });
    pump(sock.as_raw_fd(), 1)
}

/// Copy `from` to `to` until `from` ends, a read at a time. Not io::copy:
/// on Linux that splices when one side is a pipe (ssh's stdin and stdout),
/// and a splice from a pipe into a Unix socket can sit on bytes that have
/// arrived, which here means a request that never reaches the server. Raw
/// descriptors also skip io::stdout()'s line buffering, which would hold
/// frames back.
fn pump(from: RawFd, to: RawFd) -> io::Result<()> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        // SAFETY: reads into `buf`, valid for its length.
        let n = unsafe { libc::read(from, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if n == 0 {
            return Ok(());
        }
        let mut out = &buf[..n as usize];
        while !out.is_empty() {
            // SAFETY: writes from `out`, valid for its length.
            let w = unsafe { libc::write(to, out.as_ptr().cast(), out.len()) };
            if w < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            out = &out[w as usize..];
        }
    }
}

#[cfg(test)]
mod tests;
