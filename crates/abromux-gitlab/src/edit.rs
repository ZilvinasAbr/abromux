//! Writing, as GitLab's review pages offer it: a comment on the merge
//! request, replies in threads, resolving and reopening threads, new
//! threads on a line of the diff, and approving.
//!
//! Each write runs in the background; when it lands the review is fetched
//! again quietly (as the two-minute poll does), so what's shown is what
//! GitLab has. A failure says why above the overview and keeps what was
//! typed.

use crate::{api::Client, style::*, types::DiffRefs, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputState};

/// A line a new thread goes on: (file index, old line, new line).
pub type LineKey = (usize, Option<u32>, Option<u32>);

/// The text fields writing uses, made once per tab.
pub struct Boxes {
    /// The Overview's "Add a comment".
    pub comment: Entity<InputState>,
    /// The open reply (one at a time, as in GitLab).
    pub reply: Entity<InputState>,
    /// The open new thread on a line.
    pub line: Entity<InputState>,
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
        }
    }

    /// One of them has the keys: the tab's shortcuts stand aside.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        [&self.comment, &self.reply, &self.line]
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
        f: impl FnOnce(&Client, u64, u64) -> anyhow::Result<()> + Send + 'static,
        done: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(c), Some(p), Some(iid)) = (self.client.clone(), self.loc.project, self.loc.iid) else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.write_error = None;
        cx.notify();
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let got = cx.background_spawn(async move { f(&c, p, iid) }).await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |v, cx| {
                    v.busy = false;
                    match got {
                        Ok(()) => {
                            done(v, window, cx);
                            v.refresh_quietly(cx);
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
        self.write(move |c, p, iid| c.resolve(p, iid, &id, resolved).map(|_| ()), |_, _, _| {}, window, cx);
    }

    pub(crate) fn set_approved(&mut self, approve: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.write(move |c, p, iid| c.approve(p, iid, approve), |_, _, _| {}, window, cx);
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
    pub(crate) fn send_line_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((fi, old, new)) = self.commenting else { return };
        let body = self.boxes.line.read(cx).value().trim().to_string();
        let Some(r) = self.review.ready().cloned() else { return };
        let (Some(f), Some(refs)) = (r.files.get(fi), r.mr.diff_refs.clone()) else { return };
        if body.is_empty() {
            return;
        }
        let (old_path, new_path) = (f.meta.old_path.clone(), f.meta.new_path.clone());
        self.write(
            move |c, p, iid| {
                let refs: DiffRefs = refs;
                c.line_comment(p, iid, &refs, &old_path, &new_path, old, new, &body).map(|_| ())
            },
            |v, window, cx| {
                v.boxes.line.update(cx, |b, cx| b.set_value("", window, cx));
                v.commenting = None;
            },
            window,
            cx,
        );
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
        let enabled = !empty && !self.busy;
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
                                primary("gl-comment", if self.busy { "Commenting…" } else { "Comment" }, enabled)
                                    .when(enabled, |d| d.on_click(cx.listener(|v, _, window, cx| v.post_comment(window, cx)))),
                            ),
                    ),
            )
    }

    /// Under a thread: the reply placeholder (or the open reply box) and
    /// "Resolve thread" / "Reopen thread", as GitLab has them.
    pub(crate) fn reply_bar(&self, id: &str, resolvable: bool, resolved: bool, cx: &mut Context<Self>) -> Div {
        let open = self.replying.as_deref() == Some(id);
        let busy = self.busy;
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
        let enabled = !empty && !busy;
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
                        primary(SharedString::from(format!("send-{id}")), if busy { "Replying…" } else { "Reply" }, enabled)
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
        let enabled = !empty && !self.busy;
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
                    .child(
                        primary("gl-line-send", if self.busy { "Adding…" } else { "Add comment now" }, enabled)
                            .when(enabled, |d| d.on_click(cx.listener(|v, _, window, cx| v.send_line_comment(window, cx)))),
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
            primary("gl-approve", if self.busy { "Approving…" } else { "Approve" }, !self.busy)
                .on_click(cx.listener(|v, _, window, cx| v.set_approved(true, window, cx)))
        }
    }
}
