//! Writing, as GitLab's review pages offer it: a comment on the merge
//! request, replies in threads, resolving and reopening threads, new
//! threads on a line of the diff, and approving.
//!
//! Each write shows what it will do at once (`optimistic`), and queues to
//! run in the background, in order; when the queue empties the review is
//! fetched again quietly (as the two-minute poll does), so what's shown is
//! what GitLab has. A failure takes its effect back, says why above the
//! overview and puts what was typed back in its box.

use crate::{
    api::Client,
    mr::AwardOn,
    optimistic::{Patch, is_pending_note, is_pending_thread},
    review::Review,
    style::*,
    types::DiffRefs,
    view::{GitLabView, Load},
};
use std::rc::Rc;
use gpui::{prelude::*, *};
#[allow(unused_imports)]
use crate::style::px;
use gpui_component::input::{Input, InputState};

/// A line a new thread goes on: (file index, old line, new line).
pub type LineKey = (usize, Option<u32>, Option<u32>);

/// Which write is under way. One runs at a time, the rest queue; most show
/// their effect at once (optimistic.rs), and only those that can't say so
/// on their own button ("Submitting…").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    Comment,
    Reply,
    Resolve,
    Approve,
    Award,
    Rebase,
    LineComment,
    LineDraft,
    SubmitReview,
    Sidebar,
    Merge,
    /// Closing, reopening, marking as draft or ready.
    MrState,
    EditMr,
    EditNote,
    DeleteNote,
    Pipeline,
}

/// A button's label and whether it's live, given what's being written:
/// `doing` while its own write `w` is under way, else `idle`.
pub fn busy_label(busy: Option<Write>, w: Write, idle: &'static str, doing: &'static str) -> (&'static str, bool) {
    if busy == Some(w) { (doing, false) } else { (idle, true) }
}

/// The text fields writing uses, made once per tab.
pub struct Boxes {
    /// The Overview's "Add a comment".
    pub comment: Entity<InputState>,
    /// The open reply (one at a time, as in GitLab).
    pub reply: Entity<InputState>,
    /// The open new thread on a line.
    pub line: Entity<InputState>,
    /// The search in a sidebar dropdown (side.rs).
    pub search: Entity<InputState>,
    /// Editing the merge request: its title and description.
    pub title: Entity<InputState>,
    pub description: Entity<InputState>,
    /// Editing one of your comments.
    pub note: Entity<InputState>,
}

impl Boxes {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let field = |placeholder: &'static str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).multi_line(true).auto_grow(3, 12).placeholder(placeholder))
        };
        Boxes {
            comment: field("Write a comment…", window, cx),
            reply: field("Reply…", window, cx),
            line: field("Write a comment on this line…", window, cx),
            search: cx.new(|cx| InputState::new(window, cx).placeholder("Search")),
            title: cx.new(|cx| InputState::new(window, cx).placeholder("Title")),
            description: cx.new(|cx| InputState::new(window, cx).multi_line(true).auto_grow(6, 24).placeholder("Describe the goal of the changes and what reviewers should be aware of.")),
            note: field("Edit comment…", window, cx),
        }
    }

    /// One of them has the keys: the tab's shortcuts stand aside.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        [&self.comment, &self.reply, &self.line, &self.search, &self.title, &self.description, &self.note]
            .iter()
            .any(|b| b.read(cx).focus_handle(cx).is_focused(window))
    }
}

/// A write that failed: what was typed back in its box, unless something
/// new is being typed there.
fn put_back(b: &Entity<InputState>, text: &str, window: &mut Window, cx: &mut App) {
    b.update(cx, |b, cx| {
        if b.value().trim().is_empty() {
            b.set_value(text.to_string(), window, cx);
        }
    });
}

/// GitLab's primary (blue) button.
pub fn primary(id: impl Into<ElementId>, label: &'static str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(32.))
        .px_3()
        .rounded(px(8.))
        .bg(c(if enabled { 0x1068bf } else { 0x2b4a73 }))
        .text_color(c(if enabled { HEADING } else { MUTED }))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(0x0b5cad))))
        .child(label)
}

/// GitLab's default (grey) button.
pub fn secondary(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(32.))
        .px_3()
        .rounded(px(8.))
        .bg(ca(0x89888d66))
        .cursor_pointer()
        .hover(|s| s.bg(ca(0x89888d99)))
        .text_color(c(TEXT))
        .child(label)
}

type Run = Box<dyn FnOnce(&Client, u64, u64) -> anyhow::Result<()> + Send>;
type Undo = Box<dyn FnOnce(&mut GitLabView, &mut Window, &mut Context<GitLabView>)>;

/// A write waiting its turn, or under way: they run one at a time, in the
/// order made, so "approve, then revoke" reaches GitLab in that order.
struct Job {
    what: Write,
    n: u64,
    mr: (u64, u64),
    f: Run,
    failed: Undo,
}

/// A write's patch, shown from when it's made until a review fetched after
/// it `landed` is in (or it fails).
struct Shown {
    n: u64,
    mr: (u64, u64),
    patch: Patch,
    landed: Option<std::time::Instant>,
}

/// The writes of a tab: queued, and showing ahead of GitLab.
#[derive(Default)]
pub struct Writes {
    queue: std::collections::VecDeque<Job>,
    shown: Vec<Shown>,
    next: u64,
    /// The review as GitLab last sent it, before any patch.
    base: Option<Rc<Review>>,
}

impl Writes {
    /// Something is queued or under way.
    pub fn any(&self) -> bool {
        !self.queue.is_empty()
    }
}

/// GitLab's danger (red) button, for deleting.
pub fn danger(id: impl Into<ElementId>, label: &'static str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(32.))
        .px_3()
        .rounded(px(8.))
        .bg(c(if enabled { 0xdd2b0e } else { 0x6e1f14 }))
        .text_color(c(if enabled { HEADING } else { MUTED }))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(0xc91c00))))
        .child(label)
}

/// A checkbox with its label, as GitLab's merge widget has them.
pub fn checkbox(id: impl Into<ElementId>, on: bool, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(icon(if on { "square-check" } else { "square" }, 16., c(if on { FOCUS } else { MUTED })))
        .child(label)
}

/// A small icon button, as on a comment's header.
pub fn icon_button(id: impl Into<ElementId>, svg: Svg, tip: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|s| s.bg(ca(0x89888d66)))
        .child(svg)
        .tooltip(hint(tip))
}

/// The title with GitLab's draft marker, or without it ("Mark as ready").
pub fn draft_title(title: &str, draft: bool) -> String {
    let t = title.trim_start();
    let mut bare = t;
    for prefix in ["Draft:", "[Draft]", "(Draft)", "Draft - ", "Draft "] {
        if bare.len() >= prefix.len() && bare[..prefix.len()].eq_ignore_ascii_case(prefix) {
            bare = bare[prefix.len()..].trim_start();
            break;
        }
    }
    if draft { format!("Draft: {bare}") } else { bare.to_string() }
}

/// Whether a pipeline in this status is still to finish: auto-merge waits
/// for it, Cancel stops it.
pub fn pipeline_active(status: &str) -> bool {
    matches!(status, "created" | "waiting_for_resource" | "preparing" | "pending" | "running" | "scheduled")
}

impl GitLabView {
    /// Show `r` as GitLab sent it, fetched at `fetched`: the patches of
    /// writes that landed before then are in it now, the rest go on top.
    pub(crate) fn set_review(&mut self, r: Rc<Review>, fetched: Option<std::time::Instant>, cx: &mut Context<Self>) {
        if let Some(at) = fetched {
            let key = (r.mr.project_id, r.mr.iid);
            self.writes.shown.retain(|s| s.mr != key || s.landed.is_none_or(|l| l > at));
        }
        self.writes.base = Some(r);
        self.present(cx);
    }

    /// The review on screen gone (loading another, failed, back to the list).
    pub(crate) fn clear_review(&mut self, to: Load<Rc<Review>>) {
        self.writes.base = None;
        self.review = to;
    }

    /// The last review GitLab sent, with the writes not yet in it on top.
    fn present(&mut self, cx: &mut Context<Self>) {
        let Some(base) = self.writes.base.clone() else { return };
        let key = (base.mr.project_id, base.mr.iid);
        let mut on: Vec<&Shown> = self.writes.shown.iter().filter(|s| s.mr == key).collect();
        let shown = match (&self.me, on.is_empty()) {
            (Some(me), false) => {
                on.sort_by_key(|s| s.n);
                let mut r = (*base).clone();
                for s in on {
                    s.patch.apply(&mut r, me, s.n);
                }
                Rc::new(r)
            }
            _ => base,
        };
        self.review = Load::Ready(shown);
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// The review as GitLab last sent it (no writes on top).
    pub(crate) fn fetched_review(&self) -> Option<&Rc<Review>> {
        self.writes.base.as_ref()
    }

    /// Queue `f` against GitLab (for the open merge request: its project
    /// and iid), showing `patch` at once. When the queue empties the review
    /// is fetched again quietly, so what's shown is what GitLab has. A
    /// failure takes the patch back, says why above the overview, and runs
    /// `failed` (to put back what was typed).
    pub(crate) fn write(
        &mut self,
        what: Write,
        patch: Option<Patch>,
        f: impl FnOnce(&Client, u64, u64) -> anyhow::Result<()> + Send + 'static,
        failed: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(_), Some(p), Some(iid)) = (self.client.as_ref(), self.loc.project, self.loc.iid) else {
            return;
        };
        self.writes.next += 1;
        let n = self.writes.next;
        self.write_error = None;
        if let Some(patch) = patch {
            self.writes.shown.push(Shown { n, mr: (p, iid), patch, landed: None });
            self.present(cx);
        }
        self.writes.queue.push_back(Job { what, n, mr: (p, iid), f: Box::new(f), failed: Box::new(failed) });
        self.pump(window, cx);
        cx.notify();
    }

    /// Start the next queued write, unless one is under way.
    fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(c) = self.client.clone() else { return };
        let Some(Job { what, n, mr: (p, iid), f, failed }) = self.writes.queue.pop_front() else { return };
        self.busy = Some(what);
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let got = cx.background_spawn(async move { f(&c, p, iid) }).await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |v, cx| {
                    v.busy = None;
                    match got {
                        Ok(()) => {
                            if let Some(s) = v.writes.shown.iter_mut().find(|s| s.n == n) {
                                s.landed = Some(std::time::Instant::now());
                            }
                            v.start_agents(window, cx);
                        }
                        Err(e) => {
                            v.writes.shown.retain(|s| s.n != n);
                            v.write_error = Some(format!("{e:#}"));
                            failed(v, window, cx);
                            v.present(cx);
                        }
                    }
                    // The last of a burst: what GitLab has now.
                    if v.writes.queue.is_empty() && (v.loc.project, v.loc.iid) == (Some(p), Some(iid)) {
                        v.refresh_quietly(cx);
                    }
                    v.pump(window, cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    // ---- the actions, also behind socket commands ------------------------

    /// Post the Overview's comment box: it shows in the activity at once,
    /// and comes back to the box if GitLab turns it down.
    pub(crate) fn post_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.boxes.comment.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        self.boxes.comment.update(cx, |b, cx| b.set_value("", window, cx));
        let back = body.clone();
        self.write(
            Write::Comment,
            Some(Patch::Comment { body: body.clone() }),
            move |c, p, iid| c.comment(p, iid, &body).map(|_| ()),
            move |v, window, cx| put_back(&v.boxes.comment, &back, window, cx),
            window,
            cx,
        );
    }

    /// Open the reply box under thread `id` (closing any other).
    pub(crate) fn open_reply(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if is_pending_thread(id) {
            return;
        }
        if self.replying.as_deref() != Some(id) {
            self.boxes.reply.update(cx, |b, cx| b.set_value("", window, cx));
        }
        self.replying = Some(id.to_string());
        self.boxes.reply.update(cx, |b, cx| b.focus(window, cx));
        self.rebuild_rows(cx);
    }

    pub(crate) fn close_reply(&mut self, cx: &mut Context<Self>) {
        self.replying = None;
        self.rebuild_rows(cx);
    }

    /// Send the open reply, and with `resolve`, resolve the thread too
    /// ("Reply and resolve" isn't a GitLab button, but resolving after
    /// replying is what its checkbox does).
    pub(crate) fn send_reply(&mut self, resolve: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.replying.clone() else { return };
        let body = self.boxes.reply.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        self.boxes.reply.update(cx, |b, cx| b.set_value("", window, cx));
        self.replying = None;
        let (thread, back) = (id.clone(), body.clone());
        self.write(
            Write::Reply,
            Some(Patch::Reply { thread: id.clone(), body: body.clone(), resolve }),
            move |c, p, iid| {
                c.reply(p, iid, &id, &body)?;
                if resolve {
                    c.resolve(p, iid, &id, true)?;
                }
                Ok(())
            },
            // The reply box open on the thread again, with the text.
            move |v, window, cx| {
                if v.replying.is_none() || v.replying.as_deref() == Some(thread.as_str()) {
                    v.replying = Some(thread);
                    put_back(&v.boxes.reply, &back, window, cx);
                }
            },
            window,
            cx,
        );
        self.rebuild_rows(cx);
    }

    /// Resolve, or reopen, thread `id`.
    pub(crate) fn set_resolved(&mut self, id: &str, resolved: bool, window: &mut Window, cx: &mut Context<Self>) {
        if is_pending_thread(id) {
            return;
        }
        let id = id.to_string();
        self.write(
            Write::Resolve,
            Some(Patch::Resolve { thread: id.clone(), resolved }),
            move |c, p, iid| c.resolve(p, iid, &id, resolved).map(|_| ()),
            |_, _, _| {},
            window,
            cx,
        );
    }

    pub(crate) fn set_approved(&mut self, approve: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.write(Write::Approve, Some(Patch::Approve { approve }), move |c, p, iid| c.approve(p, iid, approve), |_, _, _| {}, window, cx);
    }

    /// GitLab's reaction toggle, on the merge request or a comment: take
    /// back mine of `name`, or add it.
    pub(crate) fn toggle_award(&mut self, on: AwardOn, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.ready() else { return };
        let me = self.me.as_ref().map(|u| u.id);
        let name = name.to_string();
        self.emoji_picker = None;
        match on {
            AwardOn::Mr => {
                let mine = r.awards.iter().find(|a| a.name == name && Some(a.user.id) == me).map(|a| a.id);
                self.write(
                    Write::Award,
                    Some(Patch::Award { name: name.clone(), add: mine.is_none() }),
                    move |c, p, iid| match mine {
                        // Added here and not yet back from GitLab: its id is
                        // GitLab's to tell (it's in by now: writes go in order).
                        Some(id) if is_pending_note(id) => {
                            let a = c.awards(p, iid)?.into_iter().find(|a| a.name == name && Some(a.user.id) == me);
                            a.map_or(Ok(()), |a| c.unaward(p, iid, a.id))
                        }
                        Some(id) => c.unaward(p, iid, id),
                        None => c.award(p, iid, &name),
                    },
                    |_, _, _| {},
                    window,
                    cx,
                );
            }
            AwardOn::Note(note) => {
                if is_pending_note(note) {
                    return;
                }
                let mine = r.note_awards.get(&note).is_some_and(|aw| aw.iter().any(|a| a.name == name && Some(a.user.id) == me));
                self.write(
                    Write::Award,
                    Some(Patch::NoteAward { note, name: name.clone(), add: !mine }),
                    move |c, p, iid| match (mine, me) {
                        (true, Some(me)) => c.note_unaward(p, iid, note, &name, me),
                        _ => c.note_award(p, iid, note, &name),
                    },
                    |_, _, _| {},
                    window,
                    cx,
                );
            }
        }
    }

    pub(crate) fn rebase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.write(Write::Rebase, None, |c, p, iid| c.rebase(p, iid), |_, _, _| {}, window, cx);
    }

    /// Open the new-thread box under a line of the diff.
    pub(crate) fn open_line_comment(&mut self, key: LineKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.commenting != Some(key) {
            self.boxes.line.update(cx, |b, cx| b.set_value("", window, cx));
        }
        self.commenting = Some(key);
        self.boxes.line.update(cx, |b, cx| b.focus(window, cx));
        self.rebuild_rows(cx);
    }

    pub(crate) fn close_line_comment(&mut self, cx: &mut Context<Self>) {
        self.commenting = None;
        self.rebuild_rows(cx);
    }

    /// Start the thread on the line: on the latest version, as GitLab
    /// starts new threads (comparing older versions is for reading).
    /// `draft`: add it to your review instead ("Start a review").
    pub(crate) fn send_line_comment(&mut self, draft: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((fi, old, new)) = self.commenting else { return };
        let body = self.boxes.line.read(cx).value().trim().to_string();
        let Some(r) = self.review.ready().cloned() else { return };
        let (Some(f), Some(refs)) = (r.files.get(fi), r.mr.diff_refs.clone()) else { return };
        if body.is_empty() {
            return;
        }
        let (old_path, new_path) = (f.meta.old_path.clone(), f.meta.new_path.clone());
        self.boxes.line.update(cx, |b, cx| b.set_value("", window, cx));
        self.commenting = None;
        let back = body.clone();
        let patch = if draft {
            Patch::Draft { body: body.clone() }
        } else {
            Patch::LineComment { file: fi, old, new, body: body.clone() }
        };
        self.write(
            if draft { Write::LineDraft } else { Write::LineComment },
            Some(patch),
            move |c, p, iid| {
                let refs: DiffRefs = refs;
                if draft {
                    c.draft_line_comment(p, iid, &refs, &old_path, &new_path, old, new, &body).map(|_| ())
                } else {
                    c.line_comment(p, iid, &refs, &old_path, &new_path, old, new, &body).map(|_| ())
                }
            },
            // The box open on the line again, with the text.
            move |v, window, cx| {
                if v.commenting.is_none() || v.commenting == Some((fi, old, new)) {
                    v.commenting = Some((fi, old, new));
                    put_back(&v.boxes.line, &back, window, cx);
                    v.rebuild_rows(cx);
                }
            },
            window,
            cx,
        );
        self.rebuild_rows(cx);
    }

    /// Submit your review: its pending comments become threads.
    pub(crate) fn submit_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.write(
            Write::SubmitReview,
            None,
            |c, p, iid| {
                let pending: Vec<String> = c.draft_notes(p, iid)?.into_iter().map(|d| d.note).collect();
                c.publish_drafts(p, iid)?;
                // GitLab publishes in the background: wait (a little) until
                // the comments are threads, so the refresh after shows them.
                for _ in 0..10 {
                    let shown = c.discussions(p, iid)?;
                    let bodies: Vec<&str> = shown.iter().flat_map(|d| &d.notes).map(|n| n.body.as_str()).collect();
                    if pending.iter().all(|b| bodies.contains(&b.as_str())) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Ok(())
            },
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// GitLab's header button while a review is pending: how many comments
    /// it holds, a click submits them.
    pub(crate) fn review_button(&self, r: &crate::review::Review, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let n = r.drafts.len();
        (n > 0 && self.me.is_some()).then(|| {
            let (label, live) = busy_label(self.busy, Write::SubmitReview, "Submit review", "Submitting…");
            primary("gl-submit-review", label, live)
                .flex()
                .items_center()
                .gap_2()
                .child(count_badge(n))
                .on_click(cx.listener(|v, _, window, cx| v.submit_review(window, cx)))
        })
    }

    /// Merge now, or with `auto` once the checks pass, with the widget's
    /// ticks. It waits for GitLab ("Merging…"): a merge can be refused.
    pub(crate) fn merge(&mut self, auto: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.ready() else { return };
        let opts = crate::api::MergeOptions {
            squash: self.merge_squash(r),
            remove_source_branch: self.merge_remove(r),
            auto,
            sha: r.mr.sha.clone(),
        };
        self.write(Write::Merge, None, move |c, p, iid| c.merge(p, iid, &opts).map(|_| ()), |_, _, _| {}, window, cx);
    }

    pub(crate) fn cancel_auto_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.write(Write::Merge, None, |c, p, iid| c.cancel_auto_merge(p, iid), |_, _, _| {}, window, cx);
    }

    /// The widget's "Squash commits": what was ticked, else the author's
    /// choice (or the project's, when it always squashes).
    pub(crate) fn merge_squash(&self, r: &crate::review::Review) -> bool {
        r.mr.squash_on_merge || self.merge_ticks.0.unwrap_or(r.mr.squash)
    }

    /// The widget's "Delete source branch": ticked, else the author's choice.
    pub(crate) fn merge_remove(&self, r: &crate::review::Review) -> bool {
        self.merge_ticks.1.unwrap_or(r.mr.force_remove_source_branch || r.mr.should_remove_source_branch.unwrap_or(false))
    }

    /// Close or reopen the merge request.
    pub(crate) fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.mr_menu = false;
        self.write(
            Write::MrState,
            Some(Patch::State { open }),
            move |c, p, iid| c.set_open(p, iid, open).map(|_| ()),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Mark as draft, or as ready.
    pub(crate) fn set_draft(&mut self, draft: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.ready() else { return };
        let fields = serde_json::json!({ "title": draft_title(&r.mr.title, draft) });
        self.mr_menu = false;
        self.write(
            Write::MrState,
            Some(Patch::Fields { fields: fields.clone(), people: vec![] }),
            move |c, p, iid| c.update(p, iid, fields).map(|_| ()),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// GitLab's Edit: the title and description in fields, at the top of
    /// the Overview.
    pub(crate) fn open_edit_mr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.ready().cloned() else { return };
        self.mr_menu = false;
        self.editing_mr = true;
        self.boxes.title.update(cx, |b, cx| b.set_value(r.mr.title.clone(), window, cx));
        self.boxes.description.update(cx, |b, cx| b.set_value(r.mr.description.clone().unwrap_or_default(), window, cx));
        self.set_section(crate::Section::Overview, cx);
        self.overview_list.scroll_to(gpui::ListOffset::default());
        self.boxes.title.update(cx, |b, cx| b.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn close_edit_mr(&mut self, cx: &mut Context<Self>) {
        self.editing_mr = false;
        cx.notify();
    }

    /// Save the edited title and description: they show at once, and the
    /// form comes back with them if GitLab turns them down.
    pub(crate) fn save_mr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.boxes.title.read(cx).value().trim().to_string();
        let description = self.boxes.description.read(cx).value().to_string();
        if title.is_empty() {
            return;
        }
        self.close_edit_mr(cx);
        let fields = serde_json::json!({ "title": title, "description": description });
        self.write(
            Write::EditMr,
            Some(Patch::Fields { fields: fields.clone(), people: vec![] }),
            move |c, p, iid| c.update(p, iid, fields).map(|_| ()),
            move |v, window, cx| {
                v.editing_mr = true;
                v.boxes.title.update(cx, |b, cx| b.set_value(title, window, cx));
                v.boxes.description.update(cx, |b, cx| b.set_value(description, window, cx));
            },
            window,
            cx,
        );
    }

    /// Edit your comment `note`: its text in a field, in place.
    pub(crate) fn open_edit_note(&mut self, note: u64, body: &str, window: &mut Window, cx: &mut Context<Self>) {
        if is_pending_note(note) {
            return;
        }
        self.editing_note = Some(note);
        self.deleting_note = None;
        let body = body.to_string();
        self.boxes.note.update(cx, |b, cx| {
            b.set_value(body, window, cx);
            b.focus(window, cx);
        });
        self.rebuild_rows(cx);
    }

    pub(crate) fn close_edit_note(&mut self, cx: &mut Context<Self>) {
        self.editing_note = None;
        self.rebuild_rows(cx);
    }

    /// Save the comment being edited: the new text shows at once, and comes
    /// back to its field if GitLab turns it down.
    pub(crate) fn save_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.editing_note else { return };
        let body = self.boxes.note.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        self.close_edit_note(cx);
        let back = body.clone();
        self.write(
            Write::EditNote,
            Some(Patch::EditNote { note, body: body.clone() }),
            move |c, p, iid| c.edit_note(p, iid, note, &body).map(|_| ()),
            move |v, window, cx| {
                v.editing_note = Some(note);
                v.boxes.note.update(cx, |b, cx| b.set_value(back, window, cx));
            },
            window,
            cx,
        );
    }

    /// Ask, then (with `sure`) delete your comment `note`: it goes at once,
    /// and comes back if GitLab turns that down.
    pub(crate) fn delete_note(&mut self, note: u64, sure: bool, window: &mut Window, cx: &mut Context<Self>) {
        if is_pending_note(note) {
            return;
        }
        if !sure {
            self.deleting_note = Some(note);
            self.rebuild_rows(cx);
            return;
        }
        self.deleting_note = None;
        self.write(
            Write::DeleteNote,
            Some(Patch::DeleteNote { note }),
            move |c, p, iid| c.delete_note(p, iid, note),
            |_, _, _| {},
            window,
            cx,
        );
    }

    pub(crate) fn cancel_delete_note(&mut self, cx: &mut Context<Self>) {
        self.deleting_note = None;
        self.rebuild_rows(cx);
    }

    /// Retry or cancel pipeline `id`, or (`None`) run a new one.
    pub(crate) fn pipeline_action(&mut self, id: Option<u64>, retry: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.write(
            Write::Pipeline,
            None,
            move |c, p, iid| match (id, retry) {
                (None, _) => c.run_pipeline(p, iid),
                (Some(id), true) => c.retry_pipeline(p, id),
                (Some(id), false) => c.cancel_pipeline(p, id),
            },
            |_, _, _| {},
            window,
            cx,
        );
    }

    // ---- what they look like ----------------------------------------------

    /// The header's buttons right of the title, as GitLab has them: Edit,
    /// and ⋮ with Mark as draft or ready, Close or Reopen, Copy reference.
    pub(crate) fn mr_actions(&self, r: &crate::review::Review, cx: &mut Context<Self>) -> Option<Div> {
        if self.is_local() {
            return None;
        }
        let m = &r.mr;
        let writes = self.me.is_some() && m.state != "merged" && m.state != "locked";
        let item = |id: &'static str, label: String, danger: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .cursor_pointer()
                .hover(|st| st.bg(c(RAISED)))
                .text_color(c(if danger { RED } else { TEXT }))
                .child(label)
        };
        let menu = self.mr_menu.then(|| {
            let draft = m.draft;
            let open = m.state == "opened";
            let reference = m.references.full.clone();
            let items = div()
                .when(writes && open, |d| {
                    d.child(
                        item("gl-mr-draft", if draft { "Mark as ready" } else { "Mark as draft" }.into(), false)
                            .on_click(cx.listener(move |v, _, window, cx| v.set_draft(!draft, window, cx))),
                    )
                })
                .when(writes, |d| {
                    d.child(
                        item("gl-mr-state", if open { "Close merge request" } else { "Reopen merge request" }.into(), open)
                            .on_click(cx.listener(move |v, _, window, cx| v.set_open(!open, window, cx))),
                    )
                })
                .when(!reference.is_empty(), |d| {
                    d.child(
                        item("gl-mr-copy-ref", "Copy reference".into(), false).on_click(cx.listener(
                            move |v, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(reference.clone()));
                                v.mr_menu = false;
                                cx.notify();
                            },
                        )),
                    )
                });
            deferred(
                div()
                    .id("gl-mr-menu")
                    .absolute()
                    .top(px(36.))
                    .right_0()
                    .min_w(px(220.))
                    .py_1()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(CARD))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                        v.mr_menu = false;
                        cx.notify();
                    }))
                    .child(items),
            )
            .with_priority(1)
        });
        Some(
            div()
                .relative()
                .flex()
                .items_center()
                .gap_2()
                .flex_none()
                .when(writes, |d| {
                    d.child(secondary("gl-mr-edit", "Edit").on_click(cx.listener(|v, _, window, cx| v.open_edit_mr(window, cx))))
                })
                .child(
                    secondary("gl-mr-more", "")
                        .px_2()
                        .child(icon("ellipsis-v", 16., c(TEXT)))
                        .tooltip(hint("Merge request actions"))
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.mr_menu = !v.mr_menu;
                            cx.notify();
                        })),
                )
                .children(menu),
        )
    }

    /// Editing the merge request: Title and Description, Save changes.
    pub(crate) fn mr_edit_form(&self, cx: &mut Context<Self>) -> Div {
        let empty = self.boxes.title.read(cx).value().trim().is_empty();
        let (label, live) = busy_label(self.busy, Write::EditMr, "Save changes", "Saving…");
        let enabled = !empty && live;
        let field = |d: Div| d.p_2().rounded(px(8.)).border_1().border_color(c(BORDER)).bg(c(CARD));
        let heading = |s: &'static str| div().font_weight(FontWeight::SEMIBOLD).text_color(c(HEADING)).child(s);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("Title"))
            .child(field(div()).child(Input::new(&self.boxes.title).appearance(false)))
            .child(div().text_size(px(12.)).text_color(c(MUTED)).child("Start the title with Draft: to prevent a merge request draft from merging before it's ready."))
            .child(heading("Description").mt_2())
            .child(field(div()).child(Input::new(&self.boxes.description).appearance(false)))
            .child(div().text_size(px(12.)).text_color(c(MUTED)).child("Markdown is supported."))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mt_2()
                    .child(
                        primary("gl-mr-save", label, enabled)
                            .when(enabled, |d| d.on_click(cx.listener(|v, _, window, cx| v.save_mr(window, cx)))),
                    )
                    .child(secondary("gl-mr-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| v.close_edit_mr(cx)))),
            )
    }

    /// Your comment's Edit and Delete buttons.
    pub(crate) fn note_owner_actions(&self, n: &crate::types::Note, cx: &mut Context<Self>) -> Option<Div> {
        let mine = self.me.as_ref().is_some_and(|me| me.id == n.author.id);
        if !mine || n.system || is_pending_note(n.id) || self.is_local() || self.review.ready().is_some_and(|r| r.mr.state == "locked") {
            return None;
        }
        let (id, body) = (n.id, n.body.clone());
        Some(
            div()
                .flex()
                .items_center()
                .child(
                    icon_button(SharedString::from(format!("note-edit-{id}")), app_icon("pencil", 14., c(MUTED)), "Edit comment")
                        .on_click(cx.listener(move |v, _, window, cx| v.open_edit_note(id, &body, window, cx))),
                )
                .child(
                    icon_button(SharedString::from(format!("note-delete-{id}")), icon("remove", 15., c(MUTED)), "Delete comment")
                        .on_click(cx.listener(move |v, _, window, cx| v.delete_note(id, false, window, cx))),
                ),
        )
    }

    /// The comment being edited: its field, Save changes and Cancel.
    pub(crate) fn note_edit_form(&self, cx: &mut Context<Self>) -> Div {
        let empty = self.boxes.note.read(cx).value().trim().is_empty();
        let (label, live) = busy_label(self.busy, Write::EditNote, "Save changes", "Saving…");
        let enabled = !empty && live;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .p_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(0x1068bf))
                    .bg(c(BG))
                    .child(Input::new(&self.boxes.note).appearance(false)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        primary("gl-note-save", label, enabled)
                            .when(enabled, |d| d.on_click(cx.listener(|v, _, window, cx| v.save_note(window, cx)))),
                    )
                    .child(secondary("gl-note-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| v.close_edit_note(cx)))),
            )
    }

    /// GitLab's question before deleting a comment.
    pub(crate) fn note_delete_confirm(&self, note: u64, cx: &mut Context<Self>) -> Div {
        let (label, live) = busy_label(self.busy, Write::DeleteNote, "Delete comment", "Deleting…");
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .p_2()
            .rounded(px(8.))
            .bg(c(0x3e1d22))
            .border_1()
            .border_color(c(0x7a2e3a))
            .child(div().flex_1().text_color(c(TEXT)).child("Are you sure you want to delete this comment?"))
            .child(
                danger(SharedString::from(format!("note-delete-sure-{note}")), label, live)
                    .when(live, |d| d.on_click(cx.listener(move |v, _, window, cx| v.delete_note(note, true, window, cx)))),
            )
            .child(secondary("gl-note-delete-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| v.cancel_delete_note(cx))))
    }

    /// What went wrong with the last write, in GitLab's alert.
    pub(crate) fn write_alert(&self, cx: &mut Context<Self>) -> Option<Div> {
        let e = self.write_error.clone()?;
        Some(
            div()
                .flex()
                .items_center()
                .gap_3()
                .p_3()
                .rounded(px(8.))
                .bg(c(0x3e1d22))
                .border_1()
                .border_color(c(0x7a2e3a))
                .text_color(c(TEXT))
                .child(app_icon("circle-x", 16., c(RED)))
                .child(div().flex_1().min_w(px(0.)).child(e))
                .child(secondary("gl-write-dismiss", "Dismiss").on_click(cx.listener(|v, _, _, cx| {
                    v.write_error = None;
                    cx.notify();
                }))),
        )
    }

    /// GitLab's comment form at the end of the Overview.
    pub(crate) fn comment_form(&self, cx: &mut Context<Self>) -> Div {
        let empty = self.boxes.comment.read(cx).value().trim().is_empty();
        // It shows at once (optimistic.rs): nothing to wait for.
        let (label, enabled) = ("Comment", !empty);
        div()
            .flex()
            .gap_3()
            .child(match self.me.as_ref() {
                Some(me) => self.avatars.of(me, 32.),
                None => div().size(px(32.)).into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(CARD))
                    .child(self.mention_input(crate::mention::Field::Comment, Input::new(&self.boxes.comment).appearance(false), cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().text_size(px(12.)).text_color(c(MUTED)).child("Markdown is supported."))
                            .child(
                                primary("gl-comment", label, enabled)
                                    .when(enabled, |d| d.on_click(cx.listener(|v, _, window, cx| v.post_comment(window, cx)))),
                            ),
                    ),
            )
    }

    /// Under a thread: the reply placeholder (or the open reply box) and
    /// "Resolve thread" / "Reopen thread", as GitLab has them.
    pub(crate) fn reply_bar(&self, id: &str, resolvable: bool, resolved: bool, cx: &mut Context<Self>) -> Div {
        // Made here, not yet GitLab's: nothing to reply to or resolve yet.
        if is_pending_thread(id) {
            return div().px_4().py_3().border_t_1().border_color(c(RAISED)).bg(c(CARD)).text_color(c(MUTED)).child("Sending…");
        }
        let open = self.replying.as_deref() == Some(id);
        let reply_label = "Reply";
        let resolve_button = resolvable.then(|| {
            let thread = id.to_string();
            secondary(
                SharedString::from(format!("resolve-{id}")),
                if resolved { "Reopen thread" } else { "Resolve thread" },
            )
            .flex_none()
            .on_click(cx.listener(move |v, _, window, cx| v.set_resolved(&thread, !resolved, window, cx)))
        });
        if !open {
            let thread = id.to_string();
            return div()
                .flex()
                .items_center()
                .gap_2()
                .px_4()
                .py_3()
                .border_t_1()
                .border_color(c(RAISED))
                .bg(c(CARD))
                .child(
                    div()
                        .id(SharedString::from(format!("reply-{id}")))
                        .flex_1()
                        .h(px(32.))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(c(0x747273))
                        .bg(ca(0x05050666))
                        .cursor_text()
                        .text_color(c(MUTED))
                        .child("Reply…")
                        .on_click(cx.listener(move |v, _, window, cx| v.open_reply(&thread, window, cx))),
                )
                .children(resolve_button);
        }
        let empty = self.boxes.reply.read(cx).value().trim().is_empty();
        let enabled = !empty;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(c(RAISED))
            .bg(c(CARD))
            .child(
                div()
                    .p_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(0x1068bf))
                    .bg(c(BG))
                    .child(self.mention_input(crate::mention::Field::Reply, Input::new(&self.boxes.reply).appearance(false), cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        primary(SharedString::from(format!("send-{id}")), reply_label, enabled)
                            .when(enabled, |d| d.on_click(cx.listener(|v, _, window, cx| v.send_reply(false, window, cx)))),
                    )
                    .when(resolvable && !resolved, |d| {
                        d.child(
                            secondary(SharedString::from(format!("send-resolve-{id}")), "Reply and resolve")
                                .when(enabled, |d| d.on_click(cx.listener(|v, _, window, cx| v.send_reply(true, window, cx)))),
                        )
                    })
                    .child(div().flex_1())
                    .child(secondary(SharedString::from(format!("cancel-{id}")), "Cancel").on_click(cx.listener(|v, _, _, cx| v.close_reply(cx)))),
            )
    }

    /// The new-thread box under a line of the diff.
    pub(crate) fn line_form(&self, cx: &mut Context<Self>) -> Div {
        let empty = self.boxes.line.read(cx).value().trim().is_empty();
        let send_label = "Add comment now";
        let enabled = !empty;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(c(BG))
            .border_t_1()
            .border_b_1()
            .border_color(c(BORDER))
            .child(
                div()
                    .p_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(0x1068bf))
                    .bg(c(CARD))
                    .child(self.mention_input(crate::mention::Field::Line, Input::new(&self.boxes.line).appearance(false), cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    // GitLab's two ways: into your review, or straight away.
                    .child(
                        primary(
                            "gl-line-draft",
                            if self.review.ready().is_some_and(|r| !r.drafts.is_empty()) { "Add to review" } else { "Start a review" },
                            enabled,
                        )
                        .when(enabled, |d| d.on_click(cx.listener(|v, _, window, cx| v.send_line_comment(true, window, cx)))),
                    )
                    .child(
                        secondary("gl-line-send", send_label)
                            .when(enabled, |d| d.on_click(cx.listener(|v, _, window, cx| v.send_line_comment(false, window, cx)))),
                    )
                    .child(div().flex_1())
                    .child(secondary("gl-line-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| v.close_line_comment(cx)))),
            )
    }

    /// The approval widget's button: "Approve", or "Revoke approval" once
    /// you have.
    pub(crate) fn approve_button(&self, approved_by_me: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        if approved_by_me {
            secondary("gl-unapprove", "Revoke approval")
                .on_click(cx.listener(|v, _, window, cx| v.set_approved(false, window, cx)))
        } else {
            primary("gl-approve", "Approve", true)
                .on_click(cx.listener(|v, _, window, cx| v.set_approved(true, window, cx)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{busy_label, Write};

    /// Only the write under way shows it: resolving a thread or sending a
    /// reply mustn't turn Approve into "Approving…".
    #[test]
    fn only_the_running_write_says_so() {
        for other in [Write::Comment, Write::Reply, Write::Resolve, Write::Award, Write::Rebase, Write::LineComment, Write::Sidebar] {
            assert_eq!(busy_label(Some(other), Write::Approve, "Approve", "Approving…"), ("Approve", true), "{other:?}");
        }
        assert_eq!(busy_label(None, Write::Approve, "Approve", "Approving…"), ("Approve", true));
        assert_eq!(busy_label(Some(Write::Approve), Write::Approve, "Approve", "Approving…"), ("Approving…", false));
        assert_eq!(busy_label(Some(Write::Approve), Write::Reply, "Reply", "Replying…"), ("Reply", true));
    }

    #[test]
    fn draft_marker_comes_and_goes() {
        use super::draft_title;
        assert_eq!(draft_title("Fix the limiter", true), "Draft: Fix the limiter");
        assert_eq!(draft_title("Draft: Fix the limiter", true), "Draft: Fix the limiter");
        assert_eq!(draft_title("[Draft] Fix the limiter", true), "Draft: Fix the limiter");
        assert_eq!(draft_title("Draft: Fix the limiter", false), "Fix the limiter");
        assert_eq!(draft_title("Drafting notes", true), "Draft: Drafting notes");
    }
}
