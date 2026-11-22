//! `abromux-server --tailnet`, with a stand-in `tailscale` that says the
//! tailnet address is 127.0.0.1 and who's calling: the raw protocol for the
//! owner, the web client (page, list, a terminal over a WebSocket), and a
//! stranger refused.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// A fake `tailscale`: this machine belongs to me@example.com, and the
/// caller is whoever the file `who` names.
fn fake_tailscale(dir: &Path) {
    let script = format!(
        r#"#!/bin/sh
case "$1" in
  ip) echo 127.0.0.1 ;;
  status) echo '{{"Self":{{"UserID":1}},"User":{{"1":{{"LoginName":"me@example.com"}}}}}}' ;;
  whois) echo "{{\"UserProfile\":{{\"LoginName\":\"$(cat {dir}/who)\"}},\"Node\":{{\"Tags\":[]}}}}" ;;
esac
"#,
        dir = dir.display()
    );
    let path = dir.join("tailscale");
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn connect(port: u16) -> TcpStream {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => {
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                return s;
            }
            Err(e) if Instant::now() > end => panic!("never listened: {e}"),
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn get(port: u16, path: &str) -> String {
    let mut s = connect(port);
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out)
        .unwrap_or_else(|e| panic!("GET {path}: {e}; got {out:?}"));
    out
}

#[test]
fn on_the_tailnet() {
    let dir = std::env::temp_dir().join(format!("abx-tailnet-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    fake_tailscale(&dir);
    std::fs::write(dir.join("who"), "me@example.com").unwrap();
    let port = 17_000 + (std::process::id() % 1000) as u16;
    let sock = dir.join("server.sock");
    let _server = Server(
        Command::new(env!("CARGO_BIN_EXE_abromux-server"))
            .args(["serve", "--socket"])
            .arg(&sock)
            .args(["--tailnet", &port.to_string()])
            .env("ABROMUX_TAILSCALE", dir.join("tailscale"))
            .spawn()
            .unwrap(),
    );

    // The raw protocol, as an app at tailnet://host speaks it.
    let mut s = connect(port);
    writeln!(s, "{}", json!({ "op": "hello" })).unwrap();
    let mut line = String::new();
    BufReader::new(s.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("\"protocol\":"), "{line}");

    // A terminal to look at.
    writeln!(
        s,
        "{}",
        json!({ "op": "spawn", "stream": true, "argv": ["/bin/sh"], "env": [["PATH", "/bin:/usr/bin"]] })
    )
    .unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut reply = String::new();
    r.read_line(&mut reply).unwrap();
    let id = serde_json::from_str::<Value>(&reply).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The web client: the page, the list.
    assert!(get(port, "/").contains("xterm.js"));
    let list = get(port, "/api/terms");
    assert!(list.contains(&id), "{list}");
    assert!(list.contains("me@example.com"), "{list}");

    // A terminal over a WebSocket: type, see it.
    let mut ws = connect(port);
    write!(
        ws,
        "GET /ws/{id} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\n\
         Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    )
    .unwrap();
    let mut wr = BufReader::new(ws.try_clone().unwrap());
    let mut head = String::new();
    while !head.ends_with("\r\n\r\n") {
        wr.read_line(&mut head).unwrap();
    }
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    // A masked text frame, as a browser sends.
    let msg = json!({ "input": "echo from-the-web-$((6*7))\n" }).to_string();
    let mask = [1u8, 2, 3, 4];
    let mut frame = vec![0x81, 0x80 | msg.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend(msg.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    ws.write_all(&frame).unwrap();
    let mut seen = Vec::new();
    let end = Instant::now() + Duration::from_secs(5);
    while !String::from_utf8_lossy(&seen).contains("from-the-web-42") {
        assert!(
            Instant::now() < end,
            "never saw it: {}",
            String::from_utf8_lossy(&seen)
        );
        let mut h = [0u8; 2];
        wr.read_exact(&mut h).unwrap();
        let len = match h[1] & 0x7F {
            126 => {
                let mut b = [0u8; 2];
                wr.read_exact(&mut b).unwrap();
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0u8; 8];
                wr.read_exact(&mut b).unwrap();
                u64::from_be_bytes(b) as usize
            }
            n => n as usize,
        };
        let mut data = vec![0u8; len];
        wr.read_exact(&mut data).unwrap();
        if h[0] & 0x0F == 0x2 {
            seen.extend(data);
        }
    }

    // A WebSocket from another site's page is refused.
    let mut other = connect(port);
    write!(
        other,
        "GET /ws/{id} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://evil.example\r\n\
         Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: x\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    other.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 403"), "{out}");

    // Someone else on the tailnet isn't let in.
    std::fs::write(dir.join("who"), "stranger@example.com").unwrap();
    assert!(get(port, "/").starts_with("HTTP/1.1 403"));
    let mut s = connect(port);
    writeln!(s, "{}", json!({ "op": "hello" })).unwrap();
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).unwrap();
    assert!(line.contains("isn't let in"), "{line}");
    let _ = std::fs::remove_dir_all(&dir);
}
