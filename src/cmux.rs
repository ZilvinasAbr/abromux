//! Importing cmux's session: cmux keeps its windows, workspaces, split
//! layout, tabs, scrollback, browser pages and coding agents in
//! `~/Library/Application Support/cmux/session-<bundle id>.json`, saved every
//! few seconds while it runs and again on quit. This turns that file into an
//! abromux [`Session`], which `restore-session --from cmux` and File ›
//! Import cmux Session add like any other session.
//!
//! Only what abromux has a place for comes over: workspace names, each
//! pane's tabs and selected tab, the splits and their ratios, the focused
//! pane, terminals' directories, font sizes and output, browser pages with
//! their back/forward lists (cmux saves the same WebKit state abromux does),
//! and agent sessions, which resume as on any restore. All of cmux's windows
//! go into abromux's one.

use crate::agent::Agent;
use crate::session::{NodeState, Session, TabState, WorkspaceState};
use serde_json::Value;
use std::path::PathBuf;

/// cmux's own session file (the release app's), or the copy it keeps of
/// the one before if that's all there is.
pub fn session_path() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let dir = home.join("Library/Application Support/cmux");
    let current = dir.join("session-com.cmuxterm.app.json");
    let previous = dir.join("session-com.cmuxterm.app-previous.json");
    if !current.exists() && previous.exists() {
        previous
    } else {
        current
    }
}

/// Whether `v` is a cmux session rather than one of abromux's.
pub fn is_cmux(v: &Value) -> bool {
    v["windows"]
        .as_array()
        .is_some_and(|w| w.iter().any(|w| w.get("tabManager").is_some()))
}

/// The abromux session for cmux session `v`.
pub fn convert(v: &Value) -> Result<Session, String> {
    if !is_cmux(v) {
        return Err("not a cmux session".into());
    }
    let windows = v["windows"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut workspaces = vec![];
    for w in windows {
        for ws in arr(&w["tabManager"]["workspaces"]) {
            if let Some(state) = workspace(ws) {
                workspaces.push(state);
            }
        }
    }
    if workspaces.is_empty() {
        return Err("the cmux session has no workspaces".into());
    }
    let active = windows
        .first()
        .and_then(|w| w["tabManager"]["selectedWorkspaceIndex"].as_u64())
        .unwrap_or(0) as usize;
    let sidebar = windows.first().map(|w| &w["sidebar"]);
    Ok(Session {
        version: crate::session::VERSION,
        window: None,
        active: active.min(workspaces.len() - 1),
        sidebar: sidebar
            .and_then(|s| s["isVisible"].as_bool())
            .unwrap_or(true),
        sidebar_width: sidebar
            .and_then(|s| s["width"].as_f64())
            .map_or(crate::SIDEBAR_WIDTH, |w| w as f32),
        workspaces,
        groups: vec![],
    })
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v[key].as_str().filter(|s| !s.is_empty())
}

fn workspace(ws: &Value) -> Option<WorkspaceState> {
    let panels: Vec<&Value> = arr(&ws["panels"]).iter().collect();
    let dir = str_of(ws, "currentDirectory").unwrap_or_default();
    let focused_panel = str_of(ws, "focusedPanelId");
    let mut panes = 0;
    let mut focused = None;
    let root = node(
        &ws["layout"],
        &panels,
        dir,
        focused_panel,
        &mut panes,
        &mut focused,
    )?;
    // A title the user gave stays put; one an agent or cmux gave it can be
    // replaced, like abromux's own automatic names.
    let title = str_of(ws, "customTitle").map(str::to_string);
    let by_user = ws["customTitleSource"].as_str().is_none_or(|s| s == "user");
    Some(WorkspaceState {
        name: title.clone().filter(|_| by_user),
        auto_name: title.filter(|_| !by_user),
        description: str_of(ws, "customDescription").map(str::to_string),
        color: str_of(ws, "customColor").and_then(hex_color),
        group: None,
        focused: focused.unwrap_or(0),
        root,
    })
}

/// `#rrggbb` as abromux keeps colors.
fn hex_color(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#').unwrap_or(s);
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok())?
}

/// A layout node. `panes` counts panes in tree order, so `focused` can say
/// which one holds the focused panel.
fn node(
    layout: &Value,
    panels: &[&Value],
    dir: &str,
    focused_panel: Option<&str>,
    panes: &mut usize,
    focused: &mut Option<usize>,
) -> Option<NodeState> {
    match layout["type"].as_str()? {
        "pane" => {
            let pane = &layout["pane"];
            let ids: Vec<&str> = arr(&pane["panelIds"])
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let selected = str_of(pane, "selectedPanelId");
            let mut tabs = vec![];
            let mut active = 0;
            for id in &ids {
                let Some(p) = panels.iter().find(|p| p["id"].as_str() == Some(id)) else {
                    continue;
                };
                let Some(t) = tab(p, dir) else { continue };
                if Some(*id) == selected {
                    active = tabs.len();
                }
                tabs.push(t);
            }
            if tabs.is_empty() {
                return None;
            }
            if focused.is_none() && focused_panel.is_some_and(|f| ids.contains(&f)) {
                *focused = Some(*panes);
            }
            *panes += 1;
            Some(NodeState::Pane { active, tabs })
        }
        "split" => {
            let split = &layout["split"];
            let a = node(&split["first"], panels, dir, focused_panel, panes, focused);
            let b = node(&split["second"], panels, dir, focused_panel, panes, focused);
            match (a, b) {
                (Some(a), Some(b)) => Some(NodeState::Split {
                    // cmux's "horizontal" lays its two side by side, as
                    // abromux's horizontal axis does.
                    vertical: split["orientation"].as_str() == Some("vertical"),
                    ratio: split["dividerPosition"].as_f64().unwrap_or(0.5) as f32,
                    children: Box::new([a, b]),
                }),
                (a, b) => a.or(b),
            }
        }
        _ => None,
    }
}

fn tab(p: &Value, dir: &str) -> Option<TabState> {
    let name = str_of(p, "customTitle").map(str::to_string);
    match p["type"].as_str()? {
        "terminal" => {
            let t = &p["terminal"];
            let cwd = str_of(t, "workingDirectory")
                .or(str_of(p, "directory"))
                .unwrap_or(dir)
                .to_string();
            Some(TabState::Terminal {
                font_size: t["fontSize"].as_f64().map(|f| f as f32),
                scrollback: str_of(t, "scrollback").map(scrollback),
                agent: agent(t),
                name,
                cwd,
                held: None,
                surface: None,
            })
        }
        "browser" => {
            let b = &p["browser"];
            Some(TabState::Browser {
                url: str_of(b, "urlString").unwrap_or_default().to_string(),
                history: str_of(b, "interactionState").map(str::to_string),
                name,
            })
        }
        _ => None,
    }
}

/// The agent cmux would resume in this terminal: the one it says was
/// running, or failing that the one its resume binding names.
fn agent(t: &Value) -> Option<Agent> {
    if t["wasAgentRunning"].as_bool() == Some(false) {
        return None;
    }
    let a = &t["agent"];
    let rb = &t["resumeBinding"];
    if rb["autoResume"].as_bool() == Some(false) {
        return None;
    }
    let (kind, id, cwd, mode) =
        if let (Some(k), Some(id)) = (str_of(a, "kind"), str_of(a, "sessionId")) {
            (
                k,
                id,
                str_of(a, "workingDirectory"),
                str_of(a, "permissionMode"),
            )
        } else {
            (
                str_of(rb, "kind")?,
                str_of(rb, "checkpointId")?,
                str_of(rb, "cwd"),
                str_of(rb, "permissionMode"),
            )
        };
    crate::agent::Kind::parse(kind)?;
    Some(Agent {
        kind: kind.into(),
        session_id: id.into(),
        cwd: cwd
            .or(str_of(t, "workingDirectory"))
            .unwrap_or_default()
            .into(),
        permission_mode: mode.map(str::to_string),
        pid: None,
    })
}

/// cmux's saved output as abromux replays it: lines end in a bare `\n`,
/// which needs a carriage return in a terminal, and it starts by setting
/// the terminal's colors to cmux's theme (OSC 4, 10–19, 104, 110–119),
/// which would outlast the replay, so those go.
fn scrollback(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 32);
    let mut rest = s;
    while let Some(i) = rest.find(['\n', '\x1b']) {
        let (before, at) = rest.split_at(i);
        out.push_str(before);
        if let Some(after) = at.strip_prefix('\n') {
            if !out.ends_with('\r') {
                out.push('\r');
            }
            out.push('\n');
            rest = after;
        } else if let Some(len) = color_osc(at) {
            rest = &at[len..];
        } else {
            out.push('\x1b');
            rest = &at[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The length of the color-setting OSC sequence `s` starts with, if it
/// starts with one.
fn color_osc(s: &str) -> Option<usize> {
    let body = s.strip_prefix("\x1b]")?;
    let num: String = body.chars().take_while(char::is_ascii_digit).collect();
    let n: u32 = num.parse().ok()?;
    if !matches!(n, 4 | 10..=19 | 104 | 110..=119) {
        return None;
    }
    let end = body.find(['\x07', '\x1b'])?;
    let term = if body[end..].starts_with('\x07') {
        1
    } else if body[end..].starts_with("\x1b\\") {
        2
    } else {
        return None;
    };
    Some(2 + end + term)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn terminal(id: &str, cwd: &str) -> Value {
        json!({
            "id": id, "type": "terminal", "directory": cwd, "title": "zsh",
            "terminal": {"workingDirectory": cwd, "fontSize": 15,
                         "scrollback": "\u{1b}]10;rgb:ff/ff/ff\u{1b}\\\u{1b}]11;rgb:1e/1e/1e\u{7}$ ls\na b\n"}
        })
    }

    fn sample() -> Value {
        let mut agent = terminal("T2", "/repo");
        agent["terminal"]["agent"] = json!({
            "kind": "claude", "sessionId": "91d0-3e", "workingDirectory": "/repo",
            "permissionMode": "auto"
        });
        agent["terminal"]["wasAgentRunning"] = json!(true);
        json!({
            "version": 1,
            "windows": [{
                "sidebar": {"isVisible": true, "width": 280.0},
                "tabManager": {
                    "selectedWorkspaceIndex": 1,
                    "workspaces": [
                        {
                            "customTitle": "Docs", "customTitleSource": "user",
                            "currentDirectory": "/docs",
                            "focusedPanelId": "T1",
                            "layout": {"type": "pane", "pane": {"panelIds": ["T1"], "selectedPanelId": "T1"}},
                            "panels": [terminal("T1", "/docs")]
                        },
                        {
                            "customTitle": "Fix the build", "customTitleSource": "agent",
                            "currentDirectory": "/repo",
                            "focusedPanelId": "B1",
                            "layout": {"type": "split", "split": {
                                "orientation": "horizontal", "dividerPosition": 0.4,
                                "first": {"type": "pane", "pane": {"panelIds": ["T1x", "T2", "M1"], "selectedPanelId": "T2"}},
                                "second": {"type": "split", "split": {
                                    "orientation": "vertical", "dividerPosition": 0.5,
                                    "first": {"type": "pane", "pane": {"panelIds": ["B1"], "selectedPanelId": "B1"}},
                                    "second": {"type": "pane", "pane": {"panelIds": ["GONE"]}}
                                }}
                            }},
                            "panels": [
                                terminal("T1x", "/repo"),
                                agent,
                                {"id": "M1", "type": "markdown"},
                                {"id": "B1", "type": "browser", "customTitle": "Docs site",
                                 "browser": {"urlString": "https://example.com", "interactionState": "AAAA"}}
                            ]
                        }
                    ]
                }
            }]
        })
    }

    #[test]
    fn converts_layout_tabs_and_agents() {
        let s = convert(&sample()).unwrap();
        assert_eq!(s.active, 1);
        assert_eq!(s.sidebar_width, 280.);
        assert_eq!(s.workspaces.len(), 2);
        assert_eq!(s.workspaces[0].name.as_deref(), Some("Docs"));

        let ws = &s.workspaces[1];
        assert_eq!(ws.name, None);
        assert_eq!(ws.auto_name.as_deref(), Some("Fix the build"));
        // The empty pane folded away, leaving the browser's pane second.
        assert_eq!(ws.focused, 1);
        let NodeState::Split {
            vertical,
            ratio,
            children,
        } = &ws.root
        else {
            panic!("{:?}", ws.root)
        };
        assert!(!vertical);
        assert_eq!(*ratio, 0.4);
        let NodeState::Pane { active, tabs } = &children[0] else {
            panic!()
        };
        // The markdown panel has no abromux counterpart.
        assert_eq!((tabs.len(), *active), (2, 1));
        let TabState::Terminal {
            cwd,
            font_size,
            scrollback,
            agent,
            ..
        } = &tabs[1]
        else {
            panic!()
        };
        assert_eq!(cwd, "/repo");
        assert_eq!(*font_size, Some(15.));
        assert_eq!(scrollback.as_deref(), Some("$ ls\r\na b\r\n"));
        let agent = agent.as_ref().unwrap();
        assert_eq!(
            agent.resume_command("/repo").unwrap(),
            "claude --resume 91d0-3e --permission-mode auto"
        );
        assert!(matches!(&tabs[0], TabState::Terminal { agent: None, .. }));
        assert_eq!(
            children[1],
            NodeState::Pane {
                active: 0,
                tabs: vec![TabState::Browser {
                    url: "https://example.com".into(),
                    name: Some("Docs site".into()),
                    history: Some("AAAA".into()),
                }]
            }
        );
    }

    #[test]
    fn falls_back_to_the_resume_binding_and_skips_finished_agents() {
        let t = json!({
            "workingDirectory": "/p",
            "resumeBinding": {"kind": "codex", "checkpointId": "01a", "cwd": "/p", "autoResume": true}
        });
        assert_eq!(
            agent(&t).unwrap().resume_command("/p").unwrap(),
            "codex resume 01a"
        );
        let mut done = t.clone();
        done["wasAgentRunning"] = json!(false);
        assert!(agent(&done).is_none());
        let unknown = json!({"agent": {"kind": "aider", "sessionId": "x"}});
        assert!(agent(&unknown).is_none());
    }

    #[test]
    fn scrollback_keeps_other_escapes() {
        assert_eq!(
            scrollback("\x1b]8;;http://a\x1b\\a\x1b]8;;\x1b\\\r\n\x1b[1mb\x1b]4;1;rgb:0/0/0\x07"),
            "\x1b]8;;http://a\x1b\\a\x1b]8;;\x1b\\\r\n\x1b[1mb"
        );
    }

    #[test]
    fn rejects_other_files() {
        assert!(convert(&json!({"version": 1, "active": 0, "workspaces": []})).is_err());
        assert!(convert(&json!({"windows": [{"tabManager": {"workspaces": []}}]})).is_err());
    }
}
