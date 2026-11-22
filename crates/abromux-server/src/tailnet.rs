//! `--tailnet [PORT]` (docs/server-plan.md, phase 6): the server on this
//! machine's Tailscale addresses (port 7297 by default), for clients that
//! can't run ssh, a phone's browser above all.
//!
//! As in Rex, the network and who's asking are Tailscale's job: the server
//! listens only on the tailnet addresses (`tailscale ip`), and asks
//! tailscaled who each caller is (`tailscale whois`). Only this machine's
//! own Tailscale user gets in, and the logins (or `tag:` names) given with
//! `--tailnet-allow`. Tagged devices never count as a person.
//!
//! A connection that starts with an HTTP request gets the web client
//! ([`crate::web`]); anything else speaks the server's own protocol, as an
//! app at `tailnet://host:7297` does, and is handed to the server's socket.

use serde_json::Value;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_PORT: u16 = 7297;

/// Who may connect besides this machine's Tailscale user.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub port: u16,
    /// Tailscale logins (`me@example.com`) and tags (`tag:phone`).
    pub allow: Vec<String>,
}

/// The `tailscale` command: `$ABROMUX_TAILSCALE`, else on PATH, else the
/// macOS app's.
fn tailscale() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ABROMUX_TAILSCALE").filter(|p| !p.is_empty()) {
        return Some(p.into());
    }
    let on_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|d| d.join("tailscale"))
            .find(|p| p.is_file())
    });
    on_path.or_else(|| {
        let app = PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale");
        app.is_file().then_some(app)
    })
}

fn run(args: &[&str]) -> Result<String, String> {
    let bin = tailscale().ok_or("tailscale isn't installed here")?;
    let out = Command::new(&bin)
        .args(args)
        .output()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!(
            "tailscale {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// This machine's tailnet addresses.
pub fn addresses() -> Result<Vec<IpAddr>, String> {
    let ips: Vec<IpAddr> = run(&["ip"])?
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    if ips.is_empty() {
        return Err("tailscale has no address here (is it up?)".into());
    }
    Ok(ips)
}

/// This machine's Tailscale user, by login.
pub fn owner() -> Result<String, String> {
    let v: Value = serde_json::from_str(&run(&["status", "--json"])?).map_err(|e| e.to_string())?;
    owner_of(&v).ok_or_else(|| "tailscale doesn't say who this machine belongs to".into())
}

fn owner_of(status: &Value) -> Option<String> {
    let id = status["Self"]["UserID"].as_u64()?;
    status["User"][id.to_string()]["LoginName"]
        .as_str()
        .map(String::from)
}

/// Who's calling from `peer`: their login, and their device's tags.
#[derive(Clone, Debug, PartialEq)]
pub struct Who {
    pub login: String,
    pub tags: Vec<String>,
}

pub fn whois(peer: SocketAddr) -> Result<Who, String> {
    let v: Value = serde_json::from_str(&run(&["whois", "--json", &peer.to_string()])?)
        .map_err(|e| e.to_string())?;
    Ok(Who {
        login: v["UserProfile"]["LoginName"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        tags: v["Node"]["Tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.as_str().map(String::from))
            .collect(),
    })
}

/// Let `who` in: the owner, or someone allowed. A tagged device is its tags,
/// never a person (it's shared, or a server).
pub fn allowed(who: &Who, owner: &str, allow: &[String]) -> bool {
    if !who.tags.is_empty() {
        return who.tags.iter().any(|t| allow.iter().any(|a| a == t));
    }
    !who.login.is_empty()
        && (who.login.eq_ignore_ascii_case(owner)
            || allow.iter().any(|a| a.eq_ignore_ascii_case(&who.login)))
}

/// Listen on the tailnet addresses, handing each caller that's let in to
/// the server at `socket` (or the web client). Returns once listening; the
/// listeners run on their own threads.
pub fn start(cfg: &Config, socket: &Path) -> Result<Vec<SocketAddr>, String> {
    let owner = owner()?;
    let mut bound = vec![];
    for ip in addresses()? {
        let addr = SocketAddr::new(ip, cfg.port);
        let l = TcpListener::bind(addr).map_err(|e| format!("{addr}: {e}"))?;
        bound.push(addr);
        let (cfg, owner, socket) = (cfg.clone(), owner.clone(), socket.to_path_buf());
        std::thread::spawn(move || listen(l, cfg, owner, socket, whois));
    }
    Ok(bound)
}

/// Serve `l`'s callers, asking `who` who each one is.
pub fn listen(
    l: TcpListener,
    cfg: Config,
    owner: String,
    socket: PathBuf,
    who: fn(SocketAddr) -> Result<Who, String>,
) {
    let shared = std::sync::Arc::new((cfg, owner, socket));
    for conn in l.incoming().flatten() {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let (cfg, owner, socket) = &*shared;
            let Ok(peer) = conn.peer_addr() else { return };
            let caller = match who(peer) {
                Ok(w) if allowed(&w, owner, &cfg.allow) => w,
                Ok(w) => return refuse(conn, &format!("{} isn't let in here", w.login)),
                Err(e) => return refuse(conn, &format!("can't tell who you are: {e}")),
            };
            let http = starts_with_get(&conn);
            if http {
                crate::web::serve(conn, socket, &caller);
            } else {
                let _ = proxy(conn, socket);
            }
        });
    }
}

/// Whether the caller's first bytes are an HTTP GET: waits for four of
/// them (a peek returns as soon as there's one).
fn starts_with_get(conn: &TcpStream) -> bool {
    let _ = conn.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let mut head = [0u8; 4];
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match conn.peek(&mut head) {
            Ok(4) => break,
            Ok(0) | Err(_) => return false,
            Ok(_) if std::time::Instant::now() > end => return false,
            Ok(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    let _ = conn.set_read_timeout(None);
    &head == b"GET "
}

fn refuse(mut conn: TcpStream, why: &str) {
    if starts_with_get(&conn) {
        // Read the request first: closing on it unread resets the
        // connection, and the browser shows that instead of why.
        let _ = crate::http::read(&conn);
        let _ = crate::http::refuse(&conn, 403, why);
    } else {
        // Its first request, answered with why not.
        let mut line = String::new();
        let _ = conn.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let _ = io::BufRead::read_line(&mut io::BufReader::new(&conn), &mut line);
        let _ = writeln!(conn, "{}", serde_json::json!({ "ok": false, "error": why }));
        let _ = conn.shutdown(std::net::Shutdown::Write);
    }
}

/// The server's own protocol over TCP: the connection, as is, to its socket.
fn proxy(conn: TcpStream, socket: &Path) -> io::Result<()> {
    let _ = conn.set_nodelay(true);
    let unix = UnixStream::connect(socket)?;
    let (mut a, mut b) = (conn.try_clone()?, unix.try_clone()?);
    let up = std::thread::spawn(move || {
        let _ = copy(&mut a, &mut b);
        let _ = b.shutdown(std::net::Shutdown::Write);
    });
    let (mut c, mut d) = (unix, conn);
    let _ = copy(&mut c, &mut d);
    let _ = d.shutdown(std::net::Shutdown::Write);
    let _ = up.join();
    Ok(())
}

/// Read and write, not io::copy (whose splice paths are for pipes).
fn copy(from: &mut impl Read, to: &mut impl Write) -> io::Result<()> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        to.write_all(&buf[..n])?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_is_let_in() {
        let me = |login: &str| Who {
            login: login.into(),
            tags: vec![],
        };
        let allow = vec!["friend@example.com".to_string(), "tag:phone".to_string()];
        assert!(allowed(&me("me@example.com"), "me@example.com", &[]));
        assert!(allowed(&me("Friend@example.com"), "me@example.com", &allow));
        assert!(!allowed(
            &me("stranger@example.com"),
            "me@example.com",
            &allow
        ));
        assert!(!allowed(&me(""), "", &[]));
        let tagged = |t: &str| Who {
            login: "me@example.com".into(),
            tags: vec![t.into()],
        };
        assert!(
            !allowed(&tagged("tag:server"), "me@example.com", &allow),
            "a tagged device isn't its owner"
        );
        assert!(allowed(&tagged("tag:phone"), "me@example.com", &allow));
    }

    #[test]
    fn owner_from_status() {
        let v = serde_json::json!({ "Self": { "UserID": 42 }, "User": { "42": { "LoginName": "me@example.com" } } });
        assert_eq!(owner_of(&v).as_deref(), Some("me@example.com"));
    }
}
