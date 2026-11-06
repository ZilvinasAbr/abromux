//! abromux plugin: review GitLab merge requests in a tab: read them, comment,
//! reply, resolve threads, start threads on lines and approve.
//!
//! - `api`: the REST API through curl; `types`: its responses
//! - `diff`: unified diff parsing; `review`: a merge request with its
//!   threads placed on their lines
//! - `auth`: tokens in the Keychain; `view`: the tab
//!
//! The tab's [`Location`] is what it saves, so it comes back after a
//! restart on the same host, list or merge request.

pub mod api;
pub mod auth;
mod avatars;
mod changes;
pub mod diff;
mod discussion;
mod edit;
mod emoji;
pub mod highlight;
mod mr;
mod style;
pub mod review;
pub mod time;
pub mod types;
pub mod view;

use abromux_plugin::{Plugin, TabHandle};
use gpui::{App, AppContext as _, Window};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PLUGIN: Plugin = Plugin {
    id: "gitlab",
    title: "New GitLab Tab",
    open,
    assets: &ASSETS,
};

macro_rules! icons {
    ($($name:literal),*) => {
        [$((concat!("plugins/gitlab/", $name, ".svg"), include_bytes!(concat!("../assets/", $name, ".svg")).as_slice())),*]
    };
}

static ASSETS: [(&str, &[u8]); 21] = icons!(
    "collapse-all",
    "list",
    "tree",
    "circle-dashed",
    "status-pending",
    "status-canceled",
    "external-link",
    "loader",
    "expand-up",
    "expand-down",
    "expand-all",
    "comment",
    "check-circle",
    "copy",
    "file",
    "square-plus",
    "square-minus",
    "square-dot",
    "square-check",
    "square",
    "merge"
);

/// The host a tab opens on when nothing says otherwise.
pub const DEFAULT_HOST: &str = "http://localhost:8929";

/// Which merge requests the list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListScope {
    #[default]
    ToReview,
    Mine,
    All,
}

/// Which state of merge requests the list shows, as GitLab's state tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MrState {
    #[default]
    Opened,
    Merged,
    Closed,
    All,
}

impl MrState {
    pub fn api(self) -> &'static str {
        match self {
            MrState::Opened => "opened",
            MrState::Merged => "merged",
            MrState::Closed => "closed",
            MrState::All => "all",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MrState::Opened => "Open",
            MrState::Merged => "Merged",
            MrState::Closed => "Closed",
            MrState::All => "All",
        }
    }
}

/// The part of a merge request showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    #[default]
    Overview,
    Changes,
    Commits,
}

/// What a tab is open on: what `save` keeps and `open` takes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Location {
    pub host: Option<String>,
    pub scope: ListScope,
    pub state: MrState,
    /// The merge request open, if any: its project id and iid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iid: Option<u64>,
    pub section: Section,
    /// The Changes tab side by side instead of inline.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub parallel: bool,
    /// Files of the open merge request marked Viewed (GitLab keeps these in
    /// the browser too).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub viewed: Vec<String>,
}

impl Location {
    pub fn host(&self) -> &str {
        self.host.as_deref().unwrap_or(DEFAULT_HOST)
    }
}

/// `params`: a saved [`Location`], or `{"url": "<merge request URL>"}`.
fn open(params: &Value, window: &mut Window, cx: &mut App) -> anyhow::Result<TabHandle> {
    let url = params.get("url").and_then(Value::as_str).map(str::to_string);
    let mut loc: Location = serde_json::from_value(params.clone()).unwrap_or_default();
    if let Some(u) = &url {
        let (host, _, _) = api::parse_mr_url(u).ok_or_else(|| anyhow::anyhow!("not a merge request URL: {u}"))?;
        loc.host = Some(host);
    }
    let view = cx.new(|cx| {
        let mut v = view::GitLabView::new(loc, window, cx);
        if let Some(u) = &url {
            let _ = abromux_plugin::Tab::command(&mut v, "open", &serde_json::json!({ "url": u }), window, cx);
        }
        v
    });
    Ok(TabHandle::new(PLUGIN.id, view))
}

/// Everything the review of merge request `iid` in `project` needs. Blocks.
pub fn load_review(c: &api::Client, project: u64, iid: u64) -> anyhow::Result<review::Review> {
    let mut r = review::Review::build(
        c.merge_request(project, iid)?,
        c.diffs(project, iid)?,
        c.discussions(project, iid)?,
        c.approvals(project, iid)?,
        c.commits(project, iid)?,
    );
    // Role badges are a nicety: a token that can't list members still reviews.
    if let Ok(members) = c.members(project) {
        r.roles = members.iter().filter_map(|m| Some((m.id, m.role()?))).collect();
    }
    r.versions = c.versions(project, iid).unwrap_or_default();
    // The single merge request has label names only: colors from the project's.
    if r.mr.labels.iter().any(|l| l.color.is_empty())
        && let Ok(all) = c.labels(project)
    {
        for l in &mut r.mr.labels {
            if let Some(full) = all.iter().find(|a| a.name == l.name) {
                *l = full.clone();
            }
        }
    }
    // "closed", "merged", "reopened": in the activity as GitLab's own notes.
    for e in c.state_events(project, iid).unwrap_or_default() {
        r.system.push(types::Note {
            id: e.id,
            body: e.state,
            author: e.user.unwrap_or_default(),
            created_at: e.created_at,
            system: true,
            ..Default::default()
        });
    }
    Ok(r)
}

/// Which diffs the Changes tab shows: version `head` (None: the latest)
/// against version `base` (None: the target branch); or just one commit,
/// `review.commits[commit]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Compare {
    pub base: Option<u64>,
    pub head: Option<u64>,
    pub commit: Option<usize>,
}

/// The review of `compare` instead of the latest version: the same merge
/// request with those diffs, and no threads (they belong to the latest). Blocks.
pub fn load_compare(c: &api::Client, r: &review::Review, compare: Compare) -> anyhow::Result<review::Review> {
    let (p, iid) = (r.mr.project_id, r.mr.iid);
    // One commit: its diff, its text, and its parent's for the old side.
    if let Some(i) = compare.commit {
        let commit = r.commits.get(i).ok_or_else(|| anyhow::anyhow!("no commit {i}"))?;
        let diffs = c.commit_diff(p, &commit.id)?;
        let mut mr = r.mr.clone();
        let parent = format!("{}~1", commit.id);
        mr.diff_refs = Some(types::DiffRefs { base_sha: parent.clone(), start_sha: parent, head_sha: commit.id.clone() });
        let mut out = review::Review::build(mr, diffs, vec![], r.approvals.clone(), r.commits.clone());
        out.roles = r.roles.clone();
        out.versions = r.versions.clone();
        return Ok(out);
    }
    let sha = |id: u64| {
        r.versions
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.head_commit_sha.clone())
            .ok_or_else(|| anyhow::anyhow!("no version {id}"))
    };
    let head = compare.head.or(r.versions.first().map(|v| v.id));
    let diffs = match (compare.base, head) {
        (None, None) => c.diffs(p, iid)?,
        (None, Some(h)) => c.version_diffs(p, iid, h)?,
        (Some(b), Some(h)) => c.compare(p, &sha(b)?, &sha(h)?)?,
        (Some(_), None) => anyhow::bail!("a base version needs a head version"),
    };
    let mut mr = r.mr.clone();
    // Unchanged lines and images come from the compared commits.
    if let Some(h) = head.and_then(|h| r.versions.iter().find(|v| v.id == h)) {
        let base = match compare.base {
            Some(b) => sha(b)?,
            None => h.base_commit_sha.clone(),
        };
        mr.diff_refs = Some(types::DiffRefs { base_sha: base.clone(), start_sha: base, head_sha: h.head_commit_sha.clone() });
    }
    let mut out = review::Review::build(mr, diffs, vec![], r.approvals.clone(), r.commits.clone());
    out.roles = r.roles.clone();
    out.versions = r.versions.clone();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{ListScope, Location, MrState, Section};

    #[test]
    fn location_round_trips_and_tolerates_junk() {
        let loc = Location {
            host: Some("http://gl.test".into()),
            scope: ListScope::All,
            state: MrState::Merged,
            project: Some(3),
            iid: Some(7),
            section: Section::Commits,
            parallel: true,
            viewed: vec!["src/a.rs".into()],
        };
        let v = serde_json::to_value(&loc).unwrap();
        assert_eq!(serde_json::from_value::<Location>(v).unwrap(), loc);
        assert_eq!(
            serde_json::from_value::<Location>(serde_json::json!({ "junk": 1 })).unwrap(),
            Location::default()
        );
    }
}
