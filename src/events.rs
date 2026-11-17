//! `abromux events`: what happens in the app, as one JSON object per line
//! for scripts and agents to follow (Rex's `rex events`, cmux's `events`).
//!
//! A subscriber is a control-socket connection that asked for
//! `events.subscribe` (`server::serve` keeps it open and writes each event
//! to it). Events come two ways:
//!
//! - Said as they happen (`emit`): notifications, the bell, a command that
//!   finished, a program's status, an agent's state.
//! - Found by comparing the app with how it last was (`AbroMux::diff_events`,
//!   every `POLL` while anyone listens): workspaces and surfaces opened,
//!   closed, renamed, retitled, moved to another directory, selected or
//!   focused. Nothing is compared while nobody listens.
//!
//! Each event has `event` (its name), `time` (Unix seconds), and the refs it
//! concerns (`workspace`, `surface`).

use crate::{AbroMux, workspace::Surface};
use gpui::{Context, Timer};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Mutex, mpsc},
    time::{Duration, SystemTime},
};

/// How often the app is compared while someone listens.
const POLL: Duration = Duration::from_millis(200);

/// Every event's name, for `events --help` and `rpc describe`.
pub const NAMES: &[(&str, &str)] = &[
    ("workspace.created", "a workspace opened: workspace, name"),
    ("workspace.closed", "a workspace closed: workspace"),
    (
        "workspace.renamed",
        "a workspace's name changed: workspace, name",
    ),
    (
        "workspace.selected",
        "the selected workspace changed: workspace",
    ),
    (
        "surface.created",
        "a tab opened: surface, workspace, type, title",
    ),
    (
        "surface.closed",
        "a tab closed: surface, workspace, exit_code (a terminal's program, when it exited by itself)",
    ),
    (
        "surface.moved",
        "a tab moved to another workspace: surface, workspace",
    ),
    (
        "surface.title",
        "a tab's title changed: surface, workspace, title",
    ),
    (
        "surface.cwd",
        "a terminal changed directory: surface, workspace, cwd",
    ),
    (
        "surface.focused",
        "the focused tab changed: surface, workspace",
    ),
    (
        "command.finished",
        "a command the shell ran finished (shell integration): surface, workspace, exit_code",
    ),
    (
        "program.status",
        "what a program reports about itself changed (OSC 7501, OSC 9;4): surface, workspace, \
         status (state, app, kind, progress, title, msg, id; null once there's none)",
    ),
    (
        "agent",
        "a coding agent's hooks reported: surface, workspace, agent, state (working, needs_input, stopped, ended)",
    ),
    (
        "notification",
        "a notification: id, surface, workspace, title, body",
    ),
    ("bell", "a terminal rang its bell: surface, workspace"),
];

type Sink = mpsc::Sender<String>;

static SUBSCRIBERS: Mutex<Vec<Sink>> = Mutex::new(Vec::new());

/// A new subscriber: everything emitted from now on arrives on the receiver.
pub fn subscribe() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    SUBSCRIBERS.lock().unwrap().push(tx);
    rx
}

/// Someone is listening.
pub fn listening() -> bool {
    !SUBSCRIBERS.lock().unwrap().is_empty()
}

/// Send `event` with `fields` to every subscriber; ones that hung up go.
pub fn emit(event: &str, mut fields: Value) {
    let mut subs = SUBSCRIBERS.lock().unwrap();
    if subs.is_empty() {
        return;
    }
    fields["event"] = json!(event);
    fields["time"] = json!(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0., |d| d.as_secs_f64())
    );
    let line = fields.to_string();
    subs.retain(|s| s.send(line.clone()).is_ok());
}

/// Forget subscribers whose connection is gone (their receivers dropped).
pub fn prune() {
    SUBSCRIBERS
        .lock()
        .unwrap()
        .retain(|s| s.send(String::new()).is_ok());
}

/// Whether `event` passes a subscriber's filters: `names` (exact names,
/// or prefixes ending in `.`/`*`), `workspace` and `surface` refs.
pub fn matches(event: &Value, filter: &Value) -> bool {
    let name = event["event"].as_str().unwrap_or_default();
    let named = match filter["events"].as_array() {
        Some(list) if !list.is_empty() => list
            .iter()
            .filter_map(Value::as_str)
            .any(|f| name_matches(name, f)),
        _ => true,
    };
    let scoped = |key: &str| {
        filter[key]
            .as_str()
            .is_none_or(|want| event[key].as_str() == Some(want))
    };
    named && scoped("workspace") && scoped("surface")
}

/// Event `name` against one filter: the exact name, or a prefix ending in
/// `.` or `*` (`surface.`, `surface*`).
pub fn name_matches(name: &str, filter: &str) -> bool {
    if filter.ends_with('*') || filter.ends_with('.') {
        name.starts_with(filter.trim_end_matches('*'))
    } else {
        name == filter
    }
}

/// What one surface looked like last time.
#[derive(Clone, PartialEq)]
struct Seen {
    workspace: usize,
    title: String,
    cwd: Option<String>,
}

/// The app as the last comparison found it.
#[derive(Default)]
pub struct State {
    /// None until the first comparison, which only records.
    workspaces: Option<HashMap<usize, String>>,
    surfaces: HashMap<usize, Seen>,
    selected: Option<usize>,
    focused: Option<usize>,
}

fn kind(s: &Surface) -> String {
    match s {
        Surface::Terminal(_) => "terminal".into(),
        Surface::Browser(_) => "browser".into(),
        Surface::Plugin(t) => t.plugin().to_string(),
    }
}

impl AbroMux {
    /// Compare the app every `POLL` while anyone listens (see the module docs).
    pub(crate) fn watch_events(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(POLL).await;
                if !listening() {
                    continue;
                }
                prune();
                if this.update(cx, |this, cx| this.diff_events(cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    fn diff_events(&mut self, cx: &mut Context<Self>) {
        let ws_ref = |id: usize| format!("workspace:{id}");
        let s_ref = |n: usize| format!("surface:{n}");
        let mut workspaces = HashMap::new();
        let mut surfaces = HashMap::new();
        let mut kinds = HashMap::new();
        for ws in &self.workspaces {
            workspaces.insert(ws.id, ws.name(cx));
            for s in ws.surfaces() {
                let Some(&n) = self.surface_refs.get(&s.entity_id()) else {
                    continue;
                };
                surfaces.insert(
                    n,
                    Seen {
                        workspace: ws.id,
                        title: s.title(cx),
                        cwd: matches!(s, Surface::Terminal(_))
                            .then(|| s.cwd(cx))
                            .flatten(),
                    },
                );
                kinds.insert(n, kind(s));
            }
        }
        let selected = self.workspaces.get(self.active).map(|w| w.id);
        let focused = self
            .workspaces
            .get(self.active)
            .and_then(|w| w.focused_surface())
            .and_then(|s| self.surface_refs.get(&s.entity_id()).copied());

        let st = &mut self.events;
        let Some(before) = st.workspaces.replace(workspaces.clone()) else {
            // The first look since someone started listening: nothing to say.
            st.surfaces = surfaces;
            (st.selected, st.focused) = (selected, focused);
            return;
        };
        for (id, name) in &workspaces {
            match before.get(id) {
                None => emit(
                    "workspace.created",
                    json!({ "workspace": ws_ref(*id), "name": name }),
                ),
                Some(old) if old != name => emit(
                    "workspace.renamed",
                    json!({ "workspace": ws_ref(*id), "name": name }),
                ),
                _ => {}
            }
        }
        for (n, now) in &surfaces {
            let (surface, workspace) = (s_ref(*n), ws_ref(now.workspace));
            let Some(old) = st.surfaces.get(n) else {
                emit(
                    "surface.created",
                    json!({ "surface": surface, "workspace": workspace,
                            "type": kinds[n], "title": now.title }),
                );
                continue;
            };
            if old.workspace != now.workspace {
                emit(
                    "surface.moved",
                    json!({ "surface": surface, "workspace": workspace }),
                );
            }
            if old.title != now.title {
                emit(
                    "surface.title",
                    json!({ "surface": surface, "workspace": workspace, "title": now.title }),
                );
            }
            if old.cwd != now.cwd
                && let Some(cwd) = &now.cwd
            {
                emit(
                    "surface.cwd",
                    json!({ "surface": surface, "workspace": workspace, "cwd": cwd }),
                );
            }
        }
        for (n, old) in &st.surfaces {
            if !surfaces.contains_key(n) {
                emit(
                    "surface.closed",
                    json!({ "surface": s_ref(*n), "workspace": ws_ref(old.workspace),
                            "exit_code": self.exits.get(n).copied().flatten() }),
                );
            }
        }
        for id in before.keys() {
            if !workspaces.contains_key(id) {
                emit("workspace.closed", json!({ "workspace": ws_ref(*id) }));
            }
        }
        if selected != st.selected
            && let Some(id) = selected
        {
            emit("workspace.selected", json!({ "workspace": ws_ref(id) }));
        }
        if focused != st.focused
            && let Some(n) = focused
        {
            emit(
                "surface.focused",
                json!({ "surface": s_ref(n), "workspace": selected.map(ws_ref) }),
            );
        }
        st.surfaces = surfaces;
        (st.selected, st.focused) = (selected, focused);
    }

    /// Say `event` about surface `id`, with its refs filled in.
    pub(crate) fn emit_for(&self, event: &str, id: gpui::EntityId, mut fields: Value) {
        if !listening() {
            return;
        }
        if let Some(n) = self.surface_refs.get(&id) {
            fields["surface"] = json!(format!("surface:{n}"));
        }
        if let Some(ws) = self.workspaces.iter().find(|w| w.owns(id)) {
            fields["workspace"] = json!(format!("workspace:{}", ws.id));
        }
        emit(event, fields);
    }
}

/// A program's status record as JSON (`program.status`, `surface.list`).
pub fn program_status_json(shown: Option<&abromux_core::program_status::Shown>) -> Value {
    let Some(s) = shown else { return Value::Null };
    let r = &s.record;
    json!({
        "id": s.id,
        "state": r.state.as_str(),
        "app": r.app,
        "kind": r.kind.map(|k| k.as_str()),
        "progress": r.progress,
        "title": r.title,
        "msg": r.msg,
        "others": s.others,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_by_name_prefix_and_ref() {
        let ev = json!({ "event": "surface.closed", "surface": "surface:3", "workspace": "workspace:1" });
        assert!(matches(&ev, &json!({})));
        assert!(matches(&ev, &json!({ "events": ["surface.closed"] })));
        assert!(matches(&ev, &json!({ "events": ["surface."] })));
        assert!(matches(&ev, &json!({ "events": ["surface*"] })));
        assert!(!matches(&ev, &json!({ "events": ["surface"] })));
        assert!(!matches(&ev, &json!({ "events": ["notification"] })));
        assert!(matches(&ev, &json!({ "workspace": "workspace:1" })));
        assert!(!matches(&ev, &json!({ "surface": "surface:4" })));
    }
}
