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

/// What a streamed attach sends before its frames, as text: the server's
/// parsed screen when it has one, else the snapshot an app left and the
/// backlog.
fn attached_screen(reply: &Value, r: &mut impl Read) -> String {
    let n = |k: &str| reply[k].as_u64().unwrap_or(0) as usize;
    let mut screen = vec![0u8; n("screen")];
    let mut snap = vec![0u8; n("snapshot")];
    let mut backlog = vec![0u8; n("backlog")];
    r.read_exact(&mut screen).unwrap();
    r.read_exact(&mut snap).unwrap();
    r.read_exact(&mut backlog).unwrap();
    let mut text = String::from_utf8_lossy(&screen).into_owned();
    if reply["parsed"] == true {
        #[cfg(feature = "vt")]
        text.push_str(
            &abromux_vt::Terminal::from_snapshot(&snap, true)
                .unwrap()
                .screen_text(),
        );
    } else {
        text.push_str(&String::from_utf8_lossy(&snap));
    }
    text + &String::from_utf8_lossy(&backlog)
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

    let (reply, mut r, _w) = request(
        &sock,
        json!({ "op": "attach", "id": id, "stream": true, "owner": "mac-a" }),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    // The app's own snapshot and the backlog, or (the server parses it)
    // the server's whole screen.
    let screen = attached_screen(&reply, &mut r);
    assert!(screen.contains("later-2"), "{screen:?}");
    if reply["parsed"] != true {
        assert!(screen.starts_with("the screen"), "{screen:?}");
    }

    // Another app streams it too: both see what it prints, either types.
    let (other, mut ro, mut wo) = request(
        &sock,
        json!({ "op": "attach", "id": id, "stream": true, "owner": "mac-b" }),
    );
    assert_eq!(other["ok"], true, "{other}");
    attached_screen(&other, &mut ro);
    write_frame(&mut wo, wire::INPUT, b"echo both-$((2+2))\n").unwrap();
    read_until(&mut ro, "both-4");
    read_until(&mut r, "both-4");

    // The same app coming back on a new connection takes its own over; the
    // other app's stays.
    let (took, mut r2, mut w2) = request(
        &sock,
        json!({ "op": "attach", "id": id, "stream": true, "take": true, "owner": "mac-a" }),
    );
    assert_eq!(took["ok"], true, "{took}");
    assert_eq!(took["backlog"], 0);
    attached_screen(&took, &mut r2);
    while let Some((kind, _)) = read_frame(&mut r).unwrap() {
        assert_eq!(kind, wire::OUTPUT, "the old connection only ends");
    }
    let (r, w) = (&mut r2, &mut w2);
    write_frame(w, wire::INPUT, b"echo still-b\n").unwrap();
    read_until(&mut ro, "still-b");
    drop((ro, wo));

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
    let screen = attached_screen(&reply, &mut r);
    assert!(screen.contains("after-drop"), "{screen:?}");
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

// ---- several clients, server-side parsing (phase 4) ------------------------

/// What `terms` says about terminal `id`.
fn term_info(sock: &Path, id: &str) -> Value {
    let (r, ..) = request(sock, json!({ "op": "terms" }));
    r["terms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .cloned()
        .unwrap_or(Value::Null)
}

/// A terminal nobody watches keeps being parsed: its title, and a
/// notification it prints is kept for the app.
#[cfg(feature = "vt")]
#[test]
fn parses_what_nobody_watches() {
    let (sock, control) = server_with_control("parse");
    let env = json!([
        ["ABROMUX_SURFACE_ID", "surface:5"],
        ["PATH", "/bin:/usr/bin"]
    ]);
    let (reply, mut r, mut w) = request(
        &sock,
        json!({ "op": "spawn", "stream": true, "argv": sh(), "env": env }),
    );
    let id = reply["id"].as_str().unwrap().to_string();
    write_frame(&mut w, wire::DETACH, b"").unwrap();
    while read_frame(&mut r).unwrap().is_some() {}
    // Printed while nobody watches.
    let (reply2, mut r2, mut w2) =
        request(&sock, json!({ "op": "attach", "id": id, "stream": true }));
    attached_screen(&reply2, &mut r2);
    // (attach to type, then let go again)
    write_frame(
        &mut w2,
        wire::INPUT,
        b"sleep 0.3; printf '\\033]2;building\\007\\033]9;tests passed\\007'\n",
    )
    .unwrap();
    write_frame(&mut w2, wire::DETACH, b"").unwrap();
    while read_frame(&mut r2).unwrap().is_some() {}
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(term_info(&sock, &id)["title"], "building");
    let n = ctl(&control, "notification.list", json!({}));
    let list = n["result"].as_array().unwrap();
    assert!(
        list.iter()
            .any(|x| x["body"] == "tests passed" && x["surface"] == "surface:5"),
        "{n}"
    );
}

/// Another app with no screen of its own gets the server's.
#[cfg(feature = "vt")]
#[test]
fn a_fresh_client_gets_the_parsed_screen() {
    let sock = server("fresh");
    let (reply, mut r, mut w) = request(
        &sock,
        json!({ "op": "spawn", "stream": true, "argv": sh(), "env": [["PATH", "/bin:/usr/bin"]] }),
    );
    let id = reply["id"].as_str().unwrap().to_string();
    write_frame(&mut w, wire::INPUT, b"echo on-the-screen\n").unwrap();
    read_until(&mut r, "on-the-screen");
    let (other, mut ro, _wo) = request(
        &sock,
        json!({ "op": "attach", "id": id, "stream": true, "fresh": true }),
    );
    assert_eq!(other["parsed"], true, "{other}");
    let mut snap = vec![0u8; other["snapshot"].as_u64().unwrap() as usize];
    ro.read_exact(&mut snap).unwrap();
    let screen = abromux_vt::Terminal::from_snapshot(&snap, true)
        .unwrap()
        .screen_text();
    assert!(screen.contains("on-the-screen"), "{screen}");
}

/// A streamed attach to a terminal a local app holds asks the app to let
/// go, and is served once it has.
#[test]
fn a_second_client_asks_the_local_app_to_share() {
    let (sock, _control) = server_with_control("share");
    // The local app: its link, and a terminal it holds (the descriptor
    // comes with the reply; this test only needs the connection).
    let (mut link, _lw) = app_link(&sock);
    let (reply, hr, holder) = request(
        &sock,
        json!({ "op": "spawn", "argv": sh(), "env": [["PATH", "/bin:/usr/bin"]] }),
    );
    let id = reply["id"].as_str().unwrap().to_string();
    assert_eq!(term_info(&sock, &id)["held"], true);

    // Another app wants it.
    let s = sock.clone();
    let i = id.clone();
    let other = std::thread::spawn(move || {
        let (reply, r, w) = request(
            &s,
            json!({ "op": "attach", "id": i, "stream": true, "fresh": true }),
        );
        (reply, r, w)
    });
    let mut line = String::new();
    link.read_line(&mut line).unwrap();
    let msg: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(msg["release"], id.as_str(), "the app is asked to share");
    // The app lets go (and would stream it itself now).
    drop((hr, holder));
    let (reply, mut r, mut w) = other.join().unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    let mut skip = vec![
        0u8;
        reply["snapshot"].as_u64().unwrap() as usize
            + reply["backlog"].as_u64().unwrap() as usize
    ];
    r.read_exact(&mut skip).unwrap();
    write_frame(&mut w, wire::INPUT, b"echo shared-$((3*3))\n").unwrap();
    read_until(&mut r, "shared-9");
    assert_eq!(term_info(&sock, &id)["held"], false);
    assert_eq!(term_info(&sock, &id)["streams"], 1);
}

/// Once the other app leaves, the local app (streaming it now) is told it
/// can hold it again.
#[test]
fn the_local_app_is_told_to_hold_it_again() {
    let (sock, _control) = server_with_control("reclaim");
    let (reply, r, w) = request(&sock, json!({ "op": "app", "owner": "mac-a" }));
    assert_eq!(reply["ok"], true);
    let mut link = r;
    let _lw = w;
    let (reply, _ra, _wa) = request(
        &sock,
        json!({ "op": "spawn", "stream": true, "argv": sh(), "owner": "mac-a" }),
    );
    let id = reply["id"].as_str().unwrap().to_string();
    let (other, rb, wb) = request(
        &sock,
        json!({ "op": "attach", "id": id, "stream": true, "owner": "mac-b" }),
    );
    assert_eq!(other["ok"], true);
    drop((rb, wb));
    let mut line = String::new();
    link.read_line(&mut line).unwrap();
    let msg: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(msg["reclaim"], id.as_str(), "{msg}");
}

/// A port forwarded through the server: `connect`, then bytes both ways.
#[test]
fn forwards_a_port() {
    let sock = server("forward");
    let web = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = web.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut c, _) = web.accept().unwrap();
        let mut req = [0u8; 4];
        c.read_exact(&mut req).unwrap();
        c.write_all(b"pong").unwrap();
    });
    let (ports, ..) = request(&sock, json!({ "op": "ports" }));
    assert!(
        ports["ports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["port"] == port),
        "{ports}"
    );
    let (reply, mut r, mut w) = request(&sock, json!({ "op": "connect", "port": port }));
    assert_eq!(reply["ok"], true, "{reply}");
    w.write_all(b"ping").unwrap();
    let mut back = [0u8; 4];
    r.read_exact(&mut back).unwrap();
    assert_eq!(&back, b"pong");
    let (refused, ..) = request(&sock, json!({ "op": "connect", "port": 1 }));
    assert_eq!(refused["ok"], false);
}
