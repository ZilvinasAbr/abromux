//! Writing, as GitLab's review pages offer it: a comment on the merge
//! request, replies in threads, resolving and reopening threads, new
//! threads on a line of the diff, and approving.
//!
//! Each write runs in the background; when it lands the review is fetched
//! again quietly (as the two-minute poll does), so what's shown is what
//! GitLab has. A failure says why above the overview and keeps what was
//! typed.

use crate::{api::Client, mr::AwardOn, style::*, types::DiffRefs, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputState};

/// A line a new thread goes on: (file index, old line, new line).
pub type LineKey = (usize, Option<u32>, Option<u32>);

/// Which write is under way. One runs at a time, and only its own button
/// says so ("Approving…"): a reply being sent mustn't make Approve flicker.
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
        }
    }

    /// One of them has the keys: the tab's shortcuts stand aside.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        [&self.comment, &self.reply, &self.line, &self.search]
            .iter()
            .any(|b| b.read(cx).focus_handle(cx).is_focused(window))
    }
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

impl GitLabView {
    /// Run `f` against GitLab in the background (for the open merge
    /// request: its project and iid), then `done` and a quiet refresh; or
    /// keep everything and show the error.
    pub(crate) fn write(
        &mut self,
        what: Write,
        f: impl FnOnce(&Client, u64, u64) -> anyhow::Result<()> + Send + 'static,
        done: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(c), Some(p), Some(iid)) = (self.client.clone(), self.loc.project, self.loc.iid) else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(what);
        self.write_error = None;
        cx.notify();
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let got = cx.background_spawn(async move { f(&c, p, iid) }).await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |v, cx| {
                    v.busy = None;
                    match got {
                        Ok(()) => {
                            done(v, window, cx);
                            v.refresh_quietly(cx);
                            v.start_agents(window, cx);
                        }
                        Err(e) => v.write_error = Some(format!("{e:#}")),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    // ---- the actions, also behind socket commands ------------------------

    /// Post the Overview's comment box.
    pub(crate) fn post_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.boxes.comment.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        self.write(
            Write::Comment,
            move |c, p, iid| c.comment(p, iid, &body).map(|_| ()),
            |v, window, cx| v.boxes.comment.update(cx, |b, cx| b.set_value("", window, cx)),
            window,
            cx,
        );
    }

    /// Open the reply box under thread `id` (closing any other).
    pub(crate) fn open_reply(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
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
        self.write(
            Write::Reply,
            move |c, p, iid| {
                c.reply(p, iid, &id, &body)?;
                if resolve {
                    c.resolve(p, iid, &id, true)?;
                }
                Ok(())
            },
            |v, window, cx| {
                v.boxes.reply.update(cx, |b, cx| b.set_value("", window, cx));
                v.replying = None;
            },
            window,
            cx,
        );
    }

    /// Resolve, or reopen, thread `id`.
    pub(crate) fn set_resolved(&mut self, id: &str, resolved: bool, window: &mut Window, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.write(Write::Resolve, move |c, p, iid| c.resolve(p, iid, &id, resolved).map(|_| ()), |_, _, _| {}, window, cx);
    }

    pub(crate) fn set_approved(&mut self, approve: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.write(Write::Approve, move |c, p, iid| c.approve(p, iid, approve), |_, _, _| {}, window, cx);
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
                    move |c, p, iid| match mine {
                        Some(id) => c.unaward(p, iid, id),
                        None => c.award(p, iid, &name),
                    },
                    |_, _, _| {},
                    window,
                    cx,
                );
            }
            AwardOn::Note(note) => {
                let mine = r.note_awards.get(&note).is_some_and(|aw| aw.iter().any(|a| a.name == name && Some(a.user.id) == me));
                self.write(
                    Write::Award,
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
        self.write(Write::Rebase, |c, p, iid| c.rebase(p, iid), |_, _, _| {}, window, cx);
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
        self.write(
            if draft { Write::LineDraft } else { Write::LineComment },
            move |c, p, iid| {
                let refs: DiffRefs = refs;
                if draft {
                    c.draft_line_comment(p, iid, &refs, &old_path, &new_path, old, new, &body).map(|_| ())
                } else {
                    c.line_comment(p, iid, &refs, &old_path, &new_path, old, new, &body).map(|_| ())
                }
            },
            |v, window, cx| {
                v.boxes.line.update(cx, |b, cx| b.set_value("", window, cx));
                v.commenting = None;
            },
            window,
            cx,
        );
    }

    /// Submit your review: its pending comments become threads.
    pub(crate) fn submit_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.write(
            Write::SubmitReview,
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

    // ---- what they look like ----------------------------------------------

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
        let (label, live) = busy_label(self.busy, Write::Comment, "Comment", "Commenting…");
        let enabled = !empty && live;
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
                    .child(Input::new(&self.boxes.comment).appearance(false))
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
        let open = self.replying.as_deref() == Some(id);
        let (reply_label, live) = busy_label(self.busy, Write::Reply, "Reply", "Replying…");
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
        let enabled = !empty && live;
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
                    .child(Input::new(&self.boxes.reply).appearance(false)),
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
        let (send_label, live) = busy_label(self.busy, Write::LineComment, "Add comment now", "Adding…");
        let enabled = !empty && live && self.busy != Some(Write::LineDraft);
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
                    .child(Input::new(&self.boxes.line).appearance(false)),
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
            let (label, live) = busy_label(self.busy, Write::Approve, "Approve", "Approving…");
            primary("gl-approve", label, live)
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
}
