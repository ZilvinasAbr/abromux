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
pub mod cache;
mod avatars;
mod changes;
pub mod diff;
mod discussion;
mod edit;
mod emoji;
pub mod highlight;
pub mod local;
mod mr;
mod search;
mod projects;
mod select;
mod side;
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
    claims,
    open_link: Some("Open GitLab Merge Request…"),
};

/// A merge request's page on any GitLab (`…/-/merge_requests/12`), or a
/// project's merge requests (`…/group/project/-/merge_requests`).
fn claims(url: &str) -> bool {
    api::parse_mr_url(url).is_some() || api::parse_project_mrs_url(url).is_some()
}

macro_rules! icons {
    ($($name:literal),*) => {
        [$((concat!("plugins/gitlab/", $name, ".svg"), include_bytes!(concat!("../assets/", $name, ".svg")).as_slice())),*]
    };
}

static ASSETS: [(&str, &[u8]); 31] = icons!(
    "comment-lines",
    "sort-desc",
    "sort-asc",
    "minus-circle-filled",
    "file-image",
    "markdown",
    "reply",
    "link",
    "smile-plus",
    "check-circle-filled",
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
    /// Your projects, to browse one's merge requests.
    Projects,
}

/// The project whose merge requests the list shows (picked from Projects,
/// or a project's merge requests link). `id` 0: found by `path` on load.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Browse {
    pub id: u64,
    /// `group/project`
    pub path: String,
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
    Pipelines,
}

impl Section {
    /// The tab a merge request URL is on: `…/merge_requests/1/diffs` the
    /// Changes, `…/1/commits` the Commits, anything else the Overview.
    pub fn of_url(url: &str) -> Section {
        let after = url.split_once("/-/merge_requests/").map_or("", |(_, a)| a);
        match after.split(['?', '#']).next().unwrap_or_default().split('/').nth(1) {
            Some("diffs") => Section::Changes,
            Some("commits") => Section::Commits,
            Some("pipelines") => Section::Pipelines,
            _ => Section::Overview,
        }
    }
}

/// How the list is ordered, as GitLab's sort dropdown offers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListSort {
    #[default]
    Created,
    Updated,
    Title,
    Popularity,
}

impl ListSort {
    pub const ALL: [ListSort; 4] = [ListSort::Created, ListSort::Updated, ListSort::Title, ListSort::Popularity];

    pub fn label(self) -> &'static str {
        match self {
            ListSort::Created => "Created date",
            ListSort::Updated => "Updated date",
            ListSort::Title => "Title",
            ListSort::Popularity => "Popularity",
        }
    }

    /// `mrs` in this order, newest (or most, or Z) first unless `ascending`.
    pub fn apply(self, mrs: &mut [&types::MergeRequest], ascending: bool) {
        match self {
            ListSort::Created => mrs.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
            ListSort::Updated => mrs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
            ListSort::Title => mrs.sort_by_key(|m| std::cmp::Reverse(m.title.to_lowercase())),
            ListSort::Popularity => mrs.sort_by(|a, b| b.upvotes.cmp(&a.upvotes).then(b.created_at.cmp(&a.created_at))),
        }
        if ascending {
            mrs.reverse();
        }
    }
}

/// What a tab is open on: what `save` keeps and `open` takes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Location {
    pub host: Option<String>,
    pub scope: ListScope,
    /// A project's merge requests instead of the scope's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browse: Option<Browse>,
    pub state: MrState,
    /// The list's order, and which way.
    pub sort: ListSort,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ascending: bool,
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
    // A project's merge requests link: the list of those.
    let project_list = url.as_deref().and_then(api::parse_project_mrs_url);
    if let Some((host, path)) = &project_list {
        loc.host = Some(host.clone());
        loc.browse = Some(Browse { id: 0, path: path.clone() });
    } else if let Some(u) = &url {
        let (host, _, _) = api::parse_mr_url(u).ok_or_else(|| anyhow::anyhow!("not a merge request URL: {u}"))?;
        loc.host = Some(host);
    }
    let view = cx.new(|cx| {
        let mut v = view::GitLabView::new(loc, window, cx);
        if let Some(u) = url.as_ref().filter(|_| project_list.is_none()) {
            let _ = abromux_plugin::Tab::command(&mut v, "open", &serde_json::json!({ "url": u }), window, cx);
        }
        v
    });
    Ok(TabHandle::new(PLUGIN.id, view))
}

/// Everything the review of merge request `iid` in `project` needs. Blocks.
pub fn load_review(c: &api::Client, project: u64, iid: u64) -> anyhow::Result<review::Review> {
    use api::first_page;
    let t0 = std::time::Instant::now();
    let lap = |what: &str| {
        if std::env::var_os("ABROMUX_GITLAB_TIMING").is_some() {
            eprintln!("gitlab timing {what}: {:?}", t0.elapsed());
        }
    };
    // Everything that doesn't depend on something else, in one go: against
    // a far-away GitLab that's one round trip instead of twenty. GitLab's
    // GraphQL (several reviewers?) beside it.
    let mrp = |s: &str| format!("projects/{project}/merge_requests/{iid}{s}");
    let pp = |s: &str| format!("projects/{project}/{s}");
    let (multiple, note_awards) = std::thread::scope(|s| {
        let multiple = s.spawn(|| c.allows_multiple(project, iid));
        let note_awards = s.spawn(|| c.note_awards(project, iid));
        c.prefetch([
        mrp("?include_diverged_commits_count=true"),
        first_page(&mrp("/diffs")),
        first_page(&mrp("/discussions")),
        mrp("/approvals"),
        first_page(&mrp("/commits")),
        first_page(&pp("members/all")),
        first_page(&mrp("/versions")),
        first_page(&pp("labels")),
        first_page(&pp("milestones?state=active")),
        first_page(&mrp("/award_emoji")),
        first_page(&mrp("/closes_issues")),
        first_page(&mrp("/draft_notes")),
        first_page(&mrp("/reviewers")),
        first_page(&mrp("/pipelines")),
        first_page(&mrp("/resource_state_events")),
        first_page(&mrp("/resource_label_events")),
        first_page(&mrp("/resource_milestone_events")),
        ]);
        (multiple.join().ok().and_then(Result::ok), note_awards.join().ok().and_then(Result::ok))
    });
    lap("batch");
    let r = load_prefetched(c, project, iid, multiple, &lap).map(|mut r| {
        r.note_awards = note_awards.unwrap_or_default();
        r
    });
    lap("done");
    c.forget_prefetched();
    r
}

fn load_prefetched(
    c: &api::Client,
    project: u64,
    iid: u64,
    multiple: Option<(bool, bool)>,
    lap: &dyn Fn(&str),
) -> anyhow::Result<review::Review> {
    let mr = c.merge_request(project, iid)?;
    let diffs = c.diffs(project, iid)?;
    let mut r = review::Review::build(
        mr,
        diffs,
        c.discussions(project, iid)?,
        c.approvals(project, iid)?,
        c.commits(project, iid)?,
    );
    // Role badges are a nicety: a token that can't list members still reviews.
    if let Ok((members, more)) = c.members_first(project) {
        r.roles = members.iter().filter_map(|m| Some((m.id, m.role()?))).collect();
        // members/all lists someone once per group they're in.
        let mut people: Vec<types::User> = vec![];
        for m in members.iter().filter(|m| m.state.is_empty() || m.state == "active") {
            if !people.iter().any(|p| p.id == m.id) {
                people.push(m.user());
            }
        }
        people.sort_by_key(|u| u.name.to_lowercase());
        r.members = people;
        r.members_more = more;
    }
    r.versions = c.versions(project, iid).unwrap_or_default();
    // The single merge request has label names only: colors from the project's.
    if let Ok(all) = c.labels(project) {
        for l in &mut r.mr.labels {
            if let Some(full) = all.iter().find(|a| a.name == l.name) {
                *l = full.clone();
            }
        }
        r.labels = all;
    }
    r.milestones = c.milestones(project).unwrap_or_default();
    r.awards = c.awards(project, iid).unwrap_or_default();
    r.closes = c.closes_issues(project, iid).unwrap_or_default();
    r.drafts = c.draft_notes(project, iid).unwrap_or_default();
    r.review_states = c.reviewers(project, iid).unwrap_or_default().into_iter().map(|rv| (rv.user.id, rv.state)).collect();
    r.pipelines = c.pipelines(project, iid).unwrap_or_default();
    if let Some(sha) = r.mr.merge_commit_sha.clone().or_else(|| r.mr.squash_commit_sha.clone()).filter(|_| r.mr.state == "merged") {
        r.merge_pipeline = c.pipeline_for_sha(project, &sha).ok().flatten();
    }
    // The roles of whoever commented that the first hundred members don't
    // cover: one short request each, together.
    let path = r.mr.references.full.split('!').next().unwrap_or_default().to_string();
    let authors: Vec<u64> = {
        let threads = r.general.iter().chain(&r.outdated).chain(r.files.iter().flat_map(|f| f.threads.iter().map(|(_, _, d)| d)));
        let mut ids: Vec<u64> = threads.flat_map(|d| d.notes.iter().map(|n| n.author.id)).chain([r.mr.author.id]).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.retain(|id| *id != 0 && !r.roles.contains_key(id));
        if r.members_more { ids } else { vec![] }
    };
    lap("built");
    c.prefetch(authors.iter().map(|id| api::member_path(project, *id)));
    let roles: Vec<(u64, &'static str)> = authors.iter().filter_map(|id| Some((*id, c.member(project, *id).ok()?.role()?))).collect();
    // Unknown (an old GitLab): let it pick several and show what GitLab kept.
    r.multiple = multiple.unwrap_or((true, true));
    r.roles.extend(roles);
    lap("roles");
    fill_in(c, &mut r, &path);
    lap("fill in");
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
    // Labels and milestones, worded as GitLab's activity has them.
    let labels = c.label_events(project, iid).unwrap_or_default();
    for (first, body) in label_notes(&labels) {
        r.system.push(types::Note {
            id: first.id,
            body,
            author: first.user.clone().unwrap_or_default(),
            created_at: first.created_at.clone(),
            system: true,
            ..Default::default()
        });
    }
    for e in c.milestone_events(project, iid).unwrap_or_default() {
        let Some(m) = &e.milestone else { continue };
        let title = review::token(&m.title);
        r.system.push(types::Note {
            id: e.id,
            body: if e.action == "remove" { format!("removed milestone %{title}") } else { format!("changed milestone to %{title}") },
            author: e.user.unwrap_or_default(),
            created_at: e.created_at,
            system: true,
            ..Default::default()
        });
    }
    Ok(r)
}

/// What GitLab leaves out of a big merge request's diff: files past its
/// 1000th (only GraphQL lists them) and files too big to send. They become
/// `Pending`, with GitLab's line counts, fetched when shown.
fn fill_in(c: &api::Client, r: &mut review::Review, full_path: &str) {
    use diff::Body;
    let capped = r.files.len() >= 1000 || r.mr.changes_count.as_deref().is_some_and(|n| n.ends_with('+'));
    let short = r.files.iter().any(|f| matches!(f.body, Body::TooLarge | Body::Collapsed));
    if !capped && !short {
        return;
    }
    let Ok(stats) = c.diff_stats(full_path, r.mr.iid) else { return };
    for f in &mut r.files {
        if matches!(f.body, Body::TooLarge | Body::Collapsed) {
            f.body = Body::Pending;
            f.stats = stats.iter().find(|s| s.0 == f.meta.new_path).map(|s| (s.1, s.2));
        }
    }
    let known: std::collections::HashSet<&str> =
        r.files.iter().flat_map(|f| [f.meta.new_path.as_str(), f.meta.old_path.as_str()]).collect();
    let missing: Vec<review::File> = stats
        .iter()
        .filter(|(path, _, _)| !known.contains(path.as_str()))
        .map(|(path, added, removed)| {
            let meta = types::FileDiff { old_path: path.clone(), new_path: path.clone(), ..Default::default() };
            let mut f = review::File::new(meta, Body::Pending);
            f.stats = Some((*added, *removed));
            f
        })
        .collect();
    r.files.extend(missing);
}

/// GitLab's label notes: one per person and moment, "added ~a ~b labels
/// and removed ~c label" (a label as `~name`, spaces kept by `token`).
fn label_notes(events: &[types::LabelEvent]) -> Vec<(&types::LabelEvent, String)> {
    let mut out: Vec<(&types::LabelEvent, Vec<String>, Vec<String>)> = vec![];
    for e in events {
        let Some(l) = &e.label else { continue };
        let who = e.user.as_ref().map(|u| u.id);
        let same = out.last().is_some_and(|(f, _, _)| f.user.as_ref().map(|u| u.id) == who && f.created_at.get(..19) == e.created_at.get(..19));
        if !same {
            out.push((e, vec![], vec![]));
        }
        let (_, added, removed) = out.last_mut().unwrap();
        let name = format!("~{}", review::token(&l.name));
        if e.action == "remove" { removed.push(name) } else { added.push(name) }
    }
    let part = |verb: &str, names: &[String]| {
        (!names.is_empty()).then(|| format!("{verb} {} label{}", names.join(" "), if names.len() == 1 { "" } else { "s" }))
    };
    out.into_iter()
        .map(|(first, added, removed)| {
            let parts: Vec<String> = [part("added", &added), part("removed", &removed)].into_iter().flatten().collect();
            (first, parts.join(" and "))
        })
        .collect()
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
    use super::{ListScope, ListSort, Location, MrState, Section, label_notes, types};
    use crate::types::{Label, LabelEvent, User};

    #[test]
    fn label_events_read_as_gitlab_words_them() {
        let ev = |id, user: u64, at: &str, name: &str, action: &str| LabelEvent {
            id,
            user: Some(User { id: user, ..Default::default() }),
            created_at: at.into(),
            label: Some(Label { name: name.into(), ..Default::default() }),
            action: action.into(),
        };
        let events = [
            ev(1, 1, "2026-11-06T06:00:00.100Z", "bug", "add"),
            ev(2, 1, "2026-11-06T06:00:00.200Z", "docs", "add"),
            // Another moment: its own note, a label with a space kept whole.
            ev(3, 2, "2026-11-06T06:05:00.000Z", "backend", "add"),
            ev(4, 2, "2026-11-06T06:05:00.000Z", "needs review", "add"),
            ev(5, 2, "2026-11-06T06:05:00.000Z", "bug", "remove"),
            ev(6, 2, "2026-11-06T06:09:00.000Z", "docs", "remove"),
        ];
        let notes: Vec<(u64, String)> = label_notes(&events).into_iter().map(|(e, b)| (e.id, b)).collect();
        assert_eq!(
            notes,
            [
                (1, "added ~bug ~docs labels".to_string()),
                (3, "added ~backend ~needs\u{a0}review labels and removed ~bug label".to_string()),
                (6, "removed ~docs label".to_string()),
            ]
        );
    }

    #[test]
    fn list_sorts() {
        let mr = |title: &str, created: &str, up: u32| types::MergeRequest {
            title: title.into(),
            created_at: created.into(),
            updated_at: created.into(),
            upvotes: up,
            ..Default::default()
        };
        let all = [mr("b", "2026-01-02", 0), mr("a", "2026-01-03", 2), mr("c", "2026-01-01", 1)];
        let order = |s: ListSort, asc: bool| {
            let mut v: Vec<&types::MergeRequest> = all.iter().collect();
            s.apply(&mut v, asc);
            v.iter().map(|m| m.title.as_str()).collect::<Vec<_>>().join("")
        };
        assert_eq!(order(ListSort::Created, false), "abc");
        assert_eq!(order(ListSort::Created, true), "cba");
        assert_eq!(order(ListSort::Title, false), "cba");
        assert_eq!(order(ListSort::Title, true), "abc");
        assert_eq!(order(ListSort::Popularity, false), "acb");
    }

    #[test]
    fn section_from_url() {
        let mr = "http://gl.test/a/b/-/merge_requests/1";
        assert_eq!(Section::of_url(mr), Section::Overview);
        assert_eq!(Section::of_url(&format!("{mr}/diffs?file=x#note_1")), Section::Changes);
        assert_eq!(Section::of_url(&format!("{mr}/commits")), Section::Commits);
        assert_eq!(Section::of_url(&format!("{mr}/pipelines")), Section::Pipelines);
        assert_eq!(Section::of_url(&format!("{mr}#note_3")), Section::Overview);
        // A project named "diffs" isn't the Changes tab.
        assert_eq!(Section::of_url("http://gl.test/diffs/-/merge_requests/2"), Section::Overview);
    }

    #[test]
    fn location_round_trips_and_tolerates_junk() {
        let loc = Location {
            host: Some("http://gl.test".into()),
            scope: ListScope::All,
            browse: Some(super::Browse { id: 7, path: "g/p".into() }),
            state: MrState::Merged,
            sort: ListSort::Title,
            ascending: true,
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
