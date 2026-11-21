//! GitLab Duo on a merge request, where the server has it (an Enterprise
//! GitLab with Duo: `aiAction` in its GraphQL), as GitLab's page offers it:
//! "Request review from GitLab Duo" by the reviewers, "Summarize my review"
//! in the review panel, "Generate commit message" when merging,
//! "Summarize code changes" in a new merge request's description, and
//! "Ask GitLab Duo" about the merge request from the ⋮ menu. Each asks
//! Duo (`aiAction`) and waits for its answer (`aiMessages`) in the
//! background; nothing of it shows on a GitLab without Duo.

#[allow(unused_imports)]
use crate::style::px;
use crate::{review::Review, style::*, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputState};
use serde_json::json;

/// What Duo is doing in a tab, and its last chat answer.
pub struct DuoState {
    /// The action waiting for Duo, by name ("summarize", "commit", …).
    pub busy: Option<&'static str>,
    pub error: Option<String>,
    /// "Ask GitLab Duo": open, the question, and the answers so far.
    pub chat_open: bool,
    pub ask: Entity<InputState>,
    pub answers: Vec<(String, String)>,
}

impl DuoState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        DuoState {
            busy: None,
            error: None,
            chat_open: false,
            ask: cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .auto_grow(2, 8)
                    .placeholder("Ask GitLab Duo about this merge request…")
            }),
            answers: vec![],
        }
    }
}

/// Where an answer goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Into_ {
    ReviewSummary,
    CommitMessage,
    NewMrDescription,
    Chat,
}

fn gid(kind: &str, id: u64) -> String {
    format!("gid://gitlab/{kind}/{id}")
}

impl GitLabView {
    pub(crate) fn duo_here(&self) -> bool {
        self.review.ready().is_some_and(|r| r.premium.duo) && self.me.is_some()
    }

    /// Ask Duo `input` in the background and put its answer `into` its place.
    pub(crate) fn ask_duo(
        &mut self,
        what: &'static str,
        input: serde_json::Value,
        into: Into_,
        question: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.client.clone() else { return };
        self.duo.busy = Some(what);
        self.duo.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let got = cx.background_spawn(async move { c.ask_duo(input) }).await;
            let _ = this.update_in(cx, |v, window, cx| {
                v.duo.busy = None;
                match got {
                    Ok(text) => {
                        let field = match into {
                            Into_::ReviewSummary => Some(v.more.summary.clone()),
                            Into_::CommitMessage => Some(v.more.merge_msg.clone()),
                            Into_::NewMrDescription => Some(v.new_mr.description.clone()),
                            Into_::Chat => None,
                        };
                        match field {
                            Some(f) => f.update(cx, |b, cx| b.set_value(text, window, cx)),
                            None => v.duo.answers.push((question.unwrap_or_default(), text)),
                        }
                    }
                    Err(e) => v.duo.error = Some(format!("{e:#}")),
                }
                v.rebuild_rows(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Request review from GitLab Duo".
    pub(crate) fn request_duo_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.write(
            crate::edit::Write::Sidebar,
            None,
            |c, p, iid| c.request_duo_review(p, iid),
            |_, _, _| {},
            window,
            cx,
        );
    }

    pub(crate) fn duo_summarize_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.ready() else { return };
        let input = json!({ "summarizeReview": { "resourceId": gid("MergeRequest", r.mr.id) } });
        self.ask_duo("summarize", input, Into_::ReviewSummary, None, window, cx);
    }

    pub(crate) fn duo_commit_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.ready() else { return };
        let input =
            json!({ "generateCommitMessage": { "resourceId": gid("MergeRequest", r.mr.id) } });
        self.more.edit_message = true;
        self.ask_duo("commit", input, Into_::CommitMessage, None, window, cx);
    }

    pub(crate) fn duo_describe_new_mr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.new_mr.project else { return };
        let v = |b: &Entity<InputState>, cx: &App| b.read(cx).value().trim().to_string();
        let (source, target) = (v(&self.new_mr.source, cx), v(&self.new_mr.target, cx));
        let input = json!({
            "projectId": gid("Project", p),
            "summarizeNewMergeRequest": {
                "resourceId": gid("Project", p),
                "sourceProjectId": p.to_string(),
                "sourceBranch": source,
                "targetBranch": target,
            }
        });
        self.ask_duo("describe", input, Into_::NewMrDescription, None, window, cx);
    }

    pub(crate) fn duo_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.ready() else { return };
        let q = self.duo.ask.read(cx).value().trim().to_string();
        if q.is_empty() {
            return;
        }
        self.duo.ask.update(cx, |b, cx| b.set_value("", window, cx));
        let input = json!({ "chat": { "content": q, "resourceId": gid("MergeRequest", r.mr.id) } });
        self.ask_duo("chat", input, Into_::Chat, Some(q), window, cx);
    }

    /// A small Duo button.
    pub(crate) fn duo_button(
        &self,
        id: &'static str,
        label: &'static str,
        busy_as: &'static str,
        f: fn(&mut GitLabView, &mut Window, &mut Context<GitLabView>),
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let here = self.duo_here() || (id == "gl-duo-describe" && self.new_mr_duo());
        here.then(|| {
            let busy = self.duo.busy == Some(busy_as);
            crate::edit::secondary(
                id,
                if busy {
                    "GitLab Duo is writing…"
                } else {
                    label
                },
            )
            .gap_1()
            .when(!busy, |d| {
                d.on_click(cx.listener(move |v, _, window, cx| f(v, window, cx)))
            })
        })
        // Its own width, not the column's.
        .map(|b| div().flex().child(b))
    }

    /// The new merge request form's project is on a GitLab with Duo (the
    /// last review loaded says so for this host).
    fn new_mr_duo(&self) -> bool {
        self.duo_seen && self.me.is_some()
    }

    /// "Ask GitLab Duo": the conversation and the question box.
    pub(crate) fn duo_chat(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        if !self.duo.chat_open || !self.duo_here() {
            return None;
        }
        let busy = self.duo.busy == Some("chat");
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded(px(12.))
                .bg(c(RAISED))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("GitLab Duo"))
                        .child(
                            crate::edit::icon_button(
                                "gl-duo-close",
                                app_icon("close", 12., c(MUTED)),
                                "Close",
                            )
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.duo.chat_open = false;
                                cx.notify();
                            })),
                        ),
                )
                .children(self.duo.answers.iter().enumerate().map(|(i, (q, a))| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_color(c(MUTED)).child(q.clone()))
                        .child(self.markdown(("duo-answer", i), a, &[], None, window, cx))
                }))
                .children(
                    self.duo
                        .error
                        .clone()
                        .map(|e| div().text_color(c(RED)).child(e)),
                )
                .child(
                    div()
                        .p_2()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(c(BORDER))
                        .bg(c(CARD))
                        .child(Input::new(&self.duo.ask).appearance(false)),
                )
                .child(
                    crate::edit::primary(
                        "gl-duo-send",
                        if busy {
                            "GitLab Duo is writing…"
                        } else {
                            "Ask"
                        },
                        !busy,
                    )
                    .when(!busy, |d| {
                        d.on_click(cx.listener(|v, _, window, cx| v.duo_ask(window, cx)))
                    }),
                ),
        )
    }

    /// "Request review from GitLab Duo", under the reviewers.
    pub(crate) fn duo_review_link(
        &self,
        r: &Review,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        (self.duo_here()
            && r.mr.state == "opened"
            && !r.mr.reviewers.iter().any(|u| u.username == "GitLabDuo"))
        .then(|| {
            div()
                .id("gl-duo-review")
                .text_color(c(LINK))
                .cursor_pointer()
                .hover(|s| s.underline())
                .child("Request review from GitLab Duo")
                .on_click(cx.listener(|v, _, window, cx| v.request_duo_review(window, cx)))
        })
    }
}
