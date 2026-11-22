//! A terminal on another machine's abromux server, streamed to this app
//! (docs/server-plan.md, phase 1).
//!
//! A remote PTY's descriptor can't come over the network, so the server
//! reads and writes it for us and the bytes travel in frames
//! ([`abromux_core::wire`]). The connection is `ssh HOST abromux-server
//! --stdio` (or a Unix socket, for tests and ssh `-L` forwards). The app
//! parses the output with libghostty-vt exactly as it does a local
//! terminal's.
//!
//! Everything that touches the network happens on the terminal's reader
//! thread, so a slow or unreachable host never holds up the window: the
//! view gets a [`Remote`] at once, and the thread connects, attaches (or
//! spawns), streams, and reconnects when the connection breaks. The program
//! runs on in the server meanwhile.

use crate::pty::Feed;
use abromux_core::wire::{self, Decoder, Size};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where a server is: `ssh://[user@]host[:port]` (or just `[user@]host`),
/// or `unix:///path/to/server.sock` (or an absolute path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Ssh { dest: String, port: Option<u16> },
    Unix(PathBuf),
}

impl Endpoint {
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Some(p) = s.strip_prefix("unix://") {
            return Ok(Endpoint::Unix(p.into()));
        }
        if s.starts_with('/') {
            return Ok(Endpoint::Unix(s.into()));
        }
        let rest = s.strip_prefix("ssh://").unwrap_or(s);
        let rest = rest.trim_end_matches('/');
        if rest.is_empty() || rest.contains(['/', ' ']) || rest.contains("://") {
            return Err(format!(
                "not a host: {s:?} (ssh://[user@]host[:port] or unix:///path)"
            ));
        }
        // A port after the host, not inside a user name or an IPv6 address.
        let (dest, port) = match rest.rsplit_once(':') {
            Some((d, p)) if !d.contains(':') && !p.is_empty() => {
                let port = p.parse().map_err(|_| format!("bad port in {s:?}"))?;
                (d.to_string(), Some(port))
            }
            _ => (rest.to_string(), None),
        };
        Ok(Endpoint::Ssh { dest, port })
    }

    /// For messages: the host, or the socket.
    pub fn label(&self) -> String {
        match self {
            Endpoint::Ssh { dest, .. } => dest.clone(),
            Endpoint::Unix(p) => p.display().to_string(),
        }
    }

    fn connect(&self) -> Result<Conn, String> {
        match self {
            Endpoint::Unix(path) => {
                let s =
                    UnixStream::connect(path).map_err(|e| format!("{}: {e}", path.display()))?;
                let read = OwnedFd::from(s.try_clone().map_err(|e| e.to_string())?);
                Ok(Conn {
                    read,
                    write: Pipe {
                        write: Box::new(s),
                        child: None,
                    },
                    stderr: Arc::default(),
                })
            }
            Endpoint::Ssh { dest, port } => ssh(dest, *port),
        }
    }
}

/// What runs on the host: the server's `--stdio` bridge, found in the
/// places `cargo install` and `~/.local/bin` installs put it even when the
/// login shell's PATH doesn't have them. `$ABROMUX_REMOTE_COMMAND` replaces it.
fn remote_command() -> String {
    std::env::var("ABROMUX_REMOTE_COMMAND")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| {
            "sh -c 'PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\" exec abromux-server --stdio'"
                .into()
        })
}

/// Where ssh keeps the shared connection to each host (`ControlMaster`):
/// every terminal on a host rides one ssh connection, authenticated once.
fn control_dir() -> PathBuf {
    // SAFETY: getuid can't fail.
    let uid = unsafe { libc::getuid() };
    // Short and fixed: socket paths are limited to ~104 bytes, and macOS's
    // $TMPDIR alone is about half of that.
    let dir = PathBuf::from(format!("/tmp/abromux-ssh-{uid}"));
    if std::fs::create_dir_all(&dir).is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir
}

fn ssh(dest: &str, port: Option<u16>) -> Result<Conn, String> {
    use std::process::{Command, Stdio};
    // `$ABROMUX_SSH`: another ssh to run (a wrapper, or one with its own
    // config).
    let program = std::env::var("ABROMUX_SSH")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "ssh".into());
    let mut cmd = Command::new(program);
    cmd.arg("-T")
        // No prompts: there's no terminal to answer them. Keys or an agent
        // (or Tailscale SSH) do the authenticating.
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=10"])
        .args([
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
        ])
        .args(["-o", "ControlMaster=auto", "-o", "ControlPersist=600"])
        .arg("-o")
        .arg(format!("ControlPath={}/%C", control_dir().display()));
    if let Some(p) = port {
        cmd.args(["-p", &p.to_string()]);
    }
    cmd.arg(dest)
        .arg(remote_command())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("can't run ssh: {e}"))?;
    let stdin = child.stdin.take().ok_or("ssh: no stdin")?;
    let stdout = child.stdout.take().ok_or("ssh: no stdout")?;
    // What ssh (or the remote shell) says goes into errors.
    let stderr: Arc<Mutex<Vec<u8>>> = Arc::default();
    if let Some(mut err) = child.stderr.take() {
        let keep = stderr.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = io::Read::read(&mut err, &mut buf) {
                if n == 0 {
                    break;
                }
                let mut k = keep.lock().unwrap();
                k.extend_from_slice(&buf[..n]);
                let len = k.len();
                if len > 4096 {
                    k.drain(..len - 4096);
                }
            }
        });
    }
    Ok(Conn {
        read: OwnedFd::from(stdout),
        write: Pipe {
            write: Box::new(stdin),
            child: Some(child),
        },
        stderr,
    })
}

/// One connection to a server: a byte pipe either way.
struct Conn {
    read: OwnedFd,
    write: Pipe,
    stderr: Arc<Mutex<Vec<u8>>>,
}

/// The connection's write side, which owns ssh's process: dropping it
/// closes ssh's stdin, which ends ssh, and reaps it.
struct Pipe {
    write: Box<dyn Write + Send>,
    child: Option<std::process::Child>,
}

impl Write for Pipe {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.write.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.write.flush()
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        self.write = Box::new(io::sink());
        if let Some(mut c) = self.child.take() {
            std::thread::spawn(move || {
                let end = Instant::now() + Duration::from_secs(5);
                while Instant::now() < end {
                    if !matches!(c.try_wait(), Ok(None)) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = c.kill();
                let _ = c.wait();
            });
        }
    }
}

impl Conn {
    /// Why it failed, with what ssh had to say about it.
    fn error(&mut self, e: String) -> String {
        // Let ssh finish saying it.
        if let Some(c) = &mut self.write.child {
            let end = Instant::now() + Duration::from_millis(500);
            while Instant::now() < end && matches!(c.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let said = String::from_utf8_lossy(&self.stderr.lock().unwrap())
            .trim()
            .to_string();
        if said.is_empty() {
            e
        } else {
            format!("{e}: {said}")
        }
    }
}

/// Read exactly `buf.len()` bytes. A signal (`stop`) interrupts it.
fn read_exact(fd: &OwnedFd, mut buf: &mut [u8]) -> Result<(), String> {
    while !buf.is_empty() {
        // SAFETY: reads into `buf`, valid for its length.
        let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n == 0 {
            return Err("the connection closed".into());
        }
        if n < 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        buf = &mut buf[n as usize..];
    }
    Ok(())
}

/// One reply line, read a byte at a time so nothing after it is taken.
fn read_line(fd: &OwnedFd) -> Result<Value, String> {
    let mut line = Vec::new();
    let mut b = [0u8];
    loop {
        read_exact(fd, &mut b)?;
        if b[0] == b'\n' {
            break;
        }
        line.push(b[0]);
        if line.len() > 1 << 20 {
            return Err("reply too long".into());
        }
    }
    serde_json::from_slice(&line).map_err(|e| format!("not an abromux server ({e})"))
}

/// Where a remote terminal's screen is saved when the app quits
/// (`hold::snapshot_path`): its id and host, as ids are only unique per
/// server.
pub fn snapshot_key(host: &str, id: &str) -> String {
    let host: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.@".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{id}@{host}")
}

/// Ask the server at `endpoint` about itself (`hello`): its name for the
/// machine, version and protocol. Blocks for as long as connecting takes
/// (ssh's 10 s timeout at most), so call it off the main thread.
pub fn hello(endpoint: &Endpoint) -> Result<Value, String> {
    let mut conn = endpoint.connect()?;
    let sent = writeln!(conn.write, "{}", json!({ "op": "hello" }))
        .and_then(|_| conn.write.flush())
        .map_err(|e| e.to_string());
    match sent.and_then(|_| read_line(&conn.read)) {
        Ok(v) if v["ok"] == true => Ok(v),
        Ok(_) => Err("not an abromux server (an old holder?)".into()),
        Err(e) => Err(conn.error(e)),
    }
}

/// What the reader thread and the view share.
struct Shared {
    /// As the session saves it.
    host: String,
    endpoint: Endpoint,
    /// The server's id for the terminal, once it has one.
    id: Mutex<Option<String>>,
    /// Where frames go while connected.
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    /// Typed while not connected; sent once connected.
    pending: Mutex<Vec<u8>>,
    size: Mutex<Size>,
    /// How the program ended, once it has: Some(code), code None for a signal.
    exit: Mutex<Option<Option<i32>>>,
    stopped: AtomicBool,
}

/// A terminal on another machine's server.
pub struct Remote {
    shared: Arc<Shared>,
    reader: Option<std::thread::JoinHandle<()>>,
}

/// How to start the program if there's no terminal to take back: `argv`
/// empty for the host user's login shell, `command` to run instead.
pub struct SpawnSpec {
    pub command: Option<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
}

impl Remote {
    /// A terminal on `endpoint`: `attach`'s if it's still there, else a new
    /// one from `spawn`. Returns at once; output arrives in `feed`.
    pub fn start(
        host: String,
        endpoint: Endpoint,
        attach: Option<String>,
        spawn: SpawnSpec,
        size: Size,
        feed: &Feed,
    ) -> Self {
        crate::pty::install_wake();
        let shared = Arc::new(Shared {
            host,
            endpoint,
            id: Mutex::new(attach),
            writer: Mutex::default(),
            pending: Mutex::default(),
            size: Mutex::new(size),
            exit: Mutex::default(),
            stopped: AtomicBool::new(false),
        });
        let s = shared.clone();
        let feed = feed.clone();
        let reader = std::thread::spawn(move || run(s, spawn, feed));
        Remote {
            shared,
            reader: Some(reader),
        }
    }

    /// The server's id for the terminal (None until it has started).
    pub fn id(&self) -> Option<String> {
        self.shared.id.lock().unwrap().clone()
    }

    /// The host as given to [`start`](Self::start).
    pub fn host(&self) -> &str {
        &self.shared.host
    }

    fn send(&self, kind: u8, payload: &[u8]) -> bool {
        let mut w = self.shared.writer.lock().unwrap();
        let ok = w.as_mut().is_some_and(|w| {
            w.write_all(&wire::frame(kind, payload))
                .and_then(|_| w.flush())
                .is_ok()
        });
        if !ok {
            *w = None;
        }
        ok
    }

    pub fn write(&self, bytes: &[u8]) {
        if !self.send(wire::INPUT, bytes) {
            let mut p = self.shared.pending.lock().unwrap();
            if p.len() + bytes.len() <= 64 << 10 {
                p.extend_from_slice(bytes);
            }
        }
    }

    pub fn resize(&self, size: Size) {
        *self.shared.size.lock().unwrap() = size;
        self.send(wire::RESIZE, &size.encode());
    }

    pub fn exit_status(&self) -> Option<i32> {
        self.shared.exit.lock().unwrap().flatten()
    }

    /// Quitting: the server sends what's left before `DETACHED` and keeps
    /// the rest in the backlog, so what the view has processed when this
    /// returns is exactly what precedes the backlog. Returns the id.
    pub fn freeze(&mut self) -> Option<String> {
        if self.send(wire::DETACH, b"")
            && let Some(r) = &self.reader
        {
            let end = Instant::now() + Duration::from_secs(1);
            while !r.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        self.stop();
        self.id()
    }

    /// End the program for good (closing its tab).
    pub fn kill(&mut self) {
        self.send(wire::KILL, b"");
        self.stop();
    }

    fn stop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        *self.shared.writer.lock().unwrap() = None;
        if let Some(r) = self.reader.take() {
            crate::pty::interrupt(r);
        }
    }
}

impl Drop for Remote {
    /// Dropping only lets go, as quitting does: the server keeps it.
    fn drop(&mut self) {
        self.stop();
    }
}

/// Dim, on its own line: what the connection is doing.
fn note(feed: &mut Feed, text: &str) -> bool {
    feed.push(format!("\r\n\x1b[2m[abromux: {text}]\x1b[0m\r\n").as_bytes())
}

/// The reader thread: connect, stream, and connect again when the
/// connection breaks, until the program ends or the view lets go.
fn run(s: Arc<Shared>, spawn: SpawnSpec, mut feed: Feed) {
    let stopped = || s.stopped.load(Ordering::Acquire);
    let host = s.endpoint.label();
    let mut delay = Duration::from_millis(500);
    // Said once per outage, so a flaky link doesn't fill the screen.
    let mut told: Option<String> = None;
    loop {
        if stopped() {
            return;
        }
        match connect(&s, &spawn) {
            Ok((read, backlog)) => {
                delay = Duration::from_millis(500);
                if told.take().is_some() && !note(&mut feed, &format!("connected to {host}")) {
                    return;
                }
                if !backlog.is_empty() && !feed.push(&backlog) {
                    return;
                }
                match stream(&s, read, &mut feed) {
                    Ended::Exited | Ended::Detached | Ended::ViewGone => return,
                    Ended::Lost => {}
                }
            }
            Err(Failed::Gone(e)) => {
                note(&mut feed, &e);
                feed.close();
                return;
            }
            Err(Failed::Retry(e)) => {
                if stopped() {
                    return;
                }
                if told.as_deref() != Some(&e) {
                    if !note(&mut feed, &format!("{e}; retrying")) {
                        return;
                    }
                    told = Some(e);
                }
            }
        }
        *s.writer.lock().unwrap() = None;
        // Back off, a little at a time so a stop isn't kept waiting.
        let until = Instant::now() + delay;
        while Instant::now() < until {
            if stopped() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        delay = (delay * 2).min(Duration::from_secs(15));
    }
}

enum Failed {
    /// Try again later: the host or the connection.
    Retry(String),
    /// No point: the server refused to start it.
    Gone(String),
}

/// Attach to the terminal (or spawn it), and get the connection streaming:
/// its read side, and the backlog to show first.
fn connect(s: &Shared, spawn: &SpawnSpec) -> Result<(OwnedFd, Vec<u8>), Failed> {
    let mut conn = s.endpoint.connect().map_err(Failed::Retry)?;
    let size = *s.size.lock().unwrap();
    let id = s.id.lock().unwrap().clone();
    let mut backlog = Vec::new();
    if let Some(id) = &id {
        let req = json!({ "op": "attach", "id": id, "stream": true, "take": true });
        let reply = request(&mut conn, &req).map_err(Failed::Retry)?;
        if reply["ok"] == true {
            // The server's snapshot is for apps without one of their own;
            // this app restored its own before connecting.
            let mut skip = vec![0u8; reply["snapshot"].as_u64().unwrap_or(0) as usize];
            read_exact(&conn.read, &mut skip).map_err(Failed::Retry)?;
            backlog.resize(reply["backlog"].as_u64().unwrap_or(0) as usize, 0);
            read_exact(&conn.read, &mut backlog).map_err(Failed::Retry)?;
        } else {
            let e = reply["error"].as_str().unwrap_or("refused").to_string();
            if !(e.starts_with("no terminal") || e.ends_with("has ended")) {
                return Err(Failed::Retry(e));
            }
            // It's gone (the host restarted, or it ended): a new one, on a
            // clean screen.
            backlog.extend_from_slice(b"\x1b[0m\x1b[2J\x1b[3J\x1b[H");
            *s.id.lock().unwrap() = None;
            drop(conn);
            return connect(s, spawn).map(|(fd, mut b)| {
                backlog.append(&mut b);
                (fd, backlog)
            });
        }
    } else {
        let env: Vec<[&str; 2]> = spawn
            .env
            .iter()
            .map(|(k, v)| [k.as_str(), v.as_str()])
            .collect();
        let req = json!({
            "op": "spawn", "stream": true, "argv": [], "command": spawn.command,
            "cwd": spawn.cwd, "env": env, "cols": size.cols, "rows": size.rows,
        });
        let reply = request(&mut conn, &req).map_err(Failed::Retry)?;
        if reply["ok"] != true {
            let e = reply["error"].as_str().unwrap_or("refused");
            return Err(Failed::Gone(format!(
                "{} couldn't start it: {e}",
                s.endpoint.label()
            )));
        }
        *s.id.lock().unwrap() = reply["id"].as_str().map(String::from);
    }
    // Streaming: the current size (the view's, which may have changed
    // since the request, while there was no writer to send it) and what was
    // typed meanwhile. Under the writer's lock, so a resize from now on is
    // either read here or sent by `resize` itself.
    let Conn {
        read, mut write, ..
    } = conn;
    let mut w = s.writer.lock().unwrap();
    let size = *s.size.lock().unwrap();
    let mut out = wire::frame(wire::RESIZE, &size.encode());
    let pending = std::mem::take(&mut *s.pending.lock().unwrap());
    if !pending.is_empty() {
        out.extend(wire::frame(wire::INPUT, &pending));
    }
    write
        .write_all(&out)
        .map_err(|e| Failed::Retry(e.to_string()))?;
    let _ = write.flush();
    *w = Some(Box::new(write));
    drop(w);
    Ok((read, backlog))
}

/// `hello`, then `req`: the reply to `req`.
fn request(conn: &mut Conn, req: &Value) -> Result<Value, String> {
    let hello = json!({ "op": "hello" });
    let sent = writeln!(conn.write, "{hello}")
        .and_then(|_| conn.write.flush())
        .map_err(|e| e.to_string());
    let reply = sent.and_then(|_| read_line(&conn.read));
    let reply = match reply {
        Ok(r) => r,
        Err(e) => return Err(conn.error(e)),
    };
    // Revision 1 is a holder, which can't stream.
    if reply["protocol"].as_u64().unwrap_or(1) < wire::PROTOCOL {
        return Err("the server there is too old to stream terminals; update it".into());
    }
    writeln!(conn.write, "{req}")
        .and_then(|_| conn.write.flush())
        .map_err(|e| e.to_string())?;
    read_line(&conn.read).map_err(|e| conn.error(e))
}

enum Ended {
    /// The program ended.
    Exited,
    /// The app quit (`DETACHED`).
    Detached,
    /// The view is gone.
    ViewGone,
    /// The connection broke.
    Lost,
}

/// Frames into the view until something ends it.
fn stream(s: &Shared, read: OwnedFd, feed: &mut Feed) -> Ended {
    let mut dec = Decoder::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        // SAFETY: reads into `buf`, valid for its length.
        let n = unsafe { libc::read(read.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            if s.stopped.load(Ordering::Acquire) {
                return Ended::ViewGone;
            }
            continue;
        }
        if n <= 0 {
            return Ended::Lost;
        }
        dec.push(&buf[..n as usize]);
        loop {
            match dec.next_frame() {
                Ok(Some((wire::OUTPUT, p))) => {
                    if !feed.push(&p) {
                        return Ended::ViewGone;
                    }
                }
                Ok(Some((wire::EXIT, p))) => {
                    let v: Value = serde_json::from_slice(&p).unwrap_or_default();
                    *s.exit.lock().unwrap() = Some(v["code"].as_i64().map(|c| c as i32));
                    feed.close();
                    return Ended::Exited;
                }
                Ok(Some((wire::DETACHED, _))) => return Ended::Detached,
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => return Ended::Lost,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        let ssh = |d: &str, p| {
            Ok(Endpoint::Ssh {
                dest: d.into(),
                port: p,
            })
        };
        assert_eq!(Endpoint::parse("ssh://box"), ssh("box", None));
        assert_eq!(
            Endpoint::parse("ssh://me@box:2222"),
            ssh("me@box", Some(2222))
        );
        assert_eq!(
            Endpoint::parse("me@box.tail1234.ts.net"),
            ssh("me@box.tail1234.ts.net", None)
        );
        assert_eq!(Endpoint::parse("box:22/"), ssh("box", Some(22)));
        assert_eq!(
            Endpoint::parse("unix:///tmp/s.sock"),
            Ok(Endpoint::Unix("/tmp/s.sock".into()))
        );
        assert_eq!(
            Endpoint::parse("/tmp/s.sock"),
            Ok(Endpoint::Unix("/tmp/s.sock".into()))
        );
        assert!(Endpoint::parse("").is_err());
        assert!(Endpoint::parse("http://box").is_err());
        assert!(Endpoint::parse("box:notaport").is_err());
    }

    /// Bytes a feed got, until `want` shows up.
    fn wait_for(feed: &Feed, want: &str) -> String {
        let mut seen = Vec::new();
        let end = Instant::now() + Duration::from_secs(5);
        while !String::from_utf8_lossy(&seen).contains(want) {
            assert!(
                Instant::now() < end,
                "never saw {want:?}: {}",
                String::from_utf8_lossy(&seen)
            );
            seen.extend(feed.take().bytes);
            std::thread::sleep(Duration::from_millis(10));
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    fn server(name: &str) -> PathBuf {
        let sock =
            std::env::temp_dir().join(format!("abx-remote-{name}-{}.sock", std::process::id()));
        let s = sock.clone();
        let opts = abromux_server::Options {
            version: "test".into(),
        };
        std::thread::spawn(move || abromux_server::serve(&s, &opts));
        while UnixStream::connect(&sock).is_err() {
            std::thread::sleep(Duration::from_millis(10));
        }
        sock
    }

    fn spec() -> SpawnSpec {
        SpawnSpec {
            command: None,
            cwd: "/tmp".into(),
            env: vec![("PS1".into(), "$ ".into())],
        }
    }

    fn size() -> Size {
        Size {
            cols: 100,
            rows: 30,
            width: 0,
            height: 0,
        }
    }

    /// Start, type, quit (freeze) and come back with what it printed since;
    /// then a terminal that's gone is replaced by a new one.
    #[test]
    fn a_remote_terminal_runs_on_while_the_app_is_away() {
        let ep = Endpoint::Unix(server("away"));
        let (feed, _wake) = Feed::new();
        let mut r = Remote::start(String::new(), ep.clone(), None, spec(), size(), &feed);
        r.write(b"stty size; echo hi-$((40+2))\n");
        wait_for(&feed, "hi-42");
        r.write(b"sleep 0.3; echo later\n");
        let id = r.freeze().expect("an id");
        drop(r);
        std::thread::sleep(Duration::from_millis(700));

        let (feed, _wake) = Feed::new();
        let mut r = Remote::start(
            String::new(),
            ep.clone(),
            Some(id.clone()),
            spec(),
            size(),
            &feed,
        );
        let seen = wait_for(&feed, "later");
        assert!(
            !seen.contains("hi-42"),
            "only what came after the quit: {seen:?}"
        );
        assert_eq!(r.id(), Some(id.clone()));
        r.kill();
        drop(r);
        std::thread::sleep(Duration::from_millis(100));

        // Gone: a new terminal in its place, on a clean screen.
        let (feed, _wake) = Feed::new();
        let r = Remote::start(String::new(), ep, Some(id.clone()), spec(), size(), &feed);
        r.write(b"echo fresh\n");
        wait_for(&feed, "fresh");
        assert_ne!(r.id(), Some(id));
    }

    #[test]
    fn exit_status_comes_back() {
        let ep = Endpoint::Unix(server("exit"));
        let (feed, _wake) = Feed::new();
        let r = Remote::start(String::new(), ep, None, spec(), size(), &feed);
        r.write(b"exit 7\n");
        let end = Instant::now() + Duration::from_secs(5);
        while !feed.take().closed {
            assert!(Instant::now() < end, "never closed");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(r.exit_status(), Some(7));
    }

    /// No server there yet: it says so, keeps trying, and connects once
    /// there is one.
    #[test]
    fn it_waits_for_the_host() {
        let sock =
            std::env::temp_dir().join(format!("abx-remote-late-{}.sock", std::process::id()));
        let (feed, _wake) = Feed::new();
        let r = Remote::start(
            String::new(),
            Endpoint::Unix(sock.clone()),
            None,
            spec(),
            size(),
            &feed,
        );
        r.write(b"echo typed-early\n");
        wait_for(&feed, "retrying");
        let opts = abromux_server::Options {
            version: "test".into(),
        };
        let s = sock.clone();
        std::thread::spawn(move || abromux_server::serve(&s, &opts));
        wait_for(&feed, "typed-early");
    }

    /// Against a real host over ssh: `ABROMUX_E2E_HOST=ssh://box cargo test
    /// -p abromux-term e2e -- --ignored`. The host needs `abromux-server` on
    /// its PATH (or in ~/.local/bin) and key-based ssh.
    #[test]
    #[ignore]
    fn e2e_over_ssh() {
        let host = std::env::var("ABROMUX_E2E_HOST").expect("ABROMUX_E2E_HOST");
        let ep = Endpoint::parse(&host).unwrap();
        let (feed, _wake) = Feed::new();
        let mut r = Remote::start(host.clone(), ep.clone(), None, spec(), size(), &feed);
        r.write(b"echo \"$(uname -s) $(stty size)\"\n");
        wait_for(&feed, "Linux 30 100");
        r.resize(Size {
            cols: 132,
            rows: 43,
            width: 0,
            height: 0,
        });
        r.write(b"stty size; sleep 1; echo while-away\n");
        wait_for(&feed, "43 132");
        let id = r.freeze().unwrap();
        drop(r);
        std::thread::sleep(Duration::from_secs(2));

        let (feed, _wake) = Feed::new();
        let mut r = Remote::start(host, ep, Some(id.clone()), spec(), size(), &feed);
        wait_for(&feed, "while-away");
        assert_eq!(r.id(), Some(id));
        r.kill();
    }
}
