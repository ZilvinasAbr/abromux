//! The model methods the server answers itself, whether or not the app is
//! attached (docs/server-plan.md, phase 2): a workspace's status pills, its
//! progress and its name. The server is their source: it changes its model,
//! stamps the change, answers the caller, and hands the app the call to
//! mirror in its views (`{"mirror":..}`), or keeps it for when the app is
//! back.

use abromux_model::{Model, Progress, Status};
use serde_json::{Value, json};

/// The methods the server owns.
pub const OWNED: &[&str] = &[
    "status.set",
    "status.clear",
    "progress.set",
    "progress.clear",
    "workspace.rename",
];

/// What a call did: its result for the caller, the workspace it changed,
/// and the call as the app mirrors it (its target made explicit).
pub struct Done {
    pub result: Value,
    pub workspace: u64,
    pub mirror: Value,
}

/// Run `method` on `model` (unstamped: the caller stamps the change).
pub fn call(model: &mut Model, method: &str, p: &Value) -> Result<Done, String> {
    let id = model.target(p).ok_or("no such workspace")?;
    let ws = model
        .workspaces
        .iter_mut()
        .find(|w| w.id == id)
        .ok_or("no such workspace")?;
    let r = format!("workspace:{id}");
    let str_param = |k: &str| p[k].as_str().map(String::from);
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
        "workspace.rename" => {
            let title = str_param("title").ok_or("missing title")?;
            ws.name = title.clone();
            json!({ "ref": r, "name": title })
        }
        m => return Err(format!("{m} isn't the server's")),
    };
    let mut mirror = p.clone();
    if let Some(o) = mirror.as_object_mut() {
        o.insert("workspace".into(), json!(r));
        o.remove("caller");
    }
    Ok(Done {
        result,
        workspace: id,
        mirror: json!({ "method": method, "params": mirror }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use abromux_model::{Node, Tab, Workspace};

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
            workspaces: vec![ws(1, "home", 1), ws(4, "vps", 7)],
            selected: Some(1),
        }
    }

    #[test]
    fn the_server_changes_its_model_and_says_what_to_mirror() {
        let mut m = model();
        let d = call(
            &mut m,
            "status.set",
            &json!({ "key": "build", "value": "ok", "caller": { "surface": "surface:7" } }),
        )
        .unwrap();
        assert_eq!(d.workspace, 4);
        assert_eq!(d.result["workspace"], "workspace:4");
        assert_eq!(m.workspace(4).unwrap().status[0].value, "ok");
        assert_eq!(d.mirror["params"]["workspace"], "workspace:4");
        assert!(d.mirror["params"].get("caller").is_none());

        call(
            &mut m,
            "progress.set",
            &json!({ "value": "0.5", "workspace": "vps" }),
        )
        .unwrap();
        assert_eq!(
            m.workspace(4).unwrap().progress.as_ref().unwrap().value,
            0.5
        );
        assert!(call(&mut m, "progress.set", &json!({ "value": 2 })).is_err());
        let d = call(&mut m, "workspace.rename", &json!({ "title": "Home!" })).unwrap();
        assert_eq!(d.workspace, 1, "the selected one");
        assert_eq!(m.workspace(1).unwrap().name, "Home!");
        let d = call(
            &mut m,
            "status.clear",
            &json!({ "key": "build", "workspace": "workspace:4" }),
        )
        .unwrap();
        assert_eq!(d.result["cleared"], true);
        assert!(
            call(
                &mut m,
                "status.set",
                &json!({ "key": "k", "value": "v", "workspace": "workspace:99" })
            )
            .is_err()
        );
    }
}
