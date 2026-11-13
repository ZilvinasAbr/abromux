//! What the cache holds about a merge request, and which ones it keeps.
//!
//! A merge request is kept while something wants it:
//! - **pinned**: its source branch is checked out in an open workspace;
//! - **mine**: it's open and yours (authored, or asked to review / assigned,
//!   as the settings say);
//! - **recent**: you opened it in a GitLab tab, among the last
//!   `recentLimit` you opened;
//! - **open**: a GitLab tab shows it.
//!
//! Merged and closed ones go at once (unless a tab still shows them).
//! What a tab shows of one (its [`Review`](abromux_gitlab::review::Review))
//! is recorded on disk by `abromux_gitlab::cache`; past the space limit the
//! recordings of the longest-unopened ones go first, and they stay listed.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// A merge request on the configured GitLab: its project's full path and
/// its number there.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MrKey {
    pub project: String,
    pub iid: u64,
}

impl MrKey {
    pub fn new(project: impl Into<String>, iid: u64) -> Self {
        Self {
            project: project.into(),
            iid,
        }
    }

    /// `group/project!12`, GitLab's own way to write it.
    pub fn reference(&self) -> String {
        format!("{}!{}", self.project, self.iid)
    }

    /// `https://host/group/project/-/merge_requests/12[/…]`, or
    /// `group/project!12`.
    pub fn parse(s: &str) -> Option<MrKey> {
        let s = s.trim();
        if let Some((project, iid)) = s.rsplit_once('!')
            && !project.contains("://")
        {
            let project = project.trim_matches('/');
            return (!project.is_empty() && project.contains('/'))
                .then(|| Some(MrKey::new(project, iid.trim().parse().ok()?)))
                .flatten();
        }
        if !(s.starts_with("https://") || s.starts_with("http://")) {
            return None;
        }
        // The key leaves the host out: one GitLab is set up at a time.
        let (_, project, iid) = abromux_gitlab::api::parse_mr_url(s)?;
        Some(MrKey::new(&project, iid))
    }

    /// The project's last path segment.
    pub fn project_name(&self) -> &str {
        self.project.rsplit('/').next().unwrap_or(&self.project)
    }
}

/// What the frequent, cheap check returns for a merge request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Light {
    pub title: String,
    pub web_url: String,
    /// `opened`, `merged`, `closed` or `locked`.
    pub state: String,
    pub draft: bool,
    pub author: String,
    pub source_branch: String,
    pub target_branch: String,
    /// GitLab's ISO 8601 time; a change means there's more to fetch.
    pub updated_at: String,
    /// The head pipeline's status, lowercase (`running`, `success`, …).
    pub pipeline: Option<String>,
    /// GitLab's detailed merge status, lowercase (`mergeable`, …).
    pub merge_status: Option<String>,
    pub approved: Option<bool>,
    pub notes: u64,
    pub head_sha: Option<String>,
    pub base_sha: Option<String>,
    pub start_sha: Option<String>,
    /// GitLab's id of its project (0: not known).
    #[serde(default)]
    pub project_id: u64,
}

impl Light {
    pub fn is_open(&self) -> bool {
        self.state == "opened" || self.state.is_empty()
    }

    /// What says whether a recorded review is still this one
    /// (`abromux_gitlab::cache::signature`).
    pub fn signature(&self) -> String {
        abromux_gitlab::cache::signature(
            &self.updated_at,
            self.head_sha.as_deref().unwrap_or_default(),
            self.pipeline.as_deref().unwrap_or_default(),
            self.notes,
        )
    }

    /// From a loaded review: what the cheap check would say now. GitLab's
    /// REST has no `approved`, so `before`'s stays.
    pub fn of_review(r: &abromux_gitlab::review::Review, before: Option<&Light>) -> Light {
        let mr = &r.mr;
        let refs = mr.diff_refs.as_ref();
        Light {
            title: mr.title.clone(),
            web_url: mr.web_url.clone(),
            state: mr.state.clone(),
            draft: mr.draft,
            author: mr.author.username.clone(),
            source_branch: mr.source_branch.clone(),
            target_branch: mr.target_branch.clone(),
            updated_at: mr.updated_at.clone(),
            pipeline: mr
                .head_pipeline
                .as_ref()
                .map(|p| p.status.to_ascii_lowercase()),
            merge_status: mr
                .detailed_merge_status
                .as_ref()
                .map(|s| s.to_ascii_lowercase()),
            approved: Some(r.approvals.approved).or(before.and_then(|b| b.approved)),
            notes: mr.user_notes_count as u64,
            head_sha: refs.map(|d| d.head_sha.clone()),
            base_sha: refs.map(|d| d.base_sha.clone()),
            start_sha: refs.map(|d| d.start_sha.clone()),
            project_id: mr.project_id,
        }
    }

    /// Its pipeline is still going, so it's checked more often.
    pub fn pipeline_active(&self) -> bool {
        matches!(
            self.pipeline.as_deref(),
            Some(
                "running"
                    | "pending"
                    | "created"
                    | "preparing"
                    | "waiting_for_resource"
                    | "scheduled"
            )
        )
    }
}

/// Its review as recorded on disk (`abromux_gitlab::cache`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recorded {
    /// [`Light::signature`] of what was recorded.
    pub signature: String,
    pub bytes: u64,
}

/// Which of "mine" it's in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Roles {
    pub authored: bool,
    pub review: bool,
    pub assigned: bool,
}

impl Roles {
    pub fn any(self) -> bool {
        self.authored || self.review || self.assigned
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mr {
    pub key: MrKey,
    /// None until first fetched (opened from a link, say).
    #[serde(default)]
    pub light: Option<Light>,
    /// Its review on disk, if recorded.
    #[serde(default)]
    pub review: Option<Recorded>,
    /// The signature whose review was dropped for space: it isn't fetched
    /// again on its own, only when the merge request is opened.
    #[serde(default)]
    pub review_skip: Option<String>,
    /// Seconds since the epoch of the last fetch that succeeded.
    #[serde(default)]
    pub fetched_at: u64,
    /// When it was last opened in a GitLab tab; Some = in "recent".
    #[serde(default)]
    pub inspected_at: Option<u64>,
    #[serde(default)]
    pub roles: Roles,
    /// GitLab no longer has it (or it can't be seen).
    #[serde(default)]
    pub gone: bool,
}

impl Mr {
    pub fn new(key: MrKey) -> Self {
        Self {
            key,
            light: None,
            review: None,
            review_skip: None,
            fetched_at: 0,
            inspected_at: None,
            roles: Roles::default(),
            gone: false,
        }
    }

    pub fn title(&self) -> &str {
        self.light.as_ref().map_or("", |l| l.title.as_str())
    }

    /// GitLab's id of its project, if known.
    pub fn project_id(&self) -> Option<u64> {
        self.light
            .as_ref()
            .map(|l| l.project_id)
            .filter(|id| *id != 0)
    }

    /// The recorded review isn't what the cheap check last saw.
    pub fn review_stale(&self) -> bool {
        let Some(l) = &self.light else { return false };
        self.review
            .as_ref()
            .is_none_or(|r| r.signature != l.signature())
    }

    /// Its review should be fetched without anyone asking.
    pub fn wants_review(&self) -> bool {
        self.review_stale() && self.review_skip != self.light.as_ref().map(Light::signature)
    }

    fn closed(&self) -> bool {
        self.gone || self.light.as_ref().is_some_and(|l| !l.is_open())
    }
}

pub struct Keep<'a> {
    pub pinned: &'a HashSet<MrKey>,
    /// Shown in a view somewhere.
    pub open: &'a HashSet<MrKey>,
    pub recent_limit: usize,
    pub cache_bytes: u64,
}

/// Drop what nothing keeps any more (see the module docs). Returns the
/// recorded reviews to delete from disk: (key, project id).
pub fn evict(mrs: &mut BTreeMap<MrKey, Mr>, keep: &Keep) -> Vec<(MrKey, Option<u64>)> {
    let mut dropped = vec![];
    // Merged and closed ones, and ones past the recent limit.
    let mut recent: Vec<(u64, MrKey)> = mrs
        .values()
        .filter_map(|m| m.inspected_at.map(|t| (t, m.key.clone())))
        .collect();
    recent.sort_by(|a, b| b.cmp(a));
    for (_, k) in recent.into_iter().skip(keep.recent_limit) {
        if let Some(m) = mrs.get_mut(&k) {
            m.inspected_at = None;
        }
    }
    mrs.retain(|k, m| {
        let shown = keep.open.contains(k);
        let wanted = keep.pinned.contains(k) || m.roles.any() || m.inspected_at.is_some() || shown;
        if (m.closed() && !shown) || !wanted {
            dropped.push((k.clone(), m.project_id()));
            false
        } else {
            true
        }
    });
    // Disk space: recent-only ones before yours, longest-unopened first.
    let mut total: u64 = mrs
        .values()
        .filter_map(|m| m.review.as_ref())
        .map(|d| d.bytes)
        .sum();
    if total > keep.cache_bytes {
        let mut order: Vec<(bool, u64, String, MrKey)> = mrs
            .values()
            .filter(|m| m.review.is_some() && !keep.open.contains(&m.key))
            .map(|m| {
                let kept = keep.pinned.contains(&m.key) || m.roles.any();
                let updated = m
                    .light
                    .as_ref()
                    .map(|l| l.updated_at.clone())
                    .unwrap_or_default();
                (kept, m.inspected_at.unwrap_or(0), updated, m.key.clone())
            })
            .collect();
        order.sort();
        for (.., k) in order {
            if total <= keep.cache_bytes {
                break;
            }
            let m = mrs.get_mut(&k).expect("listed above");
            let d = m.review.take().expect("listed above");
            total -= d.bytes;
            m.review_skip = Some(d.signature);
            dropped.push((k, m.project_id()));
        }
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mr(project: &str, iid: u64) -> Mr {
        let mut m = Mr::new(MrKey::new(project, iid));
        m.light = Some(Light {
            state: "opened".into(),
            head_sha: Some("h".into()),
            base_sha: Some("b".into()),
            ..Default::default()
        });
        m
    }

    fn with_diff(mut m: Mr, bytes: u64) -> Mr {
        m.review = Some(Recorded {
            signature: m.light.as_ref().unwrap().signature(),
            bytes,
        });
        m
    }

    fn map(ms: Vec<Mr>) -> BTreeMap<MrKey, Mr> {
        ms.into_iter().map(|m| (m.key.clone(), m)).collect()
    }

    fn keep<'a>(pinned: &'a HashSet<MrKey>, open: &'a HashSet<MrKey>) -> Keep<'a> {
        Keep {
            pinned,
            open,
            recent_limit: 2,
            cache_bytes: 100,
        }
    }

    #[test]
    fn parses_urls_and_references() {
        assert_eq!(
            MrKey::parse("https://gitlab.com/g/sub/p/-/merge_requests/42/diffs?x=1"),
            Some(MrKey::new("g/sub/p", 42))
        );
        assert_eq!(MrKey::parse("g/p!7"), Some(MrKey::new("g/p", 7)));
        assert_eq!(MrKey::parse(" g/p!7 "), Some(MrKey::new("g/p", 7)));
        assert_eq!(MrKey::parse("p!7"), None);
        assert_eq!(MrKey::parse("https://gitlab.com/g/p/-/issues/3"), None);
        assert_eq!(MrKey::parse("hello"), None);
    }

    #[test]
    fn drops_closed_unless_shown() {
        let mut a = mr("g/a", 1);
        a.roles.authored = true;
        a.light.as_mut().unwrap().state = "merged".into();
        let mut b = mr("g/b", 2);
        b.roles.authored = true;
        b.gone = true;
        let mut mrs = map(vec![with_diff(a, 10), b]);
        let pinned = HashSet::new();
        let open: HashSet<_> = [MrKey::new("g/b", 2)].into();
        let dropped = evict(&mut mrs, &keep(&pinned, &open));
        assert_eq!(
            mrs.keys().cloned().collect::<Vec<_>>(),
            vec![MrKey::new("g/b", 2)]
        );
        assert_eq!(dropped, vec![(MrKey::new("g/a", 1), None)]);
    }

    #[test]
    fn caps_recent_newest_first_but_keeps_mine() {
        let mut ms = vec![];
        for (i, t) in [(1, 10), (2, 30), (3, 20), (4, 5)] {
            let mut m = mr("g/p", i);
            m.inspected_at = Some(t);
            ms.push(m);
        }
        ms[3].roles.review = true;
        let mut mrs = map(ms);
        let none = HashSet::new();
        evict(&mut mrs, &keep(&none, &none));
        let left: Vec<u64> = mrs.keys().map(|k| k.iid).collect();
        // 2 and 3 are the newest two; 4 is out of recent but is mine.
        assert_eq!(left, vec![2, 3, 4]);
        assert_eq!(mrs[&MrKey::new("g/p", 4)].inspected_at, None);
    }

    #[test]
    fn drops_diffs_for_space_oldest_recent_first() {
        let mut old = with_diff(mr("g/p", 1), 60);
        old.inspected_at = Some(1);
        let mut newer = with_diff(mr("g/p", 2), 60);
        newer.inspected_at = Some(2);
        let mut mine = with_diff(mr("g/p", 3), 60);
        mine.roles.authored = true;
        let mut mrs = map(vec![old, newer, mine]);
        let none = HashSet::new();
        let k = Keep {
            recent_limit: 10,
            ..keep(&none, &none)
        };
        let dropped = evict(&mut mrs, &k);
        let gone: Vec<u64> = dropped.iter().map(|(k, _)| k.iid).collect();
        assert_eq!(gone, vec![1, 2]);
        // Details stay, and the dropped head isn't refetched on its own.
        assert_eq!(mrs.len(), 3);
        assert!(!mrs[&MrKey::new("g/p", 1)].wants_review());
        assert!(mrs[&MrKey::new("g/p", 3)].review.is_some());
    }

    #[test]
    fn stale_flags() {
        let mut m = mr("g/p", 1);
        assert!(m.review_stale());
        assert!(m.wants_review());
        let mut m = with_diff(m, 1);
        assert!(!m.review_stale());
        m.light.as_mut().unwrap().updated_at = "2026-11-06T10:00:00Z".into();
        assert!(m.review_stale());
        m.review_skip = Some(m.light.as_ref().unwrap().signature());
        assert!(!m.wants_review());
    }
}
