//! Reaching an abromux server: where it is ([`Endpoint`]), and a
//! connection to it over ssh (`abromux-server --stdio`, with a shared
//! `ControlMaster`), a Unix socket or the tailnet. Used by the app's remote
//! terminals (abromux_term::remote) and by the CLI (`--server`, `--host`)
//! wherever it runs.

use serde_json::{Value, json};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where a server is: `ssh://[user@]host[:port]` (or just `[user@]host`),
/// or `unix:///path/to/server.sock` (or an absolute path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Ssh {
        dest: String,
        port: Option<u16>,
    },
    Unix(PathBuf),
    /// `tailnet://host[:port]` (or `tcp://`): a server listening on its
    /// tailnet addresses (`--tailnet`), which tailscaled tells who's asking.
    Tcp {
        host: String,
        port: u16,
    },
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
        if let Some(rest) = s
            .strip_prefix("tailnet://")
            .or_else(|| s.strip_prefix("tcp://"))
        {
            let rest = rest.trim_end_matches('/');
            let (host, port) = match rest.rsplit_once(':') {
                Some((h, p)) if !h.contains(':') => {
                    (h, p.parse().map_err(|_| format!("bad port in {s:?}"))?)
                }
                _ => (rest, 7297),
            };
            if host.is_empty() || host.contains(['/', ' ', '@']) {
                return Err(format!("not a host: {s:?}"));
            }
            return Ok(Endpoint::Tcp {
                host: host.to_string(),
                port,
            });
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
            Endpoint::Tcp { host, .. } => host.clone(),
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
            Endpoint::Tcp { host, port } => {
                use std::net::ToSocketAddrs;
                let addr = (host.as_str(), *port)
                    .to_socket_addrs()
                    .map_err(|e| format!("{host}: {e}"))?
                    .next()
                    .ok_or_else(|| format!("{host}: no address"))?;
                let s = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(10))
                    .map_err(|e| format!("{host}:{port}: {e}"))?;
                let _ = s.set_nodelay(true);
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
pub struct Conn {
    pub read: OwnedFd,
    pub write: Pipe,
    pub stderr: Arc<Mutex<Vec<u8>>>,
}

/// The connection's write side, which owns ssh's process: dropping it
/// closes ssh's stdin, which ends ssh, and reaps it.
pub struct Pipe {
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
    pub fn error(&mut self, e: String) -> String {
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
pub fn read_exact(fd: &OwnedFd, mut buf: &mut [u8]) -> Result<(), String> {
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
pub fn read_line(fd: &OwnedFd) -> Result<Value, String> {
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
    hello_on(&mut endpoint.connect()?)
}

pub fn hello_on(conn: &mut Conn) -> Result<Value, String> {
    let sent = writeln!(conn.write, "{}", json!({ "op": "hello" }))
        .and_then(|_| conn.write.flush())
        .map_err(|e| e.to_string());
    match sent.and_then(|_| read_line(&conn.read)) {
        Ok(v) if v["ok"] == true => Ok(v),
        Ok(_) => Err("not an abromux server (an old holder?)".into()),
        Err(e) => Err(conn.error(e)),
    }
}

/// Other addresses of the server a host string names, tried after it when
/// it doesn't answer (the app's Settings › Servers). Set once by the app.
static ALTERNATES: std::sync::OnceLock<fn(&str) -> Vec<String>> = std::sync::OnceLock::new();

pub fn set_alternates(f: fn(&str) -> Vec<String>) {
    let _ = ALTERNATES.set(f);
}

/// The server at `host` (`endpoint`, then its other addresses), connected
/// and its `hello` answered: the first address that answers wins.
pub fn dial(host: &str, endpoint: &Endpoint) -> Result<(Conn, Value), String> {
    let mut tried = vec![endpoint.clone()];
    if let Some(f) = ALTERNATES.get() {
        tried.extend(
            f(host)
                .iter()
                .filter_map(|a| Endpoint::parse(a).ok())
                .filter(|e| e != endpoint),
        );
    }
    let mut errors = vec![];
    for ep in &tried {
        match ep
            .connect()
            .and_then(|mut c| hello_on(&mut c).map(|h| (c, h)))
        {
            Ok(ok) => return Ok(ok),
            Err(e) => errors.push(if tried.len() > 1 {
                format!("{}: {e}", ep.label())
            } else {
                e
            }),
        }
    }
    Err(errors.join("; "))
}

/// A connection to the server at `host` as a local socket: what's written
/// to it goes to the server and back, over ssh when it's elsewhere (two
/// threads copy). `first` goes out before anything else (`{"op":..}`).
pub fn open_stream(host: &str, first: Option<&Value>) -> Result<(UnixStream, Value), String> {
    let endpoint = Endpoint::parse(host)?;
    let (conn, hello) = dial(host, &endpoint)?;
    let (mine, theirs) = UnixStream::pair().map_err(|e| e.to_string())?;
    let Conn {
        read, mut write, ..
    } = conn;
    if let Some(f) = first {
        writeln!(write, "{f}").map_err(|e| e.to_string())?;
    }
    let mut down = theirs.try_clone().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: reads into `buf`, valid for its length.
            let n = unsafe { libc::read(read.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if n <= 0 || down.write_all(&buf[..n as usize]).is_err() {
                let _ = down.shutdown(std::net::Shutdown::Both);
                return;
            }
        }
    });
    let mut up = theirs;
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match io::Read::read(&mut up, &mut buf) {
                Ok(0) | Err(_) => return, // dropping `write` ends ssh
                Ok(n) => {
                    if write
                        .write_all(&buf[..n])
                        .and_then(|_| write.flush())
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
    Ok((mine, hello))
}

/// Which app this is to a server (`owner`): this machine. A streamed
/// attach with `take` replaces only its own connections, not another
/// machine's sharing the terminal.
pub fn owner() -> String {
    static OWNER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OWNER
        .get_or_init(|| {
            let mut buf = [0u8; 256];
            // SAFETY: gethostname writes at most `len` bytes into `buf`.
            unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
            let end = buf.iter().position(|&b| b == 0).unwrap_or(0);
            // SAFETY: getuid can't fail.
            let uid = unsafe { libc::getuid() };
            format!("{}:{uid}", String::from_utf8_lossy(&buf[..end]))
        })
        .clone()
}
