use super::*;
use abromux_core::wire::{read_frame, write_frame};
use std::io::{BufRead, BufReader};

/// A server in a thread, on a socket of its own.
fn server(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("abx-{name}-{}", std::process::id()));
    let sock = dir.join("s.sock");
    let s = sock.clone();
    let opts = Options {
        version: "test".into(),
    };
    std::thread::spawn(move || serve(&s, &opts));
    while UnixStream::connect(&sock).is_err() {
        std::thread::sleep(Duration::from_millis(10));
    }
    sock
}

/// Send one request; the reply line, and the reader to go on with.
fn request(sock: &Path, req: Value) -> (Value, BufReader<UnixStream>, UnixStream) {
    let mut conn = UnixStream::connect(sock).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    writeln!(conn, "{req}").unwrap();
    let mut r = BufReader::new(conn.try_clone().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    (serde_json::from_str(&line).unwrap(), r, conn)
}

/// Read OUTPUT frames until `want` shows up in them.
fn read_until(r: &mut impl Read, want: &str) -> String {
    let mut seen = String::new();
    while !seen.contains(want) {
        let (kind, p) = read_frame(r)
            .unwrap()
            .unwrap_or_else(|| panic!("stream ended before {want:?}: {seen}"));
        assert_eq!(kind, wire::OUTPUT, "{seen}");
        seen.push_str(&String::from_utf8_lossy(&p));
    }
    seen
}

fn sh() -> Value {
    json!(["/bin/sh"])
}

#[test]
fn hello_reports_the_protocol() {
    let sock = server("hello");
    let (reply, ..) = request(&sock, json!({ "op": "hello" }));
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["server"], "abromux");
    assert_eq!(reply["protocol"], wire::PROTOCOL);
    assert_eq!(reply["version"], "test");
}

/// A streamed terminal: input and output in frames, a resize the program
/// sees, a detach that leaves it running with its output kept, and an
/// attach that brings back the app's snapshot and that output.
#[test]
fn streams_a_terminal_and_takes_it_back() {
    let sock = server("stream");
    let env = json!([["PROBE", "streamed"], ["PATH", "/bin:/usr/bin"]]);
    let (reply, mut r, mut w) = request(
        &sock,
        json!({ "op": "spawn", "stream": true, "argv": sh(), "env": env, "cwd": "/tmp" }),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let id = reply["id"].as_str().unwrap().to_string();

    write_frame(&mut w, wire::INPUT, b"echo $PROBE-$((40+2))\n").unwrap();
    read_until(&mut r, "streamed-42");

    let size = Size {
        cols: 123,
        rows: 45,
        width: 0,
        height: 0,
    };
    write_frame(&mut w, wire::RESIZE, &size.encode()).unwrap();
    write_frame(&mut w, wire::INPUT, b"stty size\n").unwrap();
    read_until(&mut r, "45 123");

    // Quit: the stream ends at DETACHED, and what follows goes to the backlog.
    write_frame(&mut w, wire::SNAPSHOT, b"the screen").unwrap();
    write_frame(&mut w, wire::INPUT, b"sleep 0.3; echo later-$((1+1))\n").unwrap();
    write_frame(&mut w, wire::DETACH, b"").unwrap();
    loop {
        let (kind, _) = read_frame(&mut r)
            .unwrap()
            .expect("DETACHED before the end");
        if kind == wire::DETACHED {
            break;
        }
        assert_eq!(kind, wire::OUTPUT);
    }
    assert!(
        read_frame(&mut r).unwrap().is_none(),
        "closed after DETACHED"
    );
    std::thread::sleep(Duration::from_millis(800));

    let (reply, mut r, mut w) = request(&sock, json!({ "op": "attach", "id": id, "stream": true }));
    assert_eq!(reply["ok"], true, "{reply}");
    let mut snap = vec![0u8; reply["snapshot"].as_u64().unwrap() as usize];
    r.read_exact(&mut snap).unwrap();
    assert_eq!(snap, b"the screen");
    let mut backlog = vec![0u8; reply["backlog"].as_u64().unwrap() as usize];
    r.read_exact(&mut backlog).unwrap();
    let backlog = String::from_utf8_lossy(&backlog);
    assert!(backlog.contains("later-2"), "{backlog:?}");

    // One app at a time.
    let (busy, ..) = request(&sock, json!({ "op": "attach", "id": id, "stream": true }));
    assert_eq!(busy["ok"], false);

    write_frame(&mut w, wire::INPUT, b"echo again\n").unwrap();
    read_until(&mut r, "again");
    write_frame(&mut w, wire::KILL, b"").unwrap();
    drop(w);
    std::thread::sleep(Duration::from_millis(100));
    let (gone, ..) = request(&sock, json!({ "op": "attach", "id": id, "stream": true }));
    assert_eq!(gone["ok"], false, "gone after KILL");
}

/// A lost connection is a detach too: the program runs on.
#[test]
fn a_dropped_stream_lets_go() {
    let sock = server("drop");
    let (reply, _r, mut w) = request(
        &sock,
        json!({ "op": "spawn", "stream": true, "argv": sh(), "env": [["PATH", "/bin:/usr/bin"]] }),
    );
    let id = reply["id"].as_str().unwrap().to_string();
    write_frame(&mut w, wire::INPUT, b"sleep 0.2; echo after-drop\n").unwrap();
    drop((w, _r));
    std::thread::sleep(Duration::from_millis(600));
    let (reply, mut r, _w) = request(&sock, json!({ "op": "attach", "id": id, "stream": true }));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["snapshot"], 0);
    let mut backlog = vec![0u8; reply["backlog"].as_u64().unwrap() as usize];
    r.read_exact(&mut backlog).unwrap();
    assert!(String::from_utf8_lossy(&backlog).contains("after-drop"));
}

#[test]
fn a_streamed_program_reports_how_it_ended() {
    let sock = server("exit");
    let (reply, mut r, _w) = request(
        &sock,
        json!({ "op": "spawn", "stream": true, "argv": ["/bin/sh", "-c", "echo bye; exit 3"] }),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    loop {
        let (kind, p) = read_frame(&mut r).unwrap().expect("EXIT before the end");
        if kind == wire::EXIT {
            let v: Value = serde_json::from_slice(&p).unwrap();
            assert_eq!(v["code"], 3);
            break;
        }
    }
    assert!(read_frame(&mut r).unwrap().is_none());
}

/// A streamed spawn with no argv runs the server's user's shell, with the
/// server's environment under the app's variables, and a `command`.
#[test]
fn a_streamed_spawn_uses_this_machines_shell_and_env() {
    let sock = server("env");
    let (reply, mut r, _w) = request(
        &sock,
        json!({
            "op": "spawn", "stream": true, "argv": [],
            "command": "echo \"[$PROBE][${HOME:+home}]\"",
            "env": [["PROBE", "app"]],
            "cwd": "/nonexistent/abromux"
        }),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    read_until(&mut r, "[app][home]");
}

/// A program flooding a streamed app that doesn't read: the server stops
/// reading the PTY instead of queueing without bound, and stays responsive.
#[test]
fn a_slow_app_holds_the_program_back() {
    let sock = server("flood");
    let (reply, _r, _w) = request(
        &sock,
        json!({ "op": "spawn", "stream": true, "argv": ["/bin/sh", "-c", "yes flood"] }),
    );
    assert_eq!(reply["ok"], true);
    std::thread::sleep(Duration::from_millis(500));
    let start = Instant::now();
    let (hello, ..) = request(&sock, json!({ "op": "hello" }));
    assert_eq!(hello["ok"], true);
    assert!(start.elapsed() < Duration::from_millis(500));
}

#[test]
fn bad_requests_are_refused() {
    let sock = server("bad");
    let (reply, ..) = request(&sock, json!({ "op": "nope" }));
    assert_eq!(reply["ok"], false);
    let (reply, ..) = request(
        &sock,
        json!({ "op": "attach", "id": "1-1", "stream": true }),
    );
    assert_eq!(reply["ok"], false);
}
