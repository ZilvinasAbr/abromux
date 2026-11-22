//! The model methods the server answers itself, whether or not the app is
//! attached (docs/server-plan.md, phase 2). The server is their source:
//!
//! - **Reads** ([`READS`]): answered from its model, in the app's shapes.
//! - **Changes** ([`OWNED`]): a workspace's status pills, progress, log,
//!   name, description and colour, the selection, the order, and a tab's
//!   name. The server changes its model, stamps the change, answers the
//!   caller, and hands the app the call to mirror in its views
//!   (`{"mirror":..}`), or keeps it for when the app is back.
//!
//! What needs the app's views to happen at all (a split, a tab, a terminal,
//! a browser) is the app's: it does it and proposes the change.

use abromux_model::{Event, LogEntry, Model, Progress, Status, Workspace};
use serde_json::{Value, json};

/// Whether control method `method` is about the workspaces' model (true) or
/// needs only the app's views (browser automation, input, focus, reading a
/// screen, questions to a person). The app's `api::kind` is this.
pub fn is_model_method(method: &str) -> bool {
    let (ns, verb) = method.split_once('.').unwrap_or((method, ""));
    match ns {
        "workspace" | "group" | "status" | "progress" | "log" | "notification" | "link"
        | "forward" | "agent" | "tab" | "system" | "session" => true,
        "pane" => verb != "zoom",
        "surface" => matches!(
            verb,
            "list"
                | "split"
                | "create"
                | "close"
                | "rename"
                | "respawn"
                | "move"
                | "reorder"
                | "split_off"
                | "move_to_new_workspace"
                | "clear_history"
                | "reclaim"
                | "restream"
        ),
        _ => false,
    }
}

/// Model methods the server has the app carry out (they make or close
/// views), committing the change to its model before the caller hears back.
/// Not: what streams or waits on a person, and what the app only reads.
pub fn executed(method: &str) -> bool {
    let verb = method.split_once('.').map_or("", |(_, v)| v);
    is_model_method(method)
        // Reads change nothing: straight to the app.
        && !matches!(verb, "list" | "current" | "get" | "tree")
        && !OWNED.contains(&method)
        && !READS.contains(&method)
        && !matches!(method, "system.ping" | "system.identify" | "system.methods")
        && !method.starts_with("model.")
        && !matches!(
            method,
            "surface.reclaim" | "surface.restream" | "forward.offer"
        )
}

/// The methods the server changes its model for.
pub const OWNED: &[&str] = &[
    "status.set",
    "status.clear",
    "progress.set",
    "progress.clear",
    "log.add",
    "log.clear",
    "workspace.rename",
    "workspace.select",
    "workspace.reorder",
    "workspace.action",
    "surface.rename",
];

/// The methods the server answers from its model.
pub const READS: &[&str] = &[
    "workspace.list",
    "workspace.current",
    "status.list",
    "log.list",
];

/// What a call did.
pub enum Outcome {
    /// A read's answer.
    Read(Value),
    /// A change: its result, its events (unstamped), and the call as the
    /// app mirrors it (its target made explicit).
    Changed {
        result: Value,
        events: Vec<Event>,
        mirror: Value,
    },
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A workspace as `workspace.list` describes it.
pub fn describe(m: &Model, i: usize) -> Value {
    let w = &m.workspaces[i];
    let panes = fn_panes(&w.layout);
    json!({
        "ref": format!("workspace:{}", w.id),
        "index": i,
        "name": w.name,
        "selected": m.selected == Some(w.id),
        "unread": w.unread,
        "cwd": w.cwd,
        "branch": w.branch,
        "pane_count": panes,
        "description": w.description,
        "color": w.color,
        "group": w.group,
        "home": w.folder,
        "host": w.host,
        "status": w.status.iter().map(status_json).collect::<Vec<_>>(),
        "progress": w.progress.as_ref().map(|p| json!({ "value": (p.value * 1000.).round() / 1000., "label": p.label })),
    })
}

fn fn_panes(n: &abromux_model::Node) -> usize {
    match n {
        abromux_model::Node::Pane { .. } => 1,
        abromux_model::Node::Split { children, .. } => {
            fn_panes(&children[0]) + fn_panes(&children[1])
        }
    }
}

fn status_json(s: &Status) -> Value {
    json!({ "key": s.key, "value": s.value, "icon": s.icon, "color": s.color })
}

/// Run `method` on `model` (changes are unstamped: the caller stamps).
/// None when it isn't the server's to answer after all (an action that
/// needs the app, a workspace or tab the model doesn't have).
pub fn call(model: &mut Model, method: &str, p: &Value) -> Result<Option<Outcome>, String> {
    if model.workspaces.is_empty() {
        return Ok(None);
    }
    let str_param = |k: &str| p[k].as_str().map(String::from);
    // A tab's name: by its surface.
    if method == "surface.rename" {
        let Some(title) = str_param("title").filter(|t| !t.is_empty()) else {
            return Ok(None); // back to the program's own title: the app knows it
        };
        let surface = p["surface"]
            .as_str()
            .or(p["caller"]["surface"].as_str())
            .and_then(|s| s.trim_start_matches("surface:").parse::<u64>().ok());
        let Some(surface) = surface else {
            return Ok(None);
        };
        let Some((i, ws)) = model
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, w)| w.layout.tabs().iter().any(|t| t.surface == surface))
        else {
            return Ok(None);
        };
        let _ = ws;
        rename_tab(&mut model.workspaces[i].layout, surface, &title);
        let ws = model.workspaces[i].clone();
        let r = format!("surface:{surface}");
        return Ok(Some(Outcome::Changed {
            result: json!({ "ref": r, "title": title }),
            events: vec![Event::Workspace {
                index: i,
                workspace: Box::new(ws),
            }],
            mirror: json!({ "method": method, "params": { "surface": r, "title": title } }),
        }));
    }
    let Some(id) = model.target(p) else {
        return Ok(None);
    };
    let i = model.workspaces.iter().position(|w| w.id == id).unwrap();
    let r = format!("workspace:{id}");
    match method {
        "workspace.list" => {
            return Ok(Some(Outcome::Read(json!(
                (0..model.workspaces.len())
                    .map(|i| describe(model, i))
                    .collect::<Vec<_>>()
            ))));
        }
        "workspace.current" => return Ok(Some(Outcome::Read(describe(model, i)))),
        "status.list" => {
            return Ok(Some(Outcome::Read(json!(
                model.workspaces[i]
                    .status
                    .iter()
                    .map(status_json)
                    .collect::<Vec<_>>()
            ))));
        }
        "log.list" => {
            let limit = match &p["limit"] {
                Value::String(n) => n.parse().map_err(|_| format!("bad limit {n:?}"))?,
                Value::Null => usize::MAX,
                n => n.as_u64().ok_or("limit must be a number")? as usize,
            };
            let log = &model.workspaces[i].log;
            let t = now();
            return Ok(Some(Outcome::Read(json!(
                log[log.len().saturating_sub(limit)..]
                    .iter()
                    .map(|l| json!({
                        "level": l.level,
                        "source": l.source,
                        "message": l.message,
                        "age_seconds": t.saturating_sub(l.at),
                    }))
                    .collect::<Vec<_>>()
            ))));
        }
        "workspace.select" => {
            if p.get("workspace").is_none_or(Value::is_null) {
                return Ok(None);
            }
            model.selected = Some(id);
            return Ok(Some(Outcome::Changed {
                result: describe(model, i),
                events: vec![Event::Selected { id: Some(id) }],
                mirror: json!({ "method": method, "params": { "workspace": r } }),
            }));
        }
        "workspace.reorder" => {
            if p["dry_run"] == true {
                return Ok(None);
            }
            let n = model.workspaces.len();
            let pos_of = |rf: &str, m: &Model| {
                let id = m.target(&json!({ "workspace": rf }))?;
                m.workspaces.iter().position(|w| w.id == id)
            };
            let to = if let Some(x) = p.get("index").filter(|v| !v.is_null()) {
                match x {
                    Value::String(s) => s.parse::<usize>().map_err(|_| "bad --index")?,
                    v => v.as_u64().ok_or("bad --index")? as usize,
                }
            } else if let Some(b) = p["before"].as_str() {
                let at = pos_of(b, model).ok_or("no such workspace for --before")?;
                if at > i { at - 1 } else { at }
            } else if let Some(a) = p["after"].as_str() {
                let at = pos_of(a, model).ok_or("no such workspace for --after")?;
                if at >= i { at } else { at + 1 }
            } else {
                return Err("give --index, --before or --after".into());
            };
            let to = to.min(n - 1);
            let ws = model.workspaces.remove(i);
            model.workspaces.insert(to, ws.clone());
            return Ok(Some(Outcome::Changed {
                result: json!({ "workspace": r, "index": to }),
                events: vec![Event::Workspace {
                    index: to,
                    workspace: Box::new(ws),
                }],
                mirror: json!({ "method": method, "params": { "workspace": r, "index": to } }),
            }));
        }
        _ => {}
    }
    let ws: &mut Workspace = &mut model.workspaces[i];
    let result = match method {
        "status.set" => {
            let key = str_param("key").ok_or("missing <key>")?;
            let value = str_param("value").ok_or("missing <value>")?;
            let s = Status {
                key: key.clone(),
                value,
                color: str_param("color"),
                icon: str_param("icon"),
            };
            match ws.status.iter_mut().find(|s| s.key == key) {
                Some(old) => *old = s,
                None => ws.status.push(s),
            }
            json!({ "workspace": r })
        }
        "status.clear" => {
            let key = str_param("key").ok_or("missing <key>")?;
            let before = ws.status.len();
            ws.status.retain(|s| s.key != key);
            json!({ "cleared": ws.status.len() != before, "workspace": r })
        }
        "progress.set" => {
            let value = match &p["value"] {
                Value::String(v) => v
                    .parse::<f64>()
                    .map_err(|_| format!("bad progress {v:?}"))?,
                Value::Null => return Err("missing <value> (0.0 to 1.0)".into()),
                v => v.as_f64().ok_or("progress must be a number")?,
            };
            if !(0.0..=1.0).contains(&value) {
                return Err("progress must be between 0.0 and 1.0".into());
            }
            ws.progress = Some(Progress {
                value,
                label: str_param("label"),
            });
            json!({ "workspace": r })
        }
        "progress.clear" => {
            ws.progress = None;
            Value::Null
        }
        "log.add" => {
            let level = str_param("level").unwrap_or_else(|| "info".into());
            if !["info", "progress", "success", "warning", "error"].contains(&level.as_str()) {
                return Err(format!(
                    "bad level {level:?} (info, progress, success, warning or error)"
                ));
            }
            let message = str_param("message").ok_or("missing <message>")?;
            ws.log.push(LogEntry {
                level,
                source: str_param("source"),
                message,
                at: now(),
            });
            Value::Null
        }
        "log.clear" => {
            ws.log.clear();
            Value::Null
        }
        "workspace.rename" => {
            let title = str_param("title").ok_or("missing title")?;
            ws.name = title.clone();
            json!({ "ref": r, "name": title })
        }
        "workspace.action" => {
            match p["action"].as_str().unwrap_or_default() {
                "rename" => ws.name = str_param("title").ok_or("rename: missing title")?,
                "set-description" => {
                    ws.description = str_param("description").filter(|d| !d.trim().is_empty())
                }
                "clear-description" => ws.description = None,
                "set-color" => {
                    ws.color = Some(str_param("color").ok_or("set-color: missing color")?)
                }
                "clear-color" => ws.color = None,
                // The rest need the app (closing, moving between groups,
                // marking read, an automatic name).
                _ => return Ok(None),
            }
            json!({ "ref": r })
        }
        m => return Err(format!("{m} isn't the server's")),
    };
    let ws = ws.clone();
    let mut mirror = p.clone();
    if let Some(o) = mirror.as_object_mut() {
        o.insert("workspace".into(), json!(r));
        o.remove("caller");
    }
    Ok(Some(Outcome::Changed {
        result,
        events: vec![Event::Workspace {
            index: i,
            workspace: Box::new(ws),
        }],
        mirror: json!({ "method": method, "params": mirror }),
    }))
}

fn rename_tab(n: &mut abromux_model::Node, surface: u64, title: &str) {
    match n {
        abromux_model::Node::Pane { tabs, .. } => {
            for t in tabs.iter_mut().filter(|t| t.surface == surface) {
                t.title = title.to_string();
            }
        }
        abromux_model::Node::Split { children, .. } => {
            rename_tab(&mut children[0], surface, title);
            rename_tab(&mut children[1], surface, title);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use abromux_model::{Node, Tab};

    fn model() -> Model {
        let ws = |id: u64, name: &str, surface: u64| Workspace {
            id,
            name: name.into(),
            layout: Node::Pane {
                id,
                active: 0,
                tabs: vec![Tab {
                    surface,
                    kind: "terminal".into(),
                    ..Default::default()
                }],
            },
            ..Default::default()
        };
        Model {
            seq: 3,
            workspaces: vec![ws(1, "home", 1), ws(4, "vps", 7), ws(5, "x", 9)],
            selected: Some(1),
        }
    }

    fn change(m: &mut Model, method: &str, p: Value) -> (Value, Vec<Event>, Value) {
        match call(m, method, &p).unwrap().expect("the server's") {
            Outcome::Changed {
                result,
                events,
                mirror,
            } => (result, events, mirror),
            Outcome::Read(_) => panic!("a change"),
        }
    }

    fn read(m: &mut Model, method: &str, p: Value) -> Value {
        match call(m, method, &p).unwrap().expect("the server's") {
            Outcome::Read(v) => v,
            Outcome::Changed { .. } => panic!("a read"),
        }
    }

    #[test]
    fn changes_say_what_to_mirror() {
        let mut m = model();
        let (r, ev, mirror) = change(
            &mut m,
            "status.set",
            json!({ "key": "build", "value": "ok", "caller": { "surface": "surface:7" } }),
        );
        assert_eq!(r["workspace"], "workspace:4");
        assert_eq!(ev.len(), 1);
        assert_eq!(m.workspace(4).unwrap().status[0].value, "ok");
        assert_eq!(mirror["params"]["workspace"], "workspace:4");
        assert!(mirror["params"].get("caller").is_none());

        change(
            &mut m,
            "progress.set",
            json!({ "value": "0.5", "workspace": "vps" }),
        );
        assert_eq!(
            m.workspace(4).unwrap().progress.as_ref().unwrap().value,
            0.5
        );
        assert!(call(&mut m, "progress.set", &json!({ "value": 2 })).is_err());
        change(
            &mut m,
            "log.add",
            json!({ "message": "deployed", "level": "success", "workspace": "workspace:4" }),
        );
        assert_eq!(
            read(&mut m, "log.list", json!({ "workspace": "workspace:4" }))[0]["message"],
            "deployed"
        );
        assert!(
            call(
                &mut m,
                "log.add",
                &json!({ "message": "x", "level": "loud" })
            )
            .is_err()
        );

        let (r, ..) = change(&mut m, "workspace.rename", json!({ "title": "Home!" }));
        assert_eq!(r["name"], "Home!");
        change(
            &mut m,
            "workspace.action",
            json!({ "action": "set-description", "description": "the VPS", "workspace": "workspace:4" }),
        );
        assert_eq!(
            m.workspace(4).unwrap().description.as_deref(),
            Some("the VPS")
        );
        assert!(
            call(
                &mut m,
                "workspace.action",
                &json!({ "action": "close-others" })
            )
            .unwrap()
            .is_none(),
            "the app's"
        );

        let (_, ev, mirror) = change(
            &mut m,
            "workspace.select",
            json!({ "workspace": "workspace:5" }),
        );
        assert_eq!(m.selected, Some(5));
        assert!(matches!(ev[0], Event::Selected { id: Some(5) }));
        assert_eq!(mirror["params"]["workspace"], "workspace:5");

        let (r, _, mirror) = change(
            &mut m,
            "workspace.reorder",
            json!({ "workspace": "workspace:5", "index": 0 }),
        );
        assert_eq!(r["index"], 0);
        assert_eq!(m.workspaces[0].id, 5);
        assert_eq!(mirror["params"]["index"], 0);
        change(
            &mut m,
            "workspace.reorder",
            json!({ "workspace": "workspace:5", "after": "workspace:4" }),
        );
        assert_eq!(
            m.workspaces.iter().map(|w| w.id).collect::<Vec<_>>(),
            [1, 4, 5]
        );

        let (r, ..) = change(
            &mut m,
            "surface.rename",
            json!({ "surface": "surface:7", "title": "server" }),
        );
        assert_eq!(r["ref"], "surface:7");
        assert_eq!(m.workspace(4).unwrap().layout.tabs()[0].title, "server");
        assert!(
            call(
                &mut m,
                "surface.rename",
                &json!({ "surface": "surface:99", "title": "x" })
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn reads_in_the_apps_shapes() {
        let mut m = model();
        let list = read(&mut m, "workspace.list", json!({}));
        assert_eq!(list[1]["ref"], "workspace:4");
        assert_eq!(list[0]["selected"], true);
        assert_eq!(list[1]["pane_count"], 1);
        assert_eq!(read(&mut m, "workspace.current", json!({}))["name"], "home");
        assert_eq!(
            read(&mut m, "status.list", json!({ "workspace": "workspace:4" })),
            json!([])
        );
        assert!(
            call(&mut Model::default(), "workspace.list", &json!({}))
                .unwrap()
                .is_none(),
            "nothing known yet"
        );
    }
}
