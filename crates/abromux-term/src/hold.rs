//! The app's side of `abromux server` (abromux_server; formerly `abromux
//! hold`): how abromux keeps shells running across a quit, crash or update
//! (`"persistence": "abromux"`, the default).
//!
//! The server is a small background process, one per app channel, started on
//! demand and detached from it (its own session, so quitting abromux doesn't
//! hang it up). It opens the PTYs and is the shells' parent, so they live as
//! long as it does.
//!
//! While abromux is attached to a terminal, the server hands it the PTY
//! itself (the master's file descriptor, over the Unix socket with
//! `SCM_RIGHTS`) and doesn't touch it: abromux reads and writes it exactly
//! as it does a PTY of its own, so a held terminal costs nothing extra while
//! abromux runs. Each terminal has its own connection to the server; when it
//! closes (quit, crash), the server starts reading that PTY again into a
//! backlog, so the program never blocks on a full PTY.
//!
//! On quit, abromux stops reading, writes each terminal's state with
//! libghostty-vt's snapshot (`snapshot_path`), then lets go. The next launch
//! attaches: the snapshot, then the backlog, give back the screen exactly as
//! the program left it. After a crash there's no snapshot, only the backlog.
//!
//! The socket is `server.sock` in the app's folder ([`set_dir`]; so the
//! release and nightly apps each have their own server), or
//! `$ABROMUX_SERVER_SOCKET` (`$ABROMUX_HOLD_SOCKET` as before). Terminals an
//! older app's holder keeps (`hold.sock`) are still taken back from it.

use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// Where the server's socket goes, and where an older app's holder's was.
static DIRS: std::sync::OnceLock<(PathBuf, PathBuf)> = std::sync::OnceLock::new();

/// Keep the server's socket in `dir` (the app's folder), and look for an
/// older app's holder in `legacy` (where it kept `hold.sock`). Call it
/// once, before anything here; later calls are ignored.
pub fn set_dir(dir: PathBuf, legacy: PathBuf) {
    let _ = DIRS.set((dir, legacy));
}

fn dirs() -> &'static (PathBuf, PathBuf) {
    DIRS.get().expect("hold::set_dir before using the server")
}

fn env_socket() -> Option<PathBuf> {
    ["ABROMUX_SERVER_SOCKET", "ABROMUX_HOLD_SOCKET"]
        .into_iter()
        .find_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

pub fn socket_path() -> PathBuf {
    env_socket().unwrap_or_else(|| dirs().0.join("server.sock"))
}

/// An older app's holder, still keeping terminals from before an update.
fn legacy_socket_path() -> Option<PathBuf> {
    if env_socket().is_some() {
        return None;
    }
    Some(dirs().1.join("hold.sock")).filter(|p| p.exists())
}

/// Where a terminal's state waits for the next launch.
pub fn snapshot_path(id: &str) -> PathBuf {
    let dir = socket_path().with_file_name("held");
    dir.join(format!("{id}.snap"))
}

/// The state a terminal left for this launch: [`snapshot_path`], or where an
/// older app's holder kept it.
pub fn take_snapshot(id: &str) -> Option<Vec<u8>> {
    let legacy = legacy_socket_path().map(|p| p.with_file_name("held").join(format!("{id}.snap")));
    [Some(snapshot_path(id)), legacy]
        .into_iter()
        .flatten()
        .find_map(|path| {
            let snap = std::fs::read(&path).ok()?;
            let _ = std::fs::remove_file(path);
            Some(snap)
        })
}

// ---- abromux's side ------------------------------------------------------

/// A terminal the server keeps, handed to us.
pub struct Held {
    pub id: String,
    pub pid: u32,
    /// The PTY's master.
    pub fd: OwnedFd,
    /// What it printed while nobody was attached.
    pub backlog: Vec<u8>,
    /// Closing it gives the PTY back to the server.
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
        let (reply, fd, conn, _) = request(&req, connect())?;
        Ok(Self {
            id: reply["id"].as_str().ok_or("no id")?.to_string(),
            pid: reply["pid"].as_u64().unwrap_or(0) as u32,
            fd,
            backlog: Vec::new(),
            conn,
        })
    }

    /// Take a terminal back from the server, with what it printed meanwhile:
    /// or from an older app's holder, after an update.
    pub fn attach(id: &str) -> Result<Self, String> {
        let req = json!({ "op": "attach", "id": id });
        let (reply, fd, conn, backlog) = match request(&req, connect()) {
            Err(e) if e.starts_with("no terminal") => match legacy_socket_path() {
                Some(old) => request(&req, UnixStream::connect(old).map_err(|e| e.to_string()))?,
                None => return Err(e),
            },
            other => other?,
        };
        Ok(Self {
            id: id.to_string(),
            pid: reply["pid"].as_u64().unwrap_or(0) as u32,
            fd,
            backlog,
            conn,
        })
    }

    /// The program's exit status, once it has ended (None if unknown).
    pub fn status(&mut self) -> Option<i32> {
        writeln!(self.conn, "{}", json!({ "op": "status" })).ok()?;
        let _ = self.conn.set_read_timeout(Some(Duration::from_secs(2)));
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(&self.conn), &mut line).ok()?;
        let reply: Value = serde_json::from_str(&line).ok()?;
        reply["code"].as_i64().map(|c| c as i32)
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

/// Send `req` on `conn`, and read the reply with its descriptor and any
/// backlog.
fn request(
    req: &Value,
    conn: Result<UnixStream, String>,
) -> Result<(Value, OwnedFd, UnixStream, Vec<u8>), String> {
    let mut conn = conn?;
    writeln!(conn, "{req}").map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    let mut fd = None;
    let mut buf = vec![0u8; 1 << 16];
    let nl = loop {
        let (n, got) = recv_with_fd(&conn, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("the server hung up".into());
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
    let fd = fd.ok_or("the server sent no PTY")?;
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

/// Connect to the server, starting it (`abromux server`, detached) if needed.
fn connect() -> Result<UnixStream, String> {
    let path = socket_path();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let args: Vec<OsString> = vec!["server".into(), "--socket".into(), path.clone().into()];
    abromux_server::connect_or_start(&path, &exe, &args)
        .map_err(|e| format!("can't start abromux server: {e}"))
}

// ---- descriptors over the socket -----------------------------------------

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
    use std::time::Instant;

    /// A real server (in a thread, on a socket of its own): a shell gets the
    /// env it was given, keeps running with nobody attached, what it prints
    /// meanwhile comes back as backlog, and kill ends it.
    #[test]
    fn holds_a_shell() {
        let dir = std::env::temp_dir().join(format!("abromux-hold-test-{}", std::process::id()));
        let sock = dir.join("server.sock");
        // SAFETY: no other test reads it.
        unsafe { std::env::set_var("ABROMUX_SERVER_SOCKET", &sock) };
        let s = sock.clone();
        let opts = abromux_server::Options {
            version: "test".into(),
        };
        std::thread::spawn(move || abromux_server::serve(&s, &opts));
        while UnixStream::connect(&sock).is_err() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let read_until = |fd: &OwnedFd, want: &str| {
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            let end = Instant::now() + Duration::from_secs(5);
            while !String::from_utf8_lossy(&seen).contains(want) {
                assert!(
                    Instant::now() < end,
                    "never saw {want:?}: {}",
                    String::from_utf8_lossy(&seen)
                );
                let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                assert!(n > 0);
                seen.extend_from_slice(&buf[..n as usize]);
            }
        };

        let env = [
            ("PROBE".to_string(), "held".to_string()),
            ("PATH".into(), "/bin:/usr/bin".into()),
        ];
        let argv = ["/bin/sh".to_string()];
        let h = Held::spawn(&argv, "/tmp", &env, (80, 24)).unwrap();
        assert!(h.pid > 0);
        let write = |fd: &OwnedFd, s: &str| unsafe {
            libc::write(fd.as_raw_fd(), s.as_ptr().cast(), s.len());
        };
        write(&h.fd, "echo $PROBE-$((40+2))\n");
        read_until(&h.fd, "held-42");
        // Let go (quit): it keeps running and the server reads it meanwhile.
        write(&h.fd, "sleep 0.3; echo later-$((1+1))\n");
        let id = h.id.clone();
        drop(h);
        std::thread::sleep(Duration::from_millis(800));
        let mut h = Held::attach(&id).unwrap();
        assert!(
            String::from_utf8_lossy(&h.backlog).contains("later-2"),
            "{:?}",
            String::from_utf8_lossy(&h.backlog)
        );
        write(&h.fd, "echo again\n");
        read_until(&h.fd, "again");
        assert!(Held::attach(&id).is_err(), "one app per terminal");
        h.kill();
        drop(h);
        assert!(Held::attach(&id).is_err(), "gone after kill");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
