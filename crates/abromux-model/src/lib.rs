//! The workspace model without UI (docs/server-plan.md, phase 2): what the
//! app's workspaces are, as plain data keyed by ids (not GPUI entities), and
//! how they change, as events with a gapless `seq`.
//!
//! - The app builds a [`Model`] from its state, and keeps its own copy up to
//!   date only through [`Model::apply`] of the events [`diff`] gives: the
//!   same path a replica takes.
//! - The app's server keeps a replica: it applies the events the app sends,
//!   and asks for a snapshot when one is missing (a gap). It answers for the
//!   app from it while the app is closed.
//! - Any other client (another app, the web client) subscribes: a snapshot
//!   with its `seq`, then every event after it, in order.
//!
//! Snapshot + the events after it = the state, whoever applies them.

use serde::{Deserialize, Serialize};

/// Every workspace, in the sidebar's order, and which one is selected.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Model {
    /// The last event applied (0: none yet).
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub selected: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Its `workspace:N`.
    pub id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The server on another machine its terminals run on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Its workspace folder, on its host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Its sidebar group's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub layout: Node,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    /// Unread notifications in it.
    #[serde(default)]
    pub unread: u32,
    /// `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The git branch of its folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `abromux log` lines, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<LogEntry>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// info, progress, success, warning or error.
    pub level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
    /// Seconds since the epoch.
    pub at: u64,
}

/// Its panes: a tree of splits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    Pane {
        id: u64,
        tabs: Vec<Tab>,
        #[serde(default)]
        active: usize,
    },
    Split {
        vertical: bool,
        ratio: f32,
        children: Box<[Node; 2]>,
    },
}

impl Default for Node {
    fn default() -> Self {
        Node::Pane {
            id: 0,
            tabs: vec![],
            active: 0,
        }
    }
}

impl Node {
    /// Every tab, in tree order.
    pub fn tabs(&self) -> Vec<&Tab> {
        match self {
            Node::Pane { tabs, .. } => tabs.iter().collect(),
            Node::Split { children, .. } => {
                let mut v = children[0].tabs();
                v.extend(children[1].tabs());
                v
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    /// Its `surface:N`.
    pub surface: u64,
    /// terminal, browser, or a plugin's kind.
    pub kind: String,
    pub title: String,
    /// The server's id for a terminal it keeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
    /// The server on another machine it runs on, if not the workspace's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One change. A changed workspace is sent whole: a workspace is small, and
/// it keeps every client's apply trivially right.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Inserted at `index` (or changed, if one with its id is there).
    Workspace {
        index: usize,
        workspace: Box<Workspace>,
    },
    Removed {
        id: u64,
    },
    Selected {
        id: Option<u64>,
    },
}

/// An event and its place in the sequence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stamped {
    pub seq: u64,
    #[serde(flatten)]
    pub event: Event,
}

/// An event that doesn't follow the last one applied: some are missing,
/// so the applier needs a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    pub have: u64,
    pub got: u64,
}

impl std::fmt::Display for Gap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "event {} after {}: some are missing",
            self.got, self.have
        )
    }
}

impl Model {
    /// Apply one event. It must be the next in sequence; one already
    /// applied (a resend) is ignored.
    pub fn apply(&mut self, e: &Stamped) -> Result<(), Gap> {
        if e.seq <= self.seq {
            return Ok(());
        }
        if e.seq != self.seq + 1 {
            return Err(Gap {
                have: self.seq,
                got: e.seq,
            });
        }
        match &e.event {
            Event::Workspace { index, workspace } => {
                if let Some(at) = self.workspaces.iter().position(|w| w.id == workspace.id) {
                    self.workspaces.remove(at);
                }
                let i = (*index).min(self.workspaces.len());
                self.workspaces.insert(i, (**workspace).clone());
            }
            Event::Removed { id } => self.workspaces.retain(|w| w.id != *id),
            Event::Selected { id } => self.selected = *id,
        }
        self.seq = e.seq;
        Ok(())
    }

    pub fn workspace(&self, id: u64) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    /// The workspace a call is about: `workspace` (`workspace:N`, an index,
    /// or a name), else the caller's (`caller.surface`), else the selected
    /// one.
    pub fn target(&self, params: &serde_json::Value) -> Option<u64> {
        if let Some(t) = params.get("workspace").filter(|t| !t.is_null()) {
            let t = match t {
                serde_json::Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            if let Some(n) = t.strip_prefix("workspace:").and_then(|n| n.parse().ok()) {
                return self.workspace(n).map(|w| w.id);
            }
            if let Ok(i) = t.parse::<usize>() {
                return self.workspaces.get(i).map(|w| w.id);
            }
            return self.workspaces.iter().find(|w| w.name == t).map(|w| w.id);
        }
        let caller = params["caller"]["surface"]
            .as_str()
            .or_else(|| params["surface"].as_str())
            .and_then(|s| s.trim_start_matches("surface:").parse::<u64>().ok());
        if let Some(w) = caller.and_then(|s| self.workspace_of(s)) {
            return Some(w.id);
        }
        self.selected.or(self.workspaces.first().map(|w| w.id))
    }

    /// The workspace tab `surface` is in.
    pub fn workspace_of(&self, surface: u64) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|w| w.layout.tabs().iter().any(|t| t.surface == surface))
    }
}

/// The events that make `old` into `new` (ignoring `seq`), unstamped: the
/// removed workspaces, then each new, moved or changed one at its place,
/// then the selection.
pub fn diff(old: &Model, new: &Model) -> Vec<Event> {
    let mut out = vec![];
    let mut cur: Vec<&Workspace> = old.workspaces.iter().collect();
    for w in &old.workspaces {
        if new.workspace(w.id).is_none() {
            out.push(Event::Removed { id: w.id });
            cur.retain(|x| x.id != w.id);
        }
    }
    for (i, w) in new.workspaces.iter().enumerate() {
        let same_place = cur.get(i).is_some_and(|x| *x == w);
        if !same_place {
            out.push(Event::Workspace {
                index: i,
                workspace: Box::new(w.clone()),
            });
            cur.retain(|x| x.id != w.id);
            cur.insert(i.min(cur.len()), w);
        }
    }
    if old.selected != new.selected {
        out.push(Event::Selected { id: new.selected });
    }
    out
}

/// Stamp `events` after `seq`: the seq of each, gapless.
pub fn stamp(seq: u64, events: Vec<Event>) -> Vec<Stamped> {
    events
        .into_iter()
        .enumerate()
        .map(|(i, event)| Stamped {
            seq: seq + 1 + i as u64,
            event,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(id: u64, name: &str, surfaces: &[u64]) -> Workspace {
        Workspace {
            id,
            name: name.into(),
            layout: Node::Pane {
                id,
                active: 0,
                tabs: surfaces
                    .iter()
                    .map(|s| Tab {
                        surface: *s,
                        kind: "terminal".into(),
                        title: format!("t{s}"),
                        ..Default::default()
                    })
                    .collect(),
            },
            ..Default::default()
        }
    }

    /// Whatever the change, applying its diff to the old model gives the new.
    fn check(old: &Model, new: &Model) {
        let mut replica = old.clone();
        for e in stamp(old.seq, diff(old, new)) {
            replica.apply(&e).unwrap();
        }
        let mut want = new.clone();
        want.seq = replica.seq;
        assert_eq!(replica, want, "diff {:#?}", diff(old, new));
    }

    #[test]
    fn diffs_apply_to_the_same_model() {
        let a = Model {
            seq: 4,
            workspaces: vec![ws(1, "home", &[1]), ws(2, "vps", &[2, 3]), ws(3, "x", &[4])],
            selected: Some(1),
        };
        // Renamed, reordered, removed, added, reselected, status.
        let mut b = a.clone();
        b.workspaces[1].name = "VPS".into();
        b.workspaces.swap(0, 2);
        check(&a, &b);
        let mut c = b.clone();
        c.workspaces.remove(1);
        c.workspaces.insert(0, ws(9, "new", &[9]));
        c.selected = Some(9);
        c.workspaces[1].status.push(Status {
            key: "build".into(),
            value: "ok".into(),
            color: None,
            icon: None,
        });
        check(&b, &c);
        check(&c, &Model::default());
        check(&Model::default(), &a);
        assert!(diff(&a, &a).is_empty());
    }

    #[test]
    fn a_gap_is_refused_and_a_resend_ignored() {
        let mut m = Model::default();
        let e = |seq| Stamped {
            seq,
            event: Event::Selected { id: Some(seq) },
        };
        m.apply(&e(1)).unwrap();
        assert_eq!(m.apply(&e(3)), Err(Gap { have: 1, got: 3 }));
        m.apply(&e(1)).unwrap();
        assert_eq!(m.selected, Some(1));
        m.apply(&e(2)).unwrap();
        assert_eq!(m.seq, 2);
    }

    #[test]
    fn events_round_trip_as_json() {
        let e = stamp(7, vec![Event::Removed { id: 3 }])[0].clone();
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#"{"seq":8,"event":"removed","id":3}"#);
        assert_eq!(serde_json::from_str::<Stamped>(&s).unwrap(), e);
        let m = Model {
            seq: 1,
            workspaces: vec![ws(1, "home", &[1, 2])],
            selected: Some(1),
        };
        let back: Model = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(m.workspace_of(2).unwrap().name, "home");
    }
}
