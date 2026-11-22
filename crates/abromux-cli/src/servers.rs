//! Servers: other machines' abromux servers this app knows by name
//! (docs/server-plan.md, phase 3), for remote workspaces and tabs.
//!
//! They live in `servers.json` next to `config.json`, not in it: the
//! release and nightly apps share the config file, and an app that doesn't
//! know a key refuses the whole file. Settings › Servers, the palette's
//! "New Workspace on …" / "New Tab on …", and `abromux servers` manage them.
//!
//! A workspace or tab keeps the server's address, not its name, so renaming
//! or removing a server here doesn't strand what runs there.

use abromux_core::dial::Endpoint;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Server {
    pub name: String,
    /// `ssh://[user@]host[:port]` (or a Unix socket path).
    pub address: String,
    /// Other addresses of the same machine, tried in order when `address`
    /// doesn't answer (its LAN name, then its Tailscale name, say).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<String>,
    /// The machine's id as its server first reported it (`hello`), to
    /// notice when an address leads somewhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    servers: Vec<Server>,
}

pub fn path() -> PathBuf {
    abromux_core::paths::xdg_config().join("abromux/servers.json")
}

/// The servers, in the order they were added. A file that can't be read is
/// no servers (and [`edit`] won't overwrite it). Read again when changed
/// here, else at most every 2 s: the sidebar asks on every draw.
pub fn load() -> Vec<Server> {
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, u64, Vec<Server>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    let now = changes();
    if let Some((at, seen, list)) = &*c
        && *seen == now
        && at.elapsed() < Duration::from_secs(2)
    {
        return list.clone();
    }
    let list = read().unwrap_or_default();
    *c = Some((Instant::now(), now, list.clone()));
    list
}

fn read() -> Result<Vec<Server>, String> {
    match std::fs::read_to_string(path()) {
        Ok(text) => serde_json::from_str::<File>(&text)
            .map(|f| f.servers)
            .map_err(|e| format!("{}: {e}", path().display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(format!("{}: {e}", path().display())),
    }
}

/// Change the list and write it back.
fn edit(f: impl FnOnce(&mut Vec<Server>) -> Result<(), String>) -> Result<(), String> {
    let mut list = read()?;
    f(&mut list)?;
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&File { servers: list }).unwrap() + "\n";
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, &p))
        .map_err(|e| format!("{}: {e}", p.display()))?;
    CHANGES.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// `box`, `me@box:2222` or `ssh://me@box` as an address: `ssh://` unless
/// it's a socket.
pub fn normalize(address: &str) -> Result<String, String> {
    let a = address.trim();
    Endpoint::parse(a)?;
    Ok(
        if ["ssh://", "unix://", "tailnet://", "tcp://"]
            .iter()
            .any(|p| a.starts_with(p))
            || a.starts_with('/')
        {
            a.to_string()
        } else {
            format!("ssh://{a}")
        },
    )
}

/// Add a server. The name is how the palette and `--host` call it.
/// `address` may list more than one, separated by commas or spaces: the
/// first is its address, the others are tried after it.
pub fn add(name: &str, address: &str) -> Result<Server, String> {
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("local") {
        return Err(format!("{name:?} can't be a server's name"));
    }
    let mut all = address
        .split([',', ' '])
        .filter(|a| !a.trim().is_empty())
        .map(normalize)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();
    let server = Server {
        name: name.to_string(),
        address: all.next().ok_or("no address")?,
        also: all.collect(),
        host_id: None,
    };
    let s = server.clone();
    edit(move |list| {
        if list.iter().any(|x| x.name.eq_ignore_ascii_case(&s.name)) {
            return Err(format!("there's already a server called {}", s.name));
        }
        list.push(s);
        Ok(())
    })?;
    Ok(server)
}

pub fn remove(name: &str) -> Result<(), String> {
    edit(|list| {
        let before = list.len();
        list.retain(|x| !x.name.eq_ignore_ascii_case(name.trim()));
        if list.len() == before {
            return Err(format!("no server called {name}"));
        }
        Ok(())
    })
}

/// A server's name or an address, as an address: what `--host` takes.
/// `local` is this machine (None).
pub fn resolve(name_or_address: &str) -> Result<Option<String>, String> {
    let s = name_or_address.trim();
    if s.eq_ignore_ascii_case("local") {
        return Ok(None);
    }
    if let Some(x) = load().into_iter().find(|x| x.name.eq_ignore_ascii_case(s)) {
        return Ok(Some(x.address));
    }
    normalize(s)
        .map(Some)
        .map_err(|e| format!("{e}; or one of the servers: {}", names().join(", ")))
}

fn names() -> Vec<String> {
    let n: Vec<String> = load().into_iter().map(|s| s.name).collect();
    if n.is_empty() {
        vec!["(none yet)".into()]
    } else {
        n
    }
}

/// The other addresses of the server whose address is `host`
/// (abromux_term::remote tries them after it).
pub fn alternates(host: &str) -> Vec<String> {
    let norm = normalize(host).unwrap_or_else(|_| host.to_string());
    load()
        .into_iter()
        .find(|x| x.address == norm)
        .map(|x| x.also)
        .unwrap_or_default()
}

/// What to call the server at `address`: its name here, else its host.
pub fn label(address: &str) -> String {
    let norm = normalize(address).unwrap_or_else(|_| address.to_string());
    load()
        .into_iter()
        .find(|x| x.address == norm)
        .map(|x| x.name)
        .unwrap_or_else(|| Endpoint::parse(address).map_or_else(|_| address.into(), |e| e.label()))
}

/// The workspaces the server at `host` keeps for this app: the session
/// the app last sent it (crate::control_link). Blocks on the network.
pub fn fetch_workspaces(host: &str) -> Result<Vec<abromux_core::session::WorkspaceState>, String> {
    use std::io::BufRead;
    let first = serde_json::json!({ "op": "get-state" });
    let (s, hello) = abromux_core::dial::open_stream(host, Some(&first))?;
    if hello["protocol"].as_u64().unwrap_or(1) < abromux_core::wire::CONTROL {
        return Err("the server there is too old to keep workspaces; update it".into());
    }
    s.set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .ok();
    let mut line = String::new();
    std::io::BufReader::new(s)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    Ok(serde_json::from_value(v["session"]["workspaces"].clone()).unwrap_or_default())
}

// ---- reachability ----------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Status {
    Checking,
    /// The server's answer: its machine name and version.
    Up(String),
    Down(String),
    /// It answered, but as another machine than the one first seen there.
    Moved(String),
}

static STATUS: Mutex<Option<HashMap<String, Status>>> = Mutex::new(None);
static CHANGES: AtomicU64 = AtomicU64::new(0);

/// Bumped whenever the list or a status changes, for views to redraw.
pub fn changes() -> u64 {
    CHANGES.load(Ordering::Relaxed)
}

pub fn status(address: &str) -> Option<Status> {
    STATUS.lock().unwrap().as_ref()?.get(address).cloned()
}

fn set_status(address: &str, s: Status) {
    STATUS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(address.to_string(), s);
    CHANGES.fetch_add(1, Ordering::Relaxed);
}

/// Ask the server at `address` whether it's there, on a thread; the answer
/// lands in [`status`].
pub fn check(address: &str) {
    if matches!(status(address), Some(Status::Checking)) {
        return;
    }
    set_status(address, Status::Checking);
    let address = address.to_string();
    std::thread::spawn(move || {
        let s = match check_now(&address) {
            Ok(up) if up.starts_with(MOVED) => Status::Moved(up),
            Ok(up) => Status::Up(up),
            Err(e) => Status::Down(e),
        };
        set_status(&address, s);
    });
}

const MOVED: &str = "a different machine";

/// The same, blocking: "hostname, version 0.1.0" or why not. The first
/// answer's machine id is kept; a later one from another machine says so.
pub fn check_now(address: &str) -> Result<String, String> {
    // Its other addresses too, as terminals and links try them.
    let (_conn, v) = abromux_core::dial::open_stream(address, None)?;
    let host = v["host"].as_str().unwrap_or("?");
    let version = v["version"].as_str().unwrap_or("?");
    let about = format!("{host}, version {version}");
    let Some(id) = v["host_id"].as_str() else {
        return Ok(about);
    };
    let norm = normalize(address).unwrap_or_else(|_| address.to_string());
    let known = load().into_iter().find(|s| s.address == norm);
    match known.as_ref().and_then(|s| s.host_id.as_deref()) {
        Some(k) if k != id => Ok(format!(
            "{MOVED} answers there now ({about}); remove and add the server if that's right"
        )),
        Some(_) => Ok(about),
        None => {
            if known.is_some() {
                let id = id.to_string();
                let _ = edit(|list| {
                    if let Some(s) = list.iter_mut().find(|s| s.address == norm) {
                        s.host_id = Some(id);
                    }
                    Ok(())
                });
            }
            Ok(about)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(normalize("box").unwrap(), "ssh://box");
        assert_eq!(normalize(" me@box:2222 ").unwrap(), "ssh://me@box:2222");
        assert_eq!(normalize("ssh://box").unwrap(), "ssh://box");
        assert_eq!(normalize("/tmp/s.sock").unwrap(), "/tmp/s.sock");
        assert!(normalize("http://box").is_err());
        assert!(normalize("").is_err());
    }
}
