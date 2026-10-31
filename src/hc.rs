//! herdr-core (`hc`) as a terminal backend: the shell runs in herdr-core's
//! holder instead of a PTY of our own, so it outlives abromux (quit, crash,
//! update). Picked with `"persistence": "herdr-core"` in the settings.
//!
//! Each terminal has its own `hc connect` process: the protocol over
//! stdin/stdout, which also finds the core's socket and starts the core when
//! it isn't running. The connection is switched to framed mode, the terminal
//! is attached, and from then on its stream carries the program's raw bytes
//! one way and keys the other (`schema/README.md` in herdr-core). Quitting
//! abromux closes the connection, which only detaches; closing a tab kills
//! the terminal (`Terminal::kill`).
//!
//! Each app gets its own core (`HC_SESSION=abromux` or `abromux-nightly`,
//! unless `$HC_SESSION` is set), so the release and nightly apps stay apart.
//! `$ABROMUX_HC` names the `hc`
//! binary; otherwise it's looked for on `$PATH` and in the usual install
//! folders, since an app started from Finder has a short `$PATH`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DATA: u8 = 0;
const CREDIT: u8 = 1;
const CLOSE: u8 = 2;
const RESIZE: u8 = 3;
/// Lines of scrollback asked for when attaching to a terminal that ran
/// while abromux was away.
const HISTORY: usize = 10_000;

/// What the stream brings.
pub enum Output {
    Bytes(Vec<u8>),
    /// The program is gone (or the core can't be reached any more).
    Closed,
}

/// The `hc` binary, if there is one.
pub fn binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ABROMUX_HC") {
        return Some(p.into());
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain([
            home.join(".local/bin"),
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
        ])
        .map(|d| d.join("hc"))
        .find(|p| p.is_file())
}

/// `$HC_SESSION` if set, else one per app.
fn session() -> String {
    std::env::var("HC_SESSION").unwrap_or_else(|_| {
        if crate::channel::nightly() {
            "abromux-nightly".into()
        } else {
            "abromux".into()
        }
    })
}

/// One live connection: where keys go and which stream is ours.
struct Link {
    stdin: ChildStdin,
    channel: u32,
    process: Child,
}

impl Drop for Link {
    fn drop(&mut self) {
        // Detaches; the terminal keeps running in the core.
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

pub struct Terminal {
    pub address: String,
    pub pid: Option<u32>,
    link: Arc<Mutex<Option<Link>>>,
    size: Arc<Mutex<(u16, u16)>>,
}

impl Terminal {
    /// Start `argv` in `cwd` with `env` (on top of the core's own) and attach.
    pub fn spawn(
        cwd: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
        size: (u16, u16),
        on_output: impl FnMut(Output) -> bool + Send + 'static,
    ) -> Result<Self, String> {
        let (mut stdin, mut stdout, process) = connect()?;
        let run = call(
            &mut stdin,
            &mut stdout,
            "terminal.run",
            serde_json::json!({ "argv": argv, "cwd": cwd, "env": env, "labels": { "abromux": "1" } }),
        )?;
        let address = run["address"]
            .as_str()
            .ok_or("terminal.run: no address")?
            .to_string();
        Self::start(address, (stdin, stdout, process), size, 0, on_output)
    }

    /// Attach to a terminal started by an earlier run of abromux, with its
    /// screen and history.
    pub fn attach(
        address: &str,
        size: (u16, u16),
        on_output: impl FnMut(Output) -> bool + Send + 'static,
    ) -> Result<Self, String> {
        Self::start(address.into(), connect()?, size, HISTORY, on_output)
    }

    fn start(
        address: String,
        (mut stdin, mut stdout, process): (ChildStdin, ChildStdout, Child),
        size: (u16, u16),
        history: usize,
        mut on_output: impl FnMut(Output) -> bool + Send + 'static,
    ) -> Result<Self, String> {
        let (channel, record) = attach(&mut stdin, &mut stdout, &address, size, history)?;
        let pid = record["pid"].as_u64().map(|p| p as u32);
        let link = Arc::new(Mutex::new(Some(Link {
            stdin,
            channel,
            process,
        })));
        let size = Arc::new(Mutex::new(size));
        let (link2, size2, addr2) = (link.clone(), size.clone(), address.clone());
        std::thread::spawn(move || {
            let mut stdout = stdout;
            loop {
                match pump(&mut stdout, &link2, &mut on_output) {
                    Pumped::Closed | Pumped::Dropped => break,
                    Pumped::Lost => {
                        // The core went away (restart, update): its programs
                        // live on in the holder, so attach again.
                        match reconnect(&addr2, &link2, &size2) {
                            Some(out) => stdout = out,
                            None => break,
                        }
                    }
                }
            }
            on_output(Output::Closed);
        });
        Ok(Self {
            address,
            pid,
            link,
            size,
        })
    }

    pub fn write(&self, bytes: &[u8]) {
        if let Some(l) = self.link.lock().unwrap().as_mut() {
            let ch = l.channel;
            let _ = frame(&mut l.stdin, ch, DATA, bytes);
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        *self.size.lock().unwrap() = (cols, rows);
        if let Some(l) = self.link.lock().unwrap().as_mut() {
            let mut p = cols.to_le_bytes().to_vec();
            p.extend(rows.to_le_bytes());
            let ch = l.channel;
            let _ = frame(&mut l.stdin, ch, RESIZE, &p);
        }
    }

    /// End the program and forget the terminal (closing its tab).
    pub fn kill(&self) {
        // Detach first, so the stream's end doesn't read as the core going away.
        let Some(link) = self.link.lock().unwrap().take() else {
            return;
        };
        let address = self.address.clone();
        std::thread::spawn(move || {
            drop(link);
            // On a connection of its own, one call after the other: `rm`
            // refuses a terminal that's still live.
            let Ok((mut stdin, mut stdout, _process)) = connect() else {
                return;
            };
            let params = serde_json::json!({ "terminal": address });
            let _ = call(&mut stdin, &mut stdout, "terminal.kill", params.clone());
            let _ = call(&mut stdin, &mut stdout, "terminal.rm", params);
        });
    }
}

impl Drop for Terminal {
    /// Detach: the program keeps running for the next launch.
    fn drop(&mut self) {
        self.link.lock().unwrap().take();
    }
}

enum Pumped {
    /// The program ended.
    Closed,
    /// The connection broke without a word: the core is gone.
    Lost,
    /// We let go of the link (killed, or the view dropped).
    Dropped,
}

fn pump(
    stdout: &mut ChildStdout,
    link: &Mutex<Option<Link>>,
    on_output: &mut impl FnMut(Output) -> bool,
) -> Pumped {
    loop {
        let Ok((ch, kind, payload)) = read_frame(stdout) else {
            return if link.lock().unwrap().is_some() {
                Pumped::Lost
            } else {
                Pumped::Dropped
            };
        };
        let mut guard = link.lock().unwrap();
        let Some(l) = guard.as_mut() else {
            return Pumped::Dropped;
        };
        if ch != l.channel {
            continue; // replies on channel 0
        }
        match kind {
            DATA => {
                let n = payload.len() as u32;
                let lch = l.channel;
                let _ = frame(&mut l.stdin, lch, CREDIT, &n.to_le_bytes());
                drop(guard);
                if !on_output(Output::Bytes(payload)) {
                    // The view is gone: detach.
                    link.lock().unwrap().take();
                    return Pumped::Dropped;
                }
            }
            CLOSE => return Pumped::Closed,
            _ => {}
        }
    }
}

/// Attach again after the core restarted, for up to ~30 s.
fn reconnect(
    address: &str,
    link: &Mutex<Option<Link>>,
    size: &Mutex<(u16, u16)>,
) -> Option<ChildStdout> {
    for _ in 0..30 {
        std::thread::sleep(Duration::from_secs(1));
        if link.lock().unwrap().is_none() {
            return None;
        }
        let Ok((mut stdin, mut stdout, process)) = connect() else {
            continue;
        };
        let size = *size.lock().unwrap();
        match attach(&mut stdin, &mut stdout, address, size, 0) {
            Ok((channel, _)) => {
                let mut guard = link.lock().unwrap();
                if guard.is_none() {
                    return None;
                }
                *guard = Some(Link {
                    stdin,
                    channel,
                    process,
                });
                return Some(stdout);
            }
            // It's gone for good (not found, finished).
            Err(_) => return None,
        }
    }
    None
}

fn connect() -> Result<(ChildStdin, ChildStdout, Child), String> {
    let hc = binary().ok_or("herdr-core (hc) isn't installed")?;
    let mut process = Command::new(&hc)
        .arg("connect")
        .env("HC_SESSION", session())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", hc.display()))?;
    let mut stdin = process.stdin.take().unwrap();
    let mut stdout = process.stdout.take().unwrap();
    // Plain mode: one JSON line each way.
    let hello = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "hello",
        "params": { "generation": 1, "framed": true },
    });
    writeln!(stdin, "{hello}").map_err(|e| e.to_string())?;
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    while stdout.read_exact(&mut b).is_ok() && b[0] != b'\n' {
        line.push(b[0]);
    }
    let reply: serde_json::Value =
        serde_json::from_slice(&line).map_err(|_| "herdr-core didn't answer".to_string())?;
    if reply["result"]["framed"] != true {
        return Err(format!("herdr-core: {}", reply["error"]));
    }
    Ok((stdin, stdout, process))
}

fn attach(
    stdin: &mut ChildStdin,
    stdout: &mut ChildStdout,
    address: &str,
    (cols, rows): (u16, u16),
    history: usize,
) -> Result<(u32, serde_json::Value), String> {
    let r = call(
        stdin,
        stdout,
        "terminal.attach",
        serde_json::json!({ "terminal": address, "cols": cols, "rows": rows, "history": history }),
    )?;
    let channel = r["channel"].as_u64().ok_or("terminal.attach: no channel")? as u32;
    if r["terminal"]["state"] != "live" {
        return Err(format!("{address} isn't running"));
    }
    Ok((channel, r["terminal"].clone()))
}

/// One call on channel 0, before the stream starts (nothing else arrives yet).
fn call(
    stdin: &mut ChildStdin,
    stdout: &mut ChildStdout,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let msg = serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": method, "params": params });
    frame(stdin, 0, DATA, msg.to_string().as_bytes()).map_err(|e| e.to_string())?;
    loop {
        let (ch, kind, payload) = read_frame(stdout).map_err(|e| e.to_string())?;
        if ch != 0 || kind != DATA {
            continue;
        }
        let mut reply: serde_json::Value =
            serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
        if reply["id"] != 2 {
            continue;
        }
        if !reply["error"].is_null() {
            return Err(format!("{method}: {}", reply["error"]["message"]));
        }
        return Ok(reply["result"].take());
    }
}

fn frame(w: &mut impl Write, channel: u32, kind: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(9 + payload.len());
    buf.extend(channel.to_le_bytes());
    buf.push(kind);
    buf.extend((payload.len() as u32).to_le_bytes());
    buf.extend(payload);
    w.write_all(&buf)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> std::io::Result<(u32, u8, Vec<u8>)> {
    let mut head = [0u8; 9];
    r.read_exact(&mut head)?;
    let channel = u32::from_le_bytes(head[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[5..9].try_into().unwrap()) as usize;
    let mut payload = vec![0; len];
    r.read_exact(&mut payload)?;
    Ok((channel, head[4], payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        frame(&mut buf, 7, RESIZE, &[1, 2, 3, 4]).unwrap();
        assert_eq!(buf[..9], [7, 0, 0, 0, RESIZE, 4, 0, 0, 0]);
        assert_eq!(
            read_frame(&mut buf.as_slice()).unwrap(),
            (7, RESIZE, vec![1, 2, 3, 4])
        );
    }

    /// Against a real `hc`, in a core of its own: a shell outlives its
    /// viewer, comes back with its screen, and ends when killed.
    /// `cargo test hc::tests::real -- --ignored`
    #[test]
    #[ignore = "needs herdr-core installed"]
    fn real_core_keeps_the_shell() {
        use std::sync::mpsc;
        let session = format!("abromux-test-{}", std::process::id());
        // SAFETY: the only test that touches it, and it runs alone (--ignored).
        unsafe { std::env::set_var("HC_SESSION", &session) };
        let sink = |tx: mpsc::Sender<Output>| move |o| tx.send(o).is_ok();
        let until = |rx: &mpsc::Receiver<Output>, want: &str| {
            let mut seen = Vec::new();
            let end = std::time::Instant::now() + Duration::from_secs(10);
            while let Ok(o) =
                rx.recv_timeout(end.saturating_duration_since(std::time::Instant::now()))
            {
                match o {
                    Output::Bytes(b) => seen.extend(b),
                    Output::Closed => return (seen, true),
                }
                if String::from_utf8_lossy(&seen).contains(want) {
                    return (seen, false);
                }
            }
            panic!("never saw {want:?}: {}", String::from_utf8_lossy(&seen));
        };

        let (tx, rx) = mpsc::channel();
        let env = BTreeMap::from([("ABROMUX_PROBE".to_string(), "kept".to_string())]);
        let argv = ["/bin/sh".to_string()];
        let t = Terminal::spawn("/tmp", &argv, &env, (80, 24), sink(tx)).unwrap();
        assert!(t.pid.is_some());
        t.write(b"echo \"$ABROMUX_PROBE\"-$((40+2))\n");
        until(&rx, "kept-42");
        let address = t.address.clone();
        drop(t); // quitting abromux: only detaches

        let (tx, rx) = mpsc::channel();
        let t = Terminal::attach(&address, (100, 30), sink(tx)).unwrap();
        until(&rx, "kept-42"); // the first paint has the old screen
        t.write(b"echo again-$((1+1))\n");
        until(&rx, "again-2");
        t.kill();
        let (_, closed) = until(&rx, "\u{0}never");
        assert!(closed);
        std::thread::sleep(Duration::from_millis(500));
        assert!(Terminal::attach(&address, (80, 24), |_| true).is_err());

        let _ = Command::new(binary().unwrap())
            .args(["server", "stop"])
            .env("HC_SESSION", &session)
            .output();
        let state = std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join(".local/state"));
        let _ = std::fs::remove_dir_all(state.join("herdr-core").join(&session));
    }
}
