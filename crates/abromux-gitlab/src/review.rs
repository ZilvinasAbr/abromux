//! One merge request put together for review: its files' diffs with each
//! comment thread attached to the line it's on.

use crate::{
    diff::{Body, Kind},
    types::*,
};
use serde_json::{Value, json};

/// A name as one word of a system note (spaces as no-break spaces), so
/// `~my label` stays one label; `untoken` turns it back.
pub fn token(name: &str) -> String {
    name.replace(' ', "\u{a0}")
}

pub fn untoken(word: &str) -> String {
    word.replace('\u{a0}', " ")
}

#[derive(Clone, Debug)]
pub struct Review {
    pub mr: MergeRequest,
    pub files: Vec<File>,
    /// Comments on the merge request itself, not on a line.
    pub general: Vec<Discussion>,
    /// On a line of a file that isn't in the diff any more.
    pub outdated: Vec<Discussion>,
    /// GitLab's own notes ("requested review from @bob"), for the activity.
    pub system: Vec<Note>,
    pub approvals: Approvals,
    pub commits: Vec<Commit>,
    /// Role badges by user id ("Developer", "Owner"), when known.
    pub roles: std::collections::HashMap<u64, &'static str>,
    /// The merge request's versions, newest first.
    pub versions: Vec<MrVersion>,
    /// Who can be picked as an assignee or reviewer, by name: the first
    /// hundred, `members_more` when there are more (the picker searches).
    pub members: Vec<User>,
    pub members_more: bool,
    /// The project's labels and open milestones, for the sidebar's pickers.
    pub labels: Vec<Label>,
    pub milestones: Vec<Milestone>,
    /// Emoji reactions on the merge request.
    pub awards: Vec<AwardEmoji>,
    /// Emoji reactions on comments, by note id (only GraphQL tells; the
    /// reactions' own ids aren't known).
    pub note_awards: std::collections::HashMap<u64, Vec<AwardEmoji>>,
    /// The merge request's pipelines, newest first, for each commit's status.
    pub pipelines: Vec<Pipeline>,
    /// Merged: the pipeline on the merge (or squash) commit.
    pub merge_pipeline: Option<Pipeline>,
    /// Each reviewer's review state by user id, when GitLab says.
    pub review_states: std::collections::HashMap<u64, String>,
    /// GitLab's "Work items": the issues it closes.
    pub closes: Vec<Issue>,
    /// Your review's pending comments.
    pub drafts: Vec<DraftNote>,
    /// Several assignees, several reviewers (GitLab's paid tiers), or one.
    pub multiple: (bool, bool),
}

/// A file fetched after GitLab left it out: its diff, and whether it was
/// added or deleted (only the fetch tells, for a file GitLab didn't list).
#[derive(Clone, Debug)]
pub struct Late {
    pub body: Body,
    pub status: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct File {
    pub meta: FileDiff,
    /// What GitLab sent; `body()` is what to show.
    pub body: Body,
    /// A `Pending` file's body once fetched, shared by the review's copies.
    pub late: std::sync::Arc<std::sync::OnceLock<Late>>,
    /// Lines added and removed, from GitLab's stats, for a file not yet fetched.
    pub stats: Option<(usize, usize)>,
    /// `counts()` once the body is final: the toolbar sums every file's
    /// each frame, and a big file has tens of thousands of lines.
    counted: std::sync::Arc<std::sync::OnceLock<(usize, usize)>>,
    /// Threads on a line: (hunk index, line index in it, thread).
    pub threads: Vec<(usize, usize, Discussion)>,
    /// On this file, but on a line no longer in the diff.
    pub outdated: Vec<Discussion>,
}

impl File {
    pub fn new(meta: FileDiff, body: Body) -> File {
        File { meta, body, late: Default::default(), stats: None, counted: Default::default(), threads: vec![], outdated: vec![] }
    }

    /// The diff to show: fetched late, or as GitLab sent it.
    pub fn body(&self) -> &Body {
        self.late.get().map_or(&self.body, |l| &l.body)
    }

    /// Fetched or not, lines added and removed.
    pub fn counts(&self) -> (usize, usize) {
        match (self.body(), self.stats) {
            (Body::Pending, Some(s)) => s,
            (Body::Pending, None) => (0, 0),
            (b, _) => *self.counted.get_or_init(|| b.counts()),
        }
    }

    pub fn path(&self) -> &str {
        if self.meta.deleted_file {
            &self.meta.old_path
        } else {
            &self.meta.new_path
        }
    }

    /// added, deleted, renamed or modified
    pub fn status(&self) -> &'static str {
        if let Some(s) = self.late.get().and_then(|l| l.status) {
            return s;
        }
        match &self.meta {
            m if m.new_file => "added",
            m if m.deleted_file => "deleted",
            m if m.renamed_file => "renamed",
            _ => "modified",
        }
    }

    fn holds(&self, p: &Position) -> bool {
        p.new_path.as_deref() == Some(&self.meta.new_path)
            || p.old_path.as_deref() == Some(&self.meta.old_path)
    }

    /// The (hunk, line) a position points at. GitLab gives a context line
    /// both numbers, an added line only its new one, a removed line only
    /// its old one.
    fn find(&self, p: &Position) -> Option<(usize, usize)> {
        for (hi, h) in self.body().hunks().iter().enumerate() {
            for (li, l) in h.lines.iter().enumerate() {
                let hit = match (p.old_line, p.new_line) {
                    (Some(o), Some(n)) => l.old == Some(o) && l.new == Some(n),
                    (None, Some(n)) => l.kind == Kind::Added && l.new == Some(n),
                    (Some(o), None) => l.kind == Kind::Removed && l.old == Some(o),
                    (None, None) => false,
                };
                if hit {
                    return Some((hi, li));
                }
            }
        }
        None
    }
}

impl Review {
    pub fn build(
        mr: MergeRequest,
        diffs: Vec<FileDiff>,
        discussions: Vec<Discussion>,
        approvals: Approvals,
        commits: Vec<Commit>,
    ) -> Review {
        let mut files: Vec<File> = diffs
            .into_iter()
            .map(|meta| {
                let body = Body::of(&meta);
                File::new(meta, body)
            })
            .collect();
        let (mut general, mut outdated, mut system) = (vec![], vec![], vec![]);
        for d in discussions {
            if d.is_system() {
                system.extend(d.notes);
                continue;
            }
            let Some(pos) = d.position().cloned() else {
                general.push(d);
                continue;
            };
            match files.iter_mut().find(|f| f.holds(&pos)) {
                Some(f) => match f.find(&pos) {
                    Some((h, l)) => f.threads.push((h, l, d)),
                    None => f.outdated.push(d),
                },
                None => outdated.push(d),
            }
        }
        for f in &mut files {
            f.threads.sort_by_key(|(h, l, _)| (*h, *l));
        }
        Review {
            mr,
            files,
            general,
            outdated,
            system,
            approvals,
            commits,
            roles: Default::default(),
            versions: vec![],
            members: vec![],
            members_more: false,
            labels: vec![],
            milestones: vec![],
            awards: vec![],
            note_awards: Default::default(),
            pipelines: vec![],
            merge_pipeline: None,
            review_states: Default::default(),
            closes: vec![],
            drafts: vec![],
            multiple: (true, true),
        }
    }

    /// Unresolved threads, of those that can be resolved.
    pub fn unresolved(&self) -> usize {
        let on_lines = self.files.iter().flat_map(|f| {
            f.threads.iter().map(|(_, _, d)| d).chain(&f.outdated)
        });
        on_lines
            .chain(&self.general)
            .chain(&self.outdated)
            .filter(|d| d.resolvable() && !d.resolved())
            .count()
    }

    /// Whether any thread can be resolved: GitLab shows its threads counter
    /// only then.
    pub fn has_resolvable(&self) -> bool {
        let on_lines = self.files.iter().flat_map(|f| f.threads.iter().map(|(_, _, d)| d).chain(&f.outdated));
        on_lines.chain(&self.general).chain(&self.outdated).any(|d| d.resolvable())
    }

    /// A summary for `abromux --json tree` and tests: what a reviewer would
    /// see, without the diff text.
    pub fn summary(&self) -> Value {
        let thread = |d: &Discussion| {
            let n = &d.notes[0];
            let line = n.position.as_ref().map(|p| json!({ "old": p.old_line, "new": p.new_line }));
            json!({
                "author": n.author.username,
                "notes": d.notes.len(),
                "resolved": d.resolved(),
                "line": line,
                "body": n.body.lines().next().unwrap_or_default(),
            })
        };
        json!({
            "ref": self.mr.references.full,
            "title": self.mr.title,
            "draft": self.mr.draft,
            "author": self.mr.author.username,
            "approved_by": self.approvals.approved_by.iter().map(|a| &a.user.username).collect::<Vec<_>>(),
            "unresolved": self.unresolved(),
            "commits": self.commits.len(),
            "general": self.general.iter().map(thread).collect::<Vec<_>>(),
            "outdated": self.outdated.iter().map(thread).collect::<Vec<_>>(),
            "files": self.files.iter().map(|f| {
                let (added, removed) = f.counts();
                json!({
                    "path": f.path(),
                    "old_path": (f.meta.old_path != f.meta.new_path).then_some(&f.meta.old_path),
                    "status": f.status(),
                    "body": match f.body {
                        Body::Text(_) => "text",
                        Body::Binary => "binary",
                        Body::Unchanged => "unchanged",
                        Body::TooLarge => "too_large",
                        Body::Collapsed => "collapsed",
                        Body::Pending => "pending",
                    },
                    "added": added,
                    "removed": removed,
                    "threads": f.threads.iter().map(|(_, _, d)| thread(d)).collect::<Vec<_>>(),
                    "outdated": f.outdated.iter().map(thread).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        })
    }
}
