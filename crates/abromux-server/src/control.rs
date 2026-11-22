//! The control socket (`abromux.sock`) in the server (docs/server-plan.md,
//! phase 2).
//!
//! The server owns the socket the `abromux` CLI, agent hooks and the phone
//! app talk to, so it's there whether or not the app is:
//!
//! - **The app attached** (its link, `{"op":"app"}`): each control
//!   connection is handed to the app, which opens a connection of its own to
//!   the server for it (`{"op":"control","conn":N}`), and the server splices
//!   the two byte for byte. The app answers with its own handlers, exactly
//!   as when it owned the socket.
//! - **The app away**: the server answers what it can from [`State`], what
//!   the app last told it about its workspaces. Calls that change a
//!   workspace's state (status pills, progress, the log, notifications and
//!   agents' reports) are kept and replayed to the app when it attaches, in
//!   order, so it comes back to what happened meanwhile. Everything else is
//!   refused with the code `app_closed`.
//!
//! The state and the kept calls are saved next to the socket
//! (`server-state.json`), so a server that stops (idle, a reboot) doesn't
//! lose them.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Calls kept while the app is away and replayed when it's back. They
/// change a workspace's state and need nothing of the app to answer.
pub const RECORDED: &[&str] = &[
    "status.set",
    "status.clear",
    "progress.set",
    "progress.clear",
    "log.add",
    "log.clear",
    "notification.create",
    "agent.report",
];

/// Calls the server answers from [`State`] while the app is away.
pub const ANSWERED: &[&str] = &[
    "system.ping",
    "workspace.list",
    "workspace.current",
    "status.list",
    "log.list",
    "notification.list",
];

/// The most calls kept; past it the oldest go.
const MAX_KEPT: usize = 10_000;

#[derive(Default)]
pub struct State {
    /// What the app last said about its workspaces: `workspace.list`'s
    /// answer, with each one's `surfaces` too.
    pub workspaces: Vec<Value>,
    /// The app's session as it last saved it (layouts, held ids, hosts).
    pub session: Value,
    /// Calls kept for the app: `{"method","params","at"}`.
    pub kept: Vec<Value>,
    path: Option<PathBuf>,
}

impl State {
    /// The state saved at `path`, if any.
    pub fn load(path: PathBuf) -> Self {
        let mut s = State {
            path: Some(path.clone()),
            ..Default::default()
        };
        if let Ok(v) = std::fs::read(&path).and_then(|b| Ok(serde_json::from_slice::<Value>(&b)?)) {
            s.workspaces = v["workspaces"].as_array().cloned().unwrap_or_default();
            s.session = v["session"].clone();
            s.kept = v["kept"].as_array().cloned().unwrap_or_default();
        }
        s
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        let v =
            json!({ "workspaces": self.workspaces, "session": self.session, "kept": self.kept });
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            let _ = std::fs::rename(tmp, path);
        }
    }

    /// The app's `{"op":"state"}`. A workspace the app no longer mentions
    /// is kept in the session while any of its terminals still runs here
    /// (`alive`): the app may have lost it (its session file, another
    /// Mac), and `get-state` is how it gets it back. One closed in the app
    /// has its terminals ended, so it goes.
    pub fn update(&mut self, msg: &Value, alive: &dyn Fn(&str) -> bool) {
        if let Some(w) = msg["workspaces"].as_array() {
            self.workspaces = w.clone();
        }
        if !msg["session"].is_null() {
            let mut session = msg["session"].clone();
            let new_ids: Vec<String> = session["workspaces"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(held_ids)
                .collect();
            let orphans: Vec<Value> = self.session["workspaces"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|w| {
                    let ids = held_ids(w);
                    ids.iter().any(|id| alive(id)) && ids.iter().all(|id| !new_ids.contains(id))
                })
                .cloned()
                .collect();
            if let Some(list) = session["workspaces"].as_array_mut() {
                list.extend(orphans);
            }
            self.session = session;
        }
        self.save();
    }

    /// Keep `method` for the app as if it had been called: the server's
    /// own reports (a notification a terminal printed while nobody
    /// watched it).
    pub fn keep(&mut self, method: &str, params: Value) {
        let ws = self.target(&params);
        self.apply(method, &params, ws.as_deref());
        self.kept
            .push(json!({ "method": method, "params": params, "at": now() }));
        self.save();
    }

    /// The calls to replay, as request lines, and forget them.
    pub fn take_kept(&mut self) -> Vec<Value> {
        let kept = std::mem::take(&mut self.kept);
        self.save();
        kept
    }

    /// Answer one control request while the app is away.
    pub fn handle(&mut self, req: &Value) -> Value {
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req["method"].as_str().unwrap_or_default();
        let p = req.get("params").cloned().unwrap_or_else(|| json!({}));
        match self.answer(method, &p) {
            Ok(result) => json!({ "id": id, "ok": true, "result": result }),
            Err((error, code)) => json!({ "id": id, "ok": false, "error": error, "code": code }),
        }
    }

    fn answer(&mut self, method: &str, p: &Value) -> Result<Value, (String, &'static str)> {
        if RECORDED.contains(&method) {
            let ws = self.target(p);
            self.apply(method, p, ws.as_deref());
            self.kept
                .push(json!({ "method": method, "params": p, "at": now() }));
            if self.kept.len() > MAX_KEPT {
                let cut = self.kept.len() - MAX_KEPT;
                self.kept.drain(..cut);
            }
            self.save();
            return Ok(match method {
                "status.clear" => json!({ "cleared": true, "workspace": ws, "queued": true }),
                _ => json!({ "workspace": ws, "queued": true }),
            });
        }
        match method {
            "system.ping" => Ok(json!("pong")),
            "workspace.list" => Ok(json!(self.workspaces)),
            "workspace.current" => {
                let r = self.target(p);
                self.ws(r.as_deref())
                    .cloned()
                    .ok_or_else(|| ("no workspaces".to_string(), "target"))
            }
            "status.list" => {
                let r = self.target(p);
                Ok(self
                    .ws(r.as_deref())
                    .map_or(json!([]), |w| w["status"].clone()))
            }
            "log.list" => {
                let r = self.target(p);
                Ok(self
                    .ws(r.as_deref())
                    .map_or(json!([]), |w| w["log"].clone()))
            }
            "notification.list" => Ok(json!(
                self.kept
                    .iter()
                    .filter(|k| k["method"] == "notification.create")
                    .enumerate()
                    .map(|(i, k)| json!({
                        "id": i + 1,
                        "title": k["params"]["title"],
                        "subtitle": k["params"]["subtitle"],
                        "body": k["params"]["body"],
                        "surface": k["params"]["caller"]["surface"],
                        "workspace": self.target(&k["params"]),
                        "age_seconds": now().saturating_sub(k["at"].as_u64().unwrap_or(0)),
                        "unread": true,
                    }))
                    .collect::<Vec<_>>()
            )),
            _ => Err((
                format!(
                    "the abromux app isn't open; until it is, the server keeps {} and answers {}",
                    RECORDED.join(", "),
                    ANSWERED.join(", ")
                ),
                "app_closed",
            )),
        }
    }

    /// The workspace a call is about: `workspace` (a ref, an index or a
    /// name), else the caller's, else the selected one.
    fn target(&self, p: &Value) -> Option<String> {
        let refs = |w: &Value| w["ref"].as_str().map(String::from);
        if let Some(t) = p.get("workspace").filter(|t| !t.is_null()) {
            let t = match t {
                Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            return self
                .workspaces
                .iter()
                .find(|w| {
                    w["ref"].as_str() == Some(&t)
                        || w["index"].as_u64().is_some_and(|i| i.to_string() == t)
                        || w["name"].as_str() == Some(&t)
                        || w["ref"].as_str().and_then(|r| r.strip_prefix("workspace:")) == Some(&t)
                })
                .and_then(refs)
                .or(Some(t));
        }
        let caller = p["caller"]["surface"]
            .as_str()
            .or_else(|| p["surface"].as_str());
        if let Some(s) = caller
            && let Some(w) = self.workspaces.iter().find(|w| {
                w["surfaces"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(s)))
            })
        {
            return refs(w);
        }
        self.workspaces
            .iter()
            .find(|w| w["selected"] == true)
            .or(self.workspaces.first())
            .and_then(refs)
    }

    fn ws(&self, r: Option<&str>) -> Option<&Value> {
        let r = r?;
        self.workspaces
            .iter()
            .find(|w| w["ref"].as_str() == Some(r))
    }

    /// Show a kept call in the state the server answers from, as the app
    /// would once it replays it.
    fn apply(&mut self, method: &str, p: &Value, ws: Option<&str>) {
        let Some(w) = ws.and_then(|r| {
            self.workspaces
                .iter_mut()
                .find(|w| w["ref"].as_str() == Some(r))
        }) else {
            return;
        };
        match method {
            "status.set" => {
                let key = p["key"].clone();
                let list = w["status"].as_array().cloned().unwrap_or_default();
                let mut list: Vec<Value> = list.into_iter().filter(|s| s["key"] != key).collect();
                list.push(json!({ "key": key, "value": p["value"], "icon": p["icon"], "color": p["color"] }));
                w["status"] = json!(list);
            }
            "status.clear" => {
                let key = p["key"].clone();
                if let Some(list) = w["status"].as_array() {
                    let list: Vec<Value> =
                        list.iter().filter(|s| s["key"] != key).cloned().collect();
                    w["status"] = json!(list);
                }
            }
            "progress.set" => {
                w["progress"] = json!({ "value": p["value"], "label": p["label"] });
            }
            "progress.clear" => w["progress"] = Value::Null,
            "log.add" => {
                let mut log = w["log"].as_array().cloned().unwrap_or_default();
                log.push(json!({ "message": p["message"], "level": p["level"], "source": p["source"], "age_seconds": 0 }));
                w["log"] = json!(log);
            }
            "log.clear" => w["log"] = json!([]),
            _ => {}
        }
    }
}

/// The server ids of the terminals a saved workspace keeps (`held`).
fn held_ids(ws: &Value) -> Vec<String> {
    fn walk(n: &Value, out: &mut Vec<String>) {
        for t in n["tabs"].as_array().into_iter().flatten() {
            if let Some(h) = t["held"].as_str() {
                out.push(h.to_string());
            }
        }
        for c in n["children"].as_array().into_iter().flatten() {
            walk(c, out);
        }
    }
    let mut out = vec![];
    walk(&ws["root"], &mut out);
    for p in ws["pinned"].as_array().into_iter().flatten() {
        if let Some(h) = p["hidden"]["held"].as_str() {
            out.push(h.to_string());
        }
    }
    out
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Where the state is saved: beside the server's socket.
pub fn state_path(socket: &Path) -> PathBuf {
    socket.with_file_name("server-state.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        let mut s = State::default();
        s.update(&json!({ "workspaces": [
            { "ref": "workspace:1", "index": 0, "name": "Home", "selected": true, "surfaces": ["surface:1"], "status": [] },
            { "ref": "workspace:4", "index": 1, "name": "VPS", "selected": false, "surfaces": ["surface:7", "surface:8"], "status": [] },
        ]}), &|_| false);
        s
    }

    fn call(s: &mut State, method: &str, params: Value) -> Value {
        s.handle(&json!({ "id": 1, "method": method, "params": params }))
    }

    #[test]
    fn keeps_what_changes_a_workspace_and_shows_it() {
        let mut s = state();
        let r = call(
            &mut s,
            "status.set",
            json!({ "key": "build", "value": "ok", "caller": { "surface": "surface:8" } }),
        );
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["result"]["workspace"], "workspace:4");
        let r = call(&mut s, "status.list", json!({ "workspace": "workspace:4" }));
        assert_eq!(r["result"][0]["value"], "ok");
        call(
            &mut s,
            "notification.create",
            json!({ "title": "Claude", "body": "done", "caller": { "surface": "surface:7" } }),
        );
        assert_eq!(
            call(&mut s, "notification.list", json!({}))["result"][0]["body"],
            "done"
        );
        assert_eq!(s.kept.len(), 2);
        let kept = s.take_kept();
        assert_eq!(kept[0]["method"], "status.set");
        assert!(s.kept.is_empty());
    }

    #[test]
    fn answers_lists_and_refuses_the_rest() {
        let mut s = state();
        let r = call(&mut s, "workspace.list", json!({}));
        assert_eq!(r["result"][1]["name"], "VPS");
        assert_eq!(
            call(&mut s, "workspace.current", json!({}))["result"]["ref"],
            "workspace:1"
        );
        let r = call(&mut s, "window.focus", json!({}));
        assert_eq!(r["ok"], false);
        assert_eq!(r["code"], "app_closed");
    }

    /// A workspace the app forgets stays while its terminals run here.
    #[test]
    fn keeps_workspaces_the_app_lost_while_they_run() {
        let ws = |name: &str, held: &str| json!({ "name": name, "root": { "kind": "pane", "tabs": [{ "kind": "terminal", "cwd": "~", "held": held }] } });
        let mut s = State::default();
        s.update(
            &json!({ "session": { "workspaces": [ws("a", "1-1"), ws("b", "1-2")] } }),
            &|_| true,
        );
        // The app now only has a; b's terminal still runs, c's is gone.
        s.update(
            &json!({ "session": { "workspaces": [ws("a", "1-1")] } }),
            &|id| id == "1-2",
        );
        let names: Vec<_> = s.session["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["name"].clone())
            .collect();
        assert_eq!(names, [json!("a"), json!("b")]);
        // Once its terminal ends, it goes.
        s.update(
            &json!({ "session": { "workspaces": [ws("a", "1-1")] } }),
            &|_| false,
        );
        assert_eq!(s.session["workspaces"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn survives_a_restart() {
        let path = std::env::temp_dir().join(format!("abx-state-{}.json", std::process::id()));
        let mut s = State::load(path.clone());
        s.update(
            &json!({ "workspaces": [{ "ref": "workspace:1", "selected": true, "surfaces": [] }] }),
            &|_| false,
        );
        call(&mut s, "log.add", json!({ "message": "hi" }));
        let s2 = State::load(path.clone());
        assert_eq!(s2.kept.len(), 1);
        assert_eq!(s2.workspaces.len(), 1);
        let _ = std::fs::remove_file(path);
    }
}
