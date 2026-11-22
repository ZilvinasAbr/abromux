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
        ..Default::default()
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

    let (reply, mut r, _w) = request(&sock, json!({ "op": "attach", "id": id, "stream": true }));
    assert_eq!(reply["ok"], true, "{reply}");
    let mut snap = vec![0u8; reply["snapshot"].as_u64().unwrap() as usize];
    r.read_exact(&mut snap).unwrap();
    assert_eq!(snap, b"the screen");
    let mut backlog = vec![0u8; reply["backlog"].as_u64().unwrap() as usize];
    r.read_exact(&mut backlog).unwrap();
    let backlog = String::from_utf8_lossy(&backlog);
    assert!(backlog.contains("later-2"), "{backlog:?}");

    // One app at a time, unless it takes the terminal (coming back on a new
    // connection): then the old connection is closed.
    let (busy, ..) = request(&sock, json!({ "op": "attach", "id": id, "stream": true }));
    assert_eq!(busy["ok"], false);
    let (took, mut r2, mut w2) = request(
        &sock,
        json!({ "op": "attach", "id": id, "stream": true, "take": true }),
    );
    assert_eq!(took["ok"], true, "{took}");
    assert_eq!(took["backlog"], 0);
    while let Some((kind, _)) = read_frame(&mut r).unwrap() {
        assert_eq!(kind, wire::OUTPUT, "the old connection only ends");
    }
    let (r, w) = (&mut r2, &mut w2);

    write_frame(w, wire::INPUT, b"echo again\n").unwrap();
    read_until(r, "again");
    write_frame(w, wire::KILL, b"").unwrap();
    drop(w2);
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

// ---- the control socket -------------------------------------------------

/// A server with a control socket, in a thread.
fn server_with_control(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("abx-c-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sock = dir.join("s.sock");
    let control = dir.join("c.sock");
    let (s, c) = (sock.clone(), control.clone());
    std::thread::spawn(move || {
        let opts = Options {
            version: "test".into(),
            control: Some(c),
            ..Default::default()
        };
        serve(&s, &opts)
    });
    while UnixStream::connect(&control).is_err() {
        std::thread::sleep(Duration::from_millis(10));
    }
    (sock, control)
}

/// One control call: `abromux`'s round trip.
fn ctl(control: &Path, method: &str, params: Value) -> Value {
    let mut conn = UnixStream::connect(control).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    writeln!(
        conn,
        "{}",
        json!({ "id": 7, "method": method, "params": params })
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// The app's link: its reader, and a function that answers each control
/// connection the server hands it with `answer`.
fn app_link(sock: &Path) -> (BufReader<UnixStream>, UnixStream) {
    let (reply, r, w) = request(sock, json!({ "op": "app" }));
    assert_eq!(reply["ok"], true, "{reply}");
    (r, w)
}

/// Take the next `{"open":N}` from the link, open the app's side and
/// answer one request on it the way `f` says.
fn serve_one(sock: &Path, link: &mut BufReader<UnixStream>, f: impl Fn(&Value) -> Value) -> u64 {
    let mut line = String::new();
    link.read_line(&mut line).unwrap();
    let open: Value = serde_json::from_str(&line).unwrap();
    let n = open["open"].as_u64().expect("an open");
    let mut side = UnixStream::connect(sock).unwrap();
    side.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    writeln!(side, "{}", json!({ "op": "control", "conn": n })).unwrap();
    let mut r = BufReader::new(side.try_clone().unwrap());
    let mut req = String::new();
    r.read_line(&mut req).unwrap();
    let req: Value = serde_json::from_str(&req).unwrap();
    let out = f(&req);
    writeln!(side, "{out}").unwrap();
    n
}

#[test]
fn without_the_app_the_server_answers_and_keeps() {
    let (sock, control) = server_with_control("away");
    assert_eq!(ctl(&control, "system.ping", json!({}))["result"], "pong");
    let r = ctl(&control, "status.set", json!({ "key": "k", "value": "v" }));
    assert_eq!(r["result"]["queued"], true, "{r}");
    let r = ctl(&control, "window.focus", json!({}));
    assert_eq!(r["code"], "app_closed");
    let (hello, ..) = request(&sock, json!({ "op": "hello" }));
    assert_eq!(hello["protocol"], 3);
    assert_eq!(hello["app"], false);
    assert_eq!(hello["control"], json!(control));
    let id = hello["host_id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 24);
    let (again, ..) = request(&sock, json!({ "op": "hello" }));
    assert_eq!(
        again["host_id"],
        id.as_str(),
        "the same machine says the same"
    );
}

#[test]
fn with_the_app_calls_go_to_it_and_kept_ones_are_replayed() {
    let (sock, control) = server_with_control("app");
    // Kept while away.
    ctl(
        &control,
        "notification.create",
        json!({ "body": "while away" }),
    );

    let (mut link, _w) = app_link(&sock);
    let (second, ..) = request(&sock, json!({ "op": "app" }));
    assert_eq!(second["ok"], false, "one app at a time");
    // First the replay of what was kept.
    let n = serve_one(&sock, &mut link, |req| {
        assert_eq!(req["method"], "notification.create");
        assert_eq!(req["params"]["body"], "while away");
        json!({ "id": req["id"], "ok": true, "result": {} })
    });
    assert_eq!(n, 0, "the replay is connection 0");

    // Then a live call, answered by the app.
    let c = control.clone();
    let caller = std::thread::spawn(move || ctl(&c, "window.focus", json!({})));
    serve_one(&sock, &mut link, |req| {
        assert_eq!(req["method"], "window.focus");
        json!({ "id": req["id"], "ok": true, "result": "focused by the app" })
    });
    let r = caller.join().unwrap();
    assert_eq!(r["result"], "focused by the app");

    // State the app sends is what the server answers with once it's gone.
    writeln!(
        &_w,
        "{}",
        json!({ "op": "state", "workspaces": [{ "ref": "workspace:3", "name": "VPS", "selected": true, "surfaces": [] }] })
    )
    .unwrap();
    drop((link, _w));
    std::thread::sleep(Duration::from_millis(100));
    let r = ctl(&control, "workspace.list", json!({}));
    assert_eq!(r["result"][0]["name"], "VPS", "{r}");
    let r = ctl(&control, "status.set", json!({ "key": "k", "value": "v" }));
    assert_eq!(r["result"]["workspace"], "workspace:3");
}

/// A call waiting for the app when its link drops is answered by the server.
#[test]
fn a_call_waiting_on_a_gone_app_is_answered_by_the_server() {
    let (sock, control) = server_with_control("gone");
    let (mut link, w) = app_link(&sock);
    let c = control.clone();
    let caller = std::thread::spawn(move || ctl(&c, "system.ping", json!({})));
    let mut line = String::new();
    link.read_line(&mut line).unwrap();
    assert!(line.contains("open"));
    drop((link, w));
    let r = caller.join().unwrap();
    assert_eq!(r["result"], "pong");
}

/// `{"op":"control"}` on the server's own socket is a control connection
/// (what `--server` sends over ssh).
#[test]
fn control_over_the_server_socket() {
    let (sock, _control) = server_with_control("direct");
    let mut conn = UnixStream::connect(&sock).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        conn,
        "{}\n{}\n",
        json!({ "op": "control" }),
        json!({ "id": 1, "method": "system.ping" })
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line).unwrap();
    assert!(line.contains("pong"), "{line}");
}

/// Many starting at once: one serves, the others leave it to it.
#[test]
fn only_one_server_per_socket() {
    let dir = std::env::temp_dir().join(format!("abx-race-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sock = dir.join("s.sock");
    let started: Vec<_> = (0..8)
        .map(|_| {
            let s = sock.clone();
            std::thread::spawn(move || {
                let opts = Options {
                    version: "test".into(),
                    ..Default::default()
                };
                let t = Instant::now();
                let _ = serve(&s, &opts);
                t.elapsed()
            })
        })
        .collect();
    while UnixStream::connect(&sock).is_err() {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
    // All but one returned at once; the one serving is still at it.
    let done = started.iter().filter(|h| h.is_finished()).count();
    assert_eq!(done, 7, "seven left it to the first");
    let (hello, ..) = request(&sock, json!({ "op": "hello" }));
    assert_eq!(hello["ok"], true);
}
