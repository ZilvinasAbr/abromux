//! abromux-hold: abromux's own way to keep shells running across a quit,
//! crash or update (`"persistence": "abromux-hold"`).
//!
//! `abromux hold` is a small background process, one per app, started on
//! demand and detached from it (its own session, so quitting abromux doesn't
//! hang it up). It opens the PTYs and is the shells' parent, so they live as
//! long as it does. It parses nothing.
//!
//! While abromux is attached to a terminal, the holder hands it the PTY
//! itself (the master's file descriptor, over the Unix socket with
//! `SCM_RIGHTS`) and doesn't touch it: abromux reads and writes it exactly
//! as it does a PTY of its own, so a held terminal costs nothing extra while
//! abromux runs. Each terminal has its own connection to the holder; when it
//! closes (quit, crash), the holder starts reading that PTY again into a
//! backlog, so the program never blocks on a full PTY.
//!
//! On quit, abromux stops reading, writes each terminal's state with
//! libghostty-vt's snapshot (`snapshot_path`), then lets go. The next launch
//! attaches: the snapshot, then the backlog, give back the screen exactly as
//! the program left it. After a crash there's no snapshot, only the backlog.
//!
//! Protocol: one JSON request per line; each reply is a JSON line, carrying
//! the PTY's descriptor for `spawn` and `attach`, and for `attach` followed
//! by `backlog` raw bytes.
//!
//! - `{"op":"spawn","argv":[..],"cwd":"..","env":[[k,v],..],"cols":c,"rows":r}`
//!   → `{"ok":true,"id":"..","pid":n}`
//! - `{"op":"attach","id":".."}` → `{"ok":true,"pid":n,"backlog":len}`
//! - `{"op":"kill"}` (the connection's terminal) → `{"ok":true}`
//!
//! The socket is `hold.sock` in the app's support folder (so the release and
//! nightly apps each have their own holder), or `$ABROMUX_HOLD_SOCKET`.

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Output kept per terminal while nobody is attached; past it, the oldest
/// goes (and the screen may come back slightly wrong).
const BACKLOG_MAX: usize = 16 << 20;
/// With no terminals and no connections for this long, the holder exits.
const IDLE_EXIT: Duration = Duration::from_secs(10);

pub fn socket_path() -> PathBuf {
    std::env::var_os("ABROMUX_HOLD_SOCKET")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::channel::support_dir().join("hold.sock"))
}

/// Where a terminal's state waits for the next launch.
pub fn snapshot_path(id: &str) -> PathBuf {
    let dir = socket_path().with_file_name("held");
    dir.join(format!("{id}.snap"))
}

// ---- the holder ----------------------------------------------------------

struct Term {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    pid: u32,
    backlog: Vec<u8>,
    /// The connection it's handed to; while set, the holder leaves its PTY alone.
    client: Option<u64>,
    /// Its PTY reported end of file: the program is gone.
    eof: bool,
}

struct Client {
    stream: UnixStream,
    buf: Vec<u8>,
    term: Option<String>,
}

/// `abromux hold`: serve until idle. Exits at once if a holder already
/// listens on `path`.
pub fn serve(path: &Path) -> io::Result<()> {
    if UnixStream::connect(path).is_ok() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;

    let mut terms: HashMap<String, Term> = HashMap::new();
    let mut clients: HashMap<u64, Client> = HashMap::new();
    let (mut next_client, mut next_term) = (0u64, 0u64);
    let mut idle_since = Instant::now();
    let mut buf = vec![0u8; 1 << 16];

    loop {
        // Programs that ended while nobody was attached are dropped.
        terms.retain(|_, t| {
            let exited = t.eof || matches!(t.child.try_wait(), Ok(Some(_)));
            !(exited && t.client.is_none())
        });
        if terms.is_empty() && clients.is_empty() {
            if idle_since.elapsed() > IDLE_EXIT {
                let _ = std::fs::remove_file(path);
                return Ok(());
            }
        } else {
            idle_since = Instant::now();
        }

        // What to wait on: new connections, requests, and the PTYs nobody holds.
        let mut fds = vec![pollfd(listener.as_raw_fd())];
        let client_ids: Vec<u64> = clients.keys().copied().collect();
        fds.extend(client_ids.iter().map(|c| pollfd(clients[c].stream.as_raw_fd())));
        let term_ids: Vec<String> = terms
            .iter()
            .filter(|(_, t)| t.client.is_none() && !t.eof)
            .map(|(id, _)| id.clone())
            .collect();
        fds.extend(term_ids.iter().filter_map(|id| Some(pollfd(terms[id].master.as_raw_fd()?))));
        // SAFETY: `fds` is a valid array of pollfd for its length.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 1000) };
        if n <= 0 {
            continue;
        }
        let ready = |i: usize| fds[i].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0;

        if ready(0) {
            while let Ok((stream, _)) = listener.accept() {
                let _ = stream.set_nonblocking(false);
                next_client += 1;
                clients.insert(
                    next_client,
                    Client {
                        stream,
                        buf: Vec::new(),
                        term: None,
                    },
                );
            }
        }

        for (i, cid) in client_ids.iter().enumerate() {
            if !ready(1 + i) {
                continue;
            }
            let Some(c) = clients.get_mut(cid) else { continue };
            let gone = match c.stream.read(&mut buf) {
                Ok(0) | Err(_) => true,
                Ok(n) => {
                    c.buf.extend_from_slice(&buf[..n]);
                    false
                }
            };
            while let Some(nl) = clients[cid].buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = clients.get_mut(cid).unwrap().buf.drain(..=nl).collect();
                let req: Value = serde_json::from_slice(&line).unwrap_or(Value::Null);
                handle(&req, *cid, &mut clients, &mut terms, &mut next_term);
            }
            if gone {
                // Let go: the holder reads its PTY again.
                if let Some(c) = clients.remove(cid)
                    && let Some(t) = c.term.and_then(|id| terms.get_mut(&id))
                    && t.client == Some(*cid)
                {
                    t.client = None;
                }
            }
        }

        let base = 1 + client_ids.len();
        for (i, id) in term_ids.iter().enumerate() {
            if !ready(base + i) {
                continue;
            }
            let Some(t) = terms.get_mut(id) else { continue };
            let Some(fd) = t.master.as_raw_fd() else { continue };
            // SAFETY: reads into `buf`, which is valid for its length.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                t.eof = true;
                continue;
            }
            t.backlog.extend_from_slice(&buf[..n as usize]);
            if t.backlog.len() > BACKLOG_MAX {
                let cut = t.backlog.len() - BACKLOG_MAX;
                t.backlog.drain(..cut);
            }
        }
    }
}

fn pollfd(fd: RawFd) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }
}

fn handle(
    req: &Value,
    cid: u64,
    clients: &mut HashMap<u64, Client>,
    terms: &mut HashMap<String, Term>,
    next_term: &mut u64,
) {
    let c = clients.get_mut(&cid).unwrap();
    let fail = |c: &mut Client, e: String| {
        let _ = writeln!(c.stream, "{}", json!({ "ok": false, "error": e }));
    };
    match req["op"].as_str() {
        Some("spawn") if c.term.is_none() => match spawn(req) {
            Ok(t) => {
                *next_term += 1;
                let id = format!("{}-{}", std::process::id(), next_term);
                let reply = json!({ "ok": true, "id": id, "pid": t.pid });
                let fd = t.master.as_raw_fd().unwrap_or(-1);
                if send_with_fd(&c.stream, format!("{reply}\n").as_bytes(), fd).is_ok() {
                    c.term = Some(id.clone());
                    terms.insert(id, Term { client: Some(cid), ..t });
                }
            }
            Err(e) => fail(c, e),
        },
        Some("attach") if c.term.is_none() => {
            let id = req["id"].as_str().unwrap_or_default().to_string();
            let Some(t) = terms.get_mut(&id) else {
                return fail(c, format!("no terminal {id}"));
            };
            let busy = t.client.is_some_and(|other| other != cid && clients.contains_key(&other));
            let c = clients.get_mut(&cid).unwrap();
            if busy {
                return fail(c, format!("{id} is attached elsewhere"));
            }
            if t.eof {
                return fail(c, format!("{id} has ended"));
            }
            let backlog = std::mem::take(&mut t.backlog);
            let reply = json!({ "ok": true, "pid": t.pid, "backlog": backlog.len() });
            let fd = t.master.as_raw_fd().unwrap_or(-1);
            if send_with_fd(&c.stream, format!("{reply}\n").as_bytes(), fd).is_ok()
                && c.stream.write_all(&backlog).is_ok()
            {
                c.term = Some(id);
                t.client = Some(cid);
            } else {
                t.backlog = backlog;
            }
        }
        Some("kill") => {
            if let Some(t) = c.term.take().and_then(|id| terms.remove(&id)) {
                hang_up(t);
            }
            let _ = writeln!(c.stream, "{}", json!({ "ok": true }));
        }
        _ => fail(c, "bad request".into()),
    }
}

/// End a terminal the way closing a terminal window does: SIGHUP to its
/// process group, SIGKILL if that's ignored. Reaped off the holder's loop.
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

fn spawn(req: &Value) -> Result<Term, String> {
    let argv: Vec<&str> = req["argv"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
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
    // The app's environment, not the holder's (which is whatever it started with).
    cmd.env_clear();
    for kv in req["env"].as_array().into_iter().flatten() {
        if let (Some(k), Some(v)) = (kv[0].as_str(), kv[1].as_str()) {
            cmd.env(k, v);
        }
    }
    if let Some(cwd) = req["cwd"].as_str() {
        cmd.cwd(cwd);
    }
    let child = pty.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pty.slave);
    Ok(Term {
        pid: child.process_id().unwrap_or(0),
        master: pty.master,
        child,
        backlog: Vec::new(),
        client: None,
        eof: false,
    })
}

// ---- abromux's side ------------------------------------------------------

/// A terminal in the holder, handed to us.
pub struct Held {
    pub id: String,
    pub pid: u32,
    /// The PTY's master.
    pub fd: OwnedFd,
    /// What it printed while nobody was attached.
    pub backlog: Vec<u8>,
    /// Closing it gives the PTY back to the holder.
    conn: UnixStream,
}

impl Held {
    /// Start `argv` in `cwd` with exactly `env`.
    pub fn spawn(
        argv: &[String],
        cwd: &str,
        env: &[(String, String)],
        (cols, rows): (u16, u16),
    ) -> Result<Self, String> {
        let env: Vec<[&str; 2]> = env.iter().map(|(k, v)| [k.as_str(), v.as_str()]).collect();
        let req = json!({ "op": "spawn", "argv": argv, "cwd": cwd, "env": env, "cols": cols, "rows": rows });
        let (reply, fd, conn, _) = request(&req)?;
        Ok(Self {
            id: reply["id"].as_str().ok_or("no id")?.to_string(),
            pid: reply["pid"].as_u64().unwrap_or(0) as u32,
            fd,
            backlog: Vec::new(),
            conn,
        })
    }

    /// Take a terminal back from the holder, with what it printed meanwhile.
    pub fn attach(id: &str) -> Result<Self, String> {
        let (reply, fd, conn, backlog) = request(&json!({ "op": "attach", "id": id }))?;
        Ok(Self {
            id: id.to_string(),
            pid: reply["pid"].as_u64().unwrap_or(0) as u32,
            fd,
            backlog,
            conn,
        })
    }

    /// End the program and forget the terminal (closing its tab).
    pub fn kill(&mut self) {
        let _ = writeln!(self.conn, "{}", json!({ "op": "kill" }));
        let mut ok = [0u8; 64];
        let _ = self.conn.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = self.conn.read(&mut ok);
        let _ = std::fs::remove_file(snapshot_path(&self.id));
    }
}

/// Connect (starting the holder if needed), send `req`, and read the reply
/// with its descriptor and any backlog.
fn request(req: &Value) -> Result<(Value, OwnedFd, UnixStream, Vec<u8>), String> {
    let mut conn = connect()?;
    writeln!(conn, "{req}").map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    let mut fd = None;
    let mut buf = vec![0u8; 1 << 16];
    let nl = loop {
        let (n, got) = recv_with_fd(&conn, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("the holder hung up".into());
        }
        fd = fd.or(got);
        data.extend_from_slice(&buf[..n]);
        if let Some(nl) = data.iter().position(|&b| b == b'\n') {
            break nl;
        }
    };
    let reply: Value = serde_json::from_slice(&data[..nl]).map_err(|e| e.to_string())?;
    if reply["ok"] != true {
        return Err(reply["error"].as_str().unwrap_or("refused").to_string());
    }
    let fd = fd.ok_or("the holder sent no PTY")?;
    let mut backlog = data[nl + 1..].to_vec();
    let want = reply["backlog"].as_u64().unwrap_or(0) as usize;
    if backlog.len() < want {
        let start = backlog.len();
        backlog.resize(want, 0);
        conn.read_exact(&mut backlog[start..])
            .map_err(|e| e.to_string())?;
    }
    Ok((reply, fd, conn, backlog))
}

fn connect() -> Result<UnixStream, String> {
    let path = socket_path();
    if let Ok(s) = UnixStream::connect(&path) {
        return Ok(s);
    }
    start_holder().map_err(|e| format!("can't start abromux hold: {e}"))?;
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        match UnixStream::connect(&path) {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() > end => return Err(format!("{}: {e}", path.display())),
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// `abromux hold`, detached: its own session, no terminal, no output.
fn start_holder() -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("hold")
        .env("ABROMUX_HOLD_SOCKET", socket_path())
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

// ---- descriptors over the socket -----------------------------------------

fn send_with_fd(sock: &UnixStream, data: &[u8], fd: RawFd) -> io::Result<()> {
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
}

fn recv_with_fd(sock: &UnixStream, buf: &mut [u8]) -> io::Result<(usize, Option<OwnedFd>)> {
    let fd_len = std::mem::size_of::<RawFd>() as u32;
    // SAFETY: as in send_with_fd; a descriptor that arrives is ours to own.
    unsafe {
        let space = libc::CMSG_SPACE(fd_len) as usize;
        let mut control = vec![0u8; space];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        let n = libc::recvmsg(sock.as_raw_fd(), &mut msg, 0);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut fd = None;
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if !cmsg.is_null()
            && (*cmsg).cmsg_level == libc::SOL_SOCKET
            && (*cmsg).cmsg_type == libc::SCM_RIGHTS
        {
            let raw = std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>());
            fd = Some(OwnedFd::from_raw_fd(raw));
        }
        Ok((n as usize, fd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real holder (in a thread, on a socket of its own): a shell gets the
    /// env it was given, keeps running with nobody attached, what it prints
    /// meanwhile comes back as backlog, and kill ends it.
    #[test]
    fn holds_a_shell() {
        let dir = std::env::temp_dir().join(format!("abromux-hold-test-{}", std::process::id()));
        let sock = dir.join("hold.sock");
        // SAFETY: no other test reads it.
        unsafe { std::env::set_var("ABROMUX_HOLD_SOCKET", &sock) };
        let s = sock.clone();
        std::thread::spawn(move || serve(&s));
        while UnixStream::connect(&sock).is_err() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let read_until = |fd: &OwnedFd, want: &str| {
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            let end = Instant::now() + Duration::from_secs(5);
            while !String::from_utf8_lossy(&seen).contains(want) {
                assert!(Instant::now() < end, "never saw {want:?}: {}", String::from_utf8_lossy(&seen));
                let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                assert!(n > 0);
                seen.extend_from_slice(&buf[..n as usize]);
            }
        };

        let env = [("PROBE".to_string(), "held".to_string()), ("PATH".into(), "/bin:/usr/bin".into())];
        let argv = ["/bin/sh".to_string()];
        let h = Held::spawn(&argv, "/tmp", &env, (80, 24)).unwrap();
        assert!(h.pid > 0);
        let write = |fd: &OwnedFd, s: &str| unsafe {
            libc::write(fd.as_raw_fd(), s.as_ptr().cast(), s.len());
        };
        write(&h.fd, "echo $PROBE-$((40+2))\n");
        read_until(&h.fd, "held-42");
        // Let go (quit): it keeps running and the holder reads it meanwhile.
        write(&h.fd, "sleep 0.3; echo later-$((1+1))\n");
        let id = h.id.clone();
        drop(h);
        std::thread::sleep(Duration::from_millis(800));
        let mut h = Held::attach(&id).unwrap();
        assert!(String::from_utf8_lossy(&h.backlog).contains("later-2"), "{:?}", String::from_utf8_lossy(&h.backlog));
        write(&h.fd, "echo again\n");
        read_until(&h.fd, "again");
        assert!(Held::attach(&id).is_err(), "only one holder of a terminal");
        h.kill();
        drop(h);
        assert!(Held::attach(&id).is_err(), "gone after kill");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
