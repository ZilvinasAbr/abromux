//! What a write will do, shown before GitLab says it did: each write's
//! [`Patch`] is applied to the review on screen at once, and again to every
//! review fetched while it's under way, until one fetched after it landed
//! has it for real. A write that fails takes its patch back.
//!
//! Comments and threads made here have stand-in ids ([`is_pending_note`],
//! [`is_pending_thread`]) and are drawn faded until GitLab's own copy
//! replaces them; nothing can be done to them meanwhile.
//!
//! Patches are idempotent: applied to a review that already has the write
//! (a fetch that raced it), they leave it as it is.

use crate::{review::Review, types::*};
use serde_json::Value;

/// Stand-in note ids count up from here: no GitLab gets near them, and
/// the view's element ids (`id * 64 + part`) still fit.
const NOTE_BASE: u64 = 1 << 52;
const THREAD_PREFIX: &str = "optimistic-";

pub fn is_pending_note(id: u64) -> bool {
    id >= NOTE_BASE
}

pub fn is_pending_thread(id: &str) -> bool {
    id.starts_with(THREAD_PREFIX)
}

/// One write's effect on the review.
#[derive(Clone, Debug)]
pub enum Patch {
    /// A comment on the merge request.
    Comment { body: String },
    /// A reply in `thread`, and with `resolve`, resolving it.
    Reply { thread: String, body: String, resolve: bool },
    Resolve { thread: String, resolved: bool },
    Approve { approve: bool },
    /// A reaction on the merge request, added or taken back.
    Award { name: String, add: bool },
    /// A reaction on comment `note`.
    NoteAward { note: u64, name: String, add: bool },
    /// A new thread on a line of file `file`.
    LineComment { file: usize, old: Option<u32>, new: Option<u32>, body: String },
    /// A comment into your review (GitLab's "Pending").
    Draft { body: String },
    /// The merge request's fields as PUT (assignee_ids, reviewer_ids,
    /// labels, remove_labels, milestone_id, title); `people`: who the ids
    /// may name, beyond those the review knows.
    Fields { fields: Value, people: Vec<User> },
}

impl Patch {
    /// Apply it to `r` as `me`; `n` makes its stand-in ids unique.
    pub fn apply(&self, r: &mut Review, me: &User, n: u64) {
        let now = crate::time::iso(crate::time::now());
        let note = |body: &str, kind: Option<&str>, resolvable: bool| Note {
            id: NOTE_BASE + n,
            kind: kind.map(str::to_string),
            body: body.to_string(),
            author: me.clone(),
            created_at: now.clone(),
            resolvable,
            ..Default::default()
        };
        let mine = |n: &Note, body: &str| n.author.id == me.id && n.body.trim() == body.trim();
        match self {
            Patch::Comment { body } => {
                if !r.general.iter().flat_map(|d| &d.notes).any(|n| mine(n, body)) {
                    r.general.push(Discussion {
                        id: format!("{THREAD_PREFIX}{n}"),
                        individual_note: true,
                        notes: vec![note(body, None, false)],
                    });
                    r.mr.user_notes_count += 1;
                }
            }
            Patch::Reply { thread, body, resolve } => {
                if let Some(d) = thread_mut(r, thread) {
                    if !d.notes.iter().any(|n| mine(n, body)) {
                        let resolvable = d.resolvable();
                        let mut reply = note(body, Some(d.notes[0].kind.as_deref().unwrap_or("DiscussionNote")), resolvable);
                        reply.position = d.notes[0].position.clone();
                        d.notes.push(reply);
                        d.individual_note = false;
                    }
                    if *resolve {
                        set_resolved(d, true, me, &now);
                    }
                }
            }
            Patch::Resolve { thread, resolved } => {
                if let Some(d) = thread_mut(r, thread) {
                    set_resolved(d, *resolved, me, &now);
                }
            }
            Patch::Approve { approve } => {
                let has = r.approvals.approved_by.iter().any(|a| a.user.id == me.id);
                if *approve && !has {
                    r.approvals.approved_by.push(Approver { user: me.clone() });
                    if r.mr.reviewers.iter().any(|u| u.id == me.id) {
                        r.review_states.insert(me.id, "approved".into());
                    }
                } else if !*approve && has {
                    r.approvals.approved_by.retain(|a| a.user.id != me.id);
                    if r.review_states.get(&me.id).is_some_and(|s| s == "approved") {
                        r.review_states.insert(me.id, "unreviewed".into());
                    }
                }
            }
            Patch::Award { name, add } => award(&mut r.awards, name, *add, me, n),
            Patch::NoteAward { note, name, add } => {
                let list = r.note_awards.entry(*note).or_default();
                award(list, name, *add, me, n);
                if list.is_empty() {
                    r.note_awards.remove(note);
                }
            }
            Patch::LineComment { file, old, new, body } => {
                let refs = r.mr.diff_refs.clone().unwrap_or_default();
                let Some(f) = r.files.get_mut(*file) else { return };
                let on_it = f.threads.iter().map(|(_, _, d)| d).chain(&f.outdated);
                if on_it.flat_map(|d| &d.notes).any(|n| mine(n, body)) {
                    return;
                }
                let pos = Position {
                    base_sha: refs.base_sha,
                    start_sha: refs.start_sha,
                    head_sha: refs.head_sha,
                    old_path: Some(f.meta.old_path.clone()),
                    new_path: Some(f.meta.new_path.clone()),
                    position_type: "text".into(),
                    old_line: *old,
                    new_line: *new,
                };
                let Some((h, l)) = f.find(&pos) else { return };
                let mut first = note(body, Some("DiffNote"), true);
                first.position = Some(pos);
                let d = Discussion { id: format!("{THREAD_PREFIX}{n}"), individual_note: false, notes: vec![first] };
                let at = f.threads.partition_point(|(fh, fl, _)| (*fh, *fl) <= (h, l));
                f.threads.insert(at, (h, l, d));
            }
            Patch::Draft { body } => {
                if !r.drafts.iter().any(|d| d.note.trim() == body.trim()) {
                    r.drafts.push(DraftNote { id: NOTE_BASE + n, note: body.clone() });
                }
            }
            Patch::Fields { fields, people } => apply_fields(r, fields, people, me),
        }
    }
}

/// Thread `id`, wherever it is.
fn thread_mut<'a>(r: &'a mut Review, id: &str) -> Option<&'a mut Discussion> {
    let on_files = r.files.iter_mut().flat_map(|f| f.threads.iter_mut().map(|(_, _, d)| d).chain(f.outdated.iter_mut()));
    r.general.iter_mut().chain(r.outdated.iter_mut()).chain(on_files).find(|d| d.id == id)
}

fn set_resolved(d: &mut Discussion, resolved: bool, me: &User, now: &str) {
    for n in d.notes.iter_mut().filter(|n| n.resolvable) {
        if n.resolved != resolved {
            n.resolved = resolved;
            n.resolved_by = resolved.then(|| me.clone());
            n.resolved_at = resolved.then(|| now.to_string());
        }
    }
}

fn award(list: &mut Vec<AwardEmoji>, name: &str, add: bool, me: &User, n: u64) {
    let has = list.iter().any(|a| a.name == name && a.user.id == me.id);
    if add && !has {
        list.push(AwardEmoji { id: NOTE_BASE + n, name: name.to_string(), user: me.clone() });
    } else if !add {
        list.retain(|a| !(a.name == name && a.user.id == me.id));
    }
}

fn apply_fields(r: &mut Review, fields: &Value, people: &[User], me: &User) {
    let known: Vec<User> = people
        .iter()
        .chain(&r.members)
        .chain(&r.mr.assignees)
        .chain(&r.mr.reviewers)
        .chain([me])
        .cloned()
        .collect();
    let users = |v: &Value| -> Option<Vec<User>> {
        let ids = v.as_array()?;
        // Someone not known here: leave it to the refresh.
        ids.iter().map(|id| known.iter().find(|u| Some(u.id) == id.as_u64()).cloned()).collect()
    };
    if let Some(us) = fields.get("assignee_ids").and_then(users) {
        r.mr.assignees = us;
    }
    if let Some(us) = fields.get("reviewer_ids").and_then(users) {
        r.mr.reviewers = us;
    }
    let label = |name: &str| r.labels.iter().find(|l| l.name == name).cloned().unwrap_or(Label { name: name.to_string(), ..Default::default() });
    if let Some(names) = fields.get("labels").and_then(Value::as_str) {
        r.mr.labels = names.split(',').map(str::trim).filter(|s| !s.is_empty()).map(label).collect();
    }
    if let Some(names) = fields.get("remove_labels").and_then(Value::as_str) {
        let gone: Vec<&str> = names.split(',').map(str::trim).collect();
        r.mr.labels.retain(|l| !gone.contains(&l.name.as_str()));
    }
    if let Some(id) = fields.get("milestone_id").and_then(Value::as_u64) {
        r.mr.milestone = if id == 0 { None } else { r.milestones.iter().find(|m| m.id == id).cloned().or(r.mr.milestone.take()) };
    }
    if let Some(t) = fields.get("title").and_then(Value::as_str) {
        r.mr.title = t.to_string();
        r.mr.draft = ["Draft:", "[Draft]", "(Draft)", "Draft "].iter().any(|p| t.starts_with(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::Review;
    use serde_json::json;

    fn user(id: u64) -> User {
        User { id, username: format!("u{id}"), name: format!("U{id}"), ..Default::default() }
    }

    fn review() -> Review {
        let diff = "@@ -1,2 +1,2 @@\n a\n-b\n+c\n";
        let meta = FileDiff { old_path: "f".into(), new_path: "f".into(), diff: diff.into(), ..Default::default() };
        let thread = Discussion {
            id: "t1".into(),
            notes: vec![Note { id: 1, body: "hm".into(), author: user(2), resolvable: true, ..Default::default() }],
            ..Default::default()
        };
        let mut r = Review::build(
            MergeRequest { diff_refs: Some(DiffRefs::default()), ..Default::default() },
            vec![meta],
            vec![thread],
            Approvals::default(),
            vec![],
        );
        r.members = vec![user(1), user(2), user(3)];
        r.labels = vec![Label { name: "bug".into(), color: "#f00".into(), ..Default::default() }];
        r
    }

    #[test]
    fn a_comment_shows_once() {
        let mut r = review();
        let p = Patch::Comment { body: "hi".into() };
        p.apply(&mut r, &user(1), 1);
        p.apply(&mut r, &user(1), 2);
        assert_eq!(r.general.len(), 2);
        let d = r.general.last().unwrap();
        assert!(is_pending_thread(&d.id) && is_pending_note(d.notes[0].id));
        assert_eq!(d.notes[0].author.id, 1);
    }

    /// The view keys a note's parts as `id * 64 + part` (discussion.rs):
    /// a stand-in id must leave room for that.
    #[test]
    fn stand_in_ids_fit_element_ids() {
        assert!((NOTE_BASE + u32::MAX as u64).checked_mul(64).is_some_and(|k| k < usize::MAX as u64));
        assert!(!is_pending_note(40_000_000_000));
    }

    #[test]
    fn reply_and_resolve() {
        let mut r = review();
        Patch::Reply { thread: "t1".into(), body: "done".into(), resolve: true }.apply(&mut r, &user(1), 1);
        let d = &r.general[0];
        assert_eq!(d.notes.len(), 2);
        assert!(d.resolved());
        assert_eq!(d.resolution().unwrap().0.id, 1);
        Patch::Resolve { thread: "t1".into(), resolved: false }.apply(&mut r, &user(1), 2);
        assert!(!r.general[0].resolved());
        assert_eq!(r.unresolved(), 1);
    }

    #[test]
    fn approve_and_revoke() {
        let mut r = review();
        Patch::Approve { approve: true }.apply(&mut r, &user(1), 1);
        Patch::Approve { approve: true }.apply(&mut r, &user(1), 2);
        assert_eq!(r.approvals.approved_by.len(), 1);
        Patch::Approve { approve: false }.apply(&mut r, &user(1), 3);
        assert!(r.approvals.approved_by.is_empty());
    }

    #[test]
    fn awards_toggle() {
        let mut r = review();
        Patch::Award { name: "tada".into(), add: true }.apply(&mut r, &user(1), 1);
        assert_eq!(r.awards.len(), 1);
        Patch::Award { name: "tada".into(), add: false }.apply(&mut r, &user(1), 2);
        assert!(r.awards.is_empty());
        Patch::NoteAward { note: 1, name: "eyes".into(), add: true }.apply(&mut r, &user(1), 3);
        assert_eq!(r.note_awards[&1].len(), 1);
        Patch::NoteAward { note: 1, name: "eyes".into(), add: false }.apply(&mut r, &user(1), 4);
        assert!(!r.note_awards.contains_key(&1));
    }

    #[test]
    fn a_line_comment_lands_on_its_line() {
        let mut r = review();
        Patch::LineComment { file: 0, old: None, new: Some(2), body: "why c?".into() }.apply(&mut r, &user(1), 1);
        let (h, l, d) = &r.files[0].threads[0];
        assert_eq!((*h, *l), (0, 2));
        assert_eq!(d.notes[0].body, "why c?");
        assert_eq!(r.unresolved(), 2);
    }

    #[test]
    fn sidebar_fields() {
        let mut r = review();
        let people = vec![user(9)];
        Patch::Fields { fields: json!({ "reviewer_ids": [2, 9], "labels": "bug,new" }), people }.apply(&mut r, &user(1), 1);
        assert_eq!(r.mr.reviewers.iter().map(|u| u.id).collect::<Vec<_>>(), [2, 9]);
        assert_eq!(r.mr.labels[0].color, "#f00");
        assert_eq!(r.mr.labels[1].name, "new");
        Patch::Fields { fields: json!({ "remove_labels": "bug" }), people: vec![] }.apply(&mut r, &user(1), 2);
        assert_eq!(r.mr.labels.len(), 1);
        // Someone unknown: left as it was.
        Patch::Fields { fields: json!({ "assignee_ids": [42] }), people: vec![] }.apply(&mut r, &user(1), 3);
        assert!(r.mr.assignees.is_empty());
    }
}
