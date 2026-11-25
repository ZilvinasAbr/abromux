//! `abromux-server --stdio`, as ssh runs it: it starts the server when none
//! is running, and a streamed terminal works through the pipe.

use abromux_core::wire::{self, read_frame, write_frame};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn a_terminal_through_stdio() {
    let dir = std::env::temp_dir().join(format!("abx-stdio-{}", std::process::id()));
    let sock = dir.join("server.sock");
    let bin = env!("CARGO_BIN_EXE_abromux-server");
    let bridge = |req: Value| {
        let mut child = Command::new(bin)
            .args(["--stdio", "--socket"])
            .arg(&sock)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut w = child.stdin.take().unwrap();
        writeln!(w, "{req}").unwrap();
        let mut r = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        (reply, r, w, child)
    };

    let (hello, ..) = bridge(json!({ "op": "hello" }));
    assert_eq!(hello["protocol"], wire::PROTOCOL);

    let (reply, mut r, mut w, mut child) = bridge(
        json!({ "op": "spawn", "stream": true, "argv": ["/bin/sh"], "env": [["PATH", "/bin:/usr/bin"]] }),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    write_frame(&mut w, wire::INPUT, b"echo via-$((6*7))\n").unwrap();
    let mut seen = String::new();
    while !seen.contains("via-42") {
        let (kind, p) = read_frame(&mut r).unwrap().expect("output");
        assert_eq!(kind, wire::OUTPUT);
        seen.push_str(&String::from_utf8_lossy(&p));
    }
    write_frame(&mut w, wire::KILL, b"").unwrap();
    drop(w);
    // The bridge ends with the connection (after any output still on its way).
    while let Some((kind, _)) = read_frame(&mut r).unwrap() {
        assert_eq!(kind, wire::OUTPUT);
    }
    child.wait().unwrap();

    // The server it started is still there, idle; ask it to go by removing
    // its socket (it exits after its idle time).
    let _ = std::fs::remove_dir_all(&dir);
}
