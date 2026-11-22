//! Forwarded ports (docs/server-plan.md, phase 5): a port on this machine
//! that connects to a port on a workspace's host, through that host's
//! abromux server (`{"op":"connect"}`, over ssh). So a browser tab here
//! reaches a dev server running in a workspace on a VPS, as Rex's tunnel
//! blocks do.
//!
//! Each forward belongs to a workspace: it's listed with it, saved with its
//! session, and closed with it.

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// One forwarded port, open while this lives.
pub struct Forward {
    /// The workspace's host, as it stores it (`ssh://..`).
    pub host: String,
    /// The port on the host.
    pub port: u16,
    /// The port here, on 127.0.0.1.
    pub local: u16,
    stop: Arc<AtomicBool>,
}

impl Forward {
    /// Listen on 127.0.0.1:`local` (any free port if None) and forward each
    /// connection to `port` on `host`.
    pub fn start(host: &str, port: u16, local: Option<u16>) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", local.unwrap_or(0)))
            .map_err(|e| format!("127.0.0.1:{}: {e}", local.unwrap_or(0)))?;
        let local = listener.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let (h, s) = (host.to_string(), stop.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if s.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(conn) = conn else { continue };
                let h = h.clone();
                std::thread::spawn(move || {
                    if let Err(e) = carry(conn, &h, port) {
                        eprintln!("abromux: forward {port} on {h}: {e}");
                    }
                });
            }
        });
        Ok(Forward {
            host: host.to_string(),
            port,
            local,
            stop,
        })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.local)
    }

    pub fn json(&self) -> Value {
        json!({
            "host": self.host,
            "server": crate::servers::label(&self.host),
            "port": self.port,
            "local": self.local,
            "url": self.url(),
        })
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake its accept so it sees the stop.
        let _ = TcpStream::connect(("127.0.0.1", self.local));
    }
}

/// One connection: to `port` on `host` through its server, then bytes
/// both ways until either side closes.
fn carry(conn: TcpStream, host: &str, port: u16) -> Result<(), String> {
    let first = json!({ "op": "connect", "port": port });
    let (mut remote, _) = abromux_term::remote::open_stream(host, Some(&first))?;
    // The reply line, read a byte at a time: what follows is the port's.
    let mut line = Vec::new();
    let mut b = [0u8];
    while b[0] != b'\n' {
        if remote.read(&mut b).map_err(|e| e.to_string())? == 0 {
            return Err("the server closed the connection".into());
        }
        line.push(b[0]);
    }
    let reply: Value = serde_json::from_slice(&line).map_err(|e| e.to_string())?;
    if reply["ok"] != true {
        return Err(reply["error"].as_str().unwrap_or("refused").to_string());
    }
    let _ = conn.set_nodelay(true);
    let (mut up_from, mut up_to) = (
        conn.try_clone().map_err(|e| e.to_string())?,
        remote.try_clone().map_err(|e| e.to_string())?,
    );
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut up_from, &mut up_to);
        let _ = up_to.shutdown(std::net::Shutdown::Write);
    });
    let mut down_to = conn;
    let _ = copy_unix(&mut remote, &mut down_to);
    let _ = down_to.shutdown(std::net::Shutdown::Write);
    let _ = up.join();
    Ok(())
}

fn copy_unix(from: &mut UnixStream, to: &mut TcpStream) -> std::io::Result<()> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        to.write_all(&buf[..n])?;
    }
}

/// Ports something started listening on, on a workspace's host: offered
/// to forward (⌘⇧P "Forward Port N of <server>"), until taken or forwarded.
static OFFERS: std::sync::Mutex<Vec<(String, u16)>> = std::sync::Mutex::new(Vec::new());

pub fn offer(host: &str, port: u16) {
    let mut o = OFFERS.lock().unwrap();
    if !o.iter().any(|(h, p)| h == host && *p == port) {
        o.push((host.to_string(), port));
    }
}

pub fn offers() -> Vec<(String, u16)> {
    OFFERS.lock().unwrap().clone()
}

pub fn withdraw(host: &str, port: u16) {
    OFFERS
        .lock()
        .unwrap()
        .retain(|(h, p)| !(h == host && *p == port));
}

/// What listens on `host` (its server's `ports`). Blocks on the network.
pub fn ports(host: &str) -> Result<Value, String> {
    let (s, _) = abromux_term::remote::open_stream(host, Some(&json!({ "op": "ports" })))?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .ok();
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(s), &mut line)
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    if v["ok"] != true {
        return Err(v["error"].as_str().unwrap_or("refused").to_string());
    }
    Ok(v["ports"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A port on "the host" (a server on a Unix socket here) reached
    /// through a forward.
    #[test]
    fn a_forward_carries_a_connection() {
        let sock = std::env::temp_dir().join(format!("abx-fwd-{}.sock", std::process::id()));
        let s = sock.clone();
        std::thread::spawn(move || {
            let opts = abromux_server::Options {
                version: "test".into(),
                ..Default::default()
            };
            abromux_server::serve(&s, &opts)
        });
        while UnixStream::connect(&sock).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let web = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = web.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut c in web.incoming().flatten() {
                let mut req = [0u8; 5];
                c.read_exact(&mut req).unwrap();
                c.write_all(b"HTTP/1.0 200 OK\r\n\r\nhello").unwrap();
            }
        });
        let host = sock.to_string_lossy().into_owned();
        let f = Forward::start(&host, port, None).unwrap();
        assert_ne!(f.local, port);
        for _ in 0..2 {
            let mut c = TcpStream::connect(("127.0.0.1", f.local)).unwrap();
            c.write_all(b"GET /").unwrap();
            let mut body = String::new();
            c.read_to_string(&mut body).unwrap();
            assert!(body.ends_with("hello"), "{body}");
        }
        let listed = ports(&host).unwrap();
        assert!(listed.as_array().unwrap().iter().any(|p| p["port"] == port));
        let local = f.local;
        drop(f);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            TcpStream::connect(("127.0.0.1", local)).is_err(),
            "closed with the forward"
        );
    }
}
