//! The Commits tab's "Add previously merged commits" (GitLab's context
//! commits): commits already on the target branch shown with the merge
//! request for context. A panel lists the target branch's recent commits,
//! searchable, to tick and save; the ones added show above the merge
//! request's own, each removable.

#[allow(unused_imports)]
use crate::style::px;
use crate::{
    review::Review,
    style::*,
    types::Commit,
    view::{GitLabView, Load},
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputState};
use std::collections::HashSet;

pub struct ContextState {
    pub open: bool,
    pub search: Entity<InputState>,
    pub candidates: Load<Vec<Commit>>,
    pub ticked: HashSet<String>,
}

impl ContextState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        ContextState {
            open: false,
            search: cx
                .new(|cx| InputState::new(window, cx).placeholder("Search by commit title or SHA")),
            candidates: Load::Idle,
            ticked: HashSet::new(),
        }
    }
}

impl GitLabView {
    pub(crate) fn open_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.ready().cloned() else {
            return;
        };
        self.context.open = !self.context.open;
        self.context.ticked.clear();
        if !self.context.open {
            cx.notify();
            return;
        }
        let Some(c) = self.client.clone() else { return };
        self.context.candidates = Load::Loading;
        let (p, target) = (r.mr.project_id, r.mr.target_branch.clone());
        cx.spawn_in(window, async move |this, cx| {
            let got = cx
                .background_spawn(async move { c.branch_commits(p, &target) })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.context.candidates = match got {
                    Ok(l) => Load::Ready(l),
                    Err(_) => Load::Ready(vec![]),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Save the ticked commits as context, or (`remove`) take one away.
    pub(crate) fn save_context(
        &mut self,
        remove: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (shas, add) = match remove {
            Some(s) => (vec![s], false),
            None => (
                self.context.ticked.iter().cloned().collect::<Vec<_>>(),
                true,
            ),
        };
        if shas.is_empty() {
            return;
        }
        self.context.open = false;
        self.write(
            crate::edit::Write::Sidebar,
            None,
            move |c, p, iid| c.context_commits_change(p, iid, &shas, add),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Above the commits: the button, the panel, and the context commits.
    pub(crate) fn context_bar(&self, r: &Review, cx: &mut Context<Self>) -> Div {
        let editable = self.me.is_some() && !self.is_local() && r.mr.state == "opened";
        let mono = self.mono(cx);
        let mut out = div().flex().flex_col().gap_2().px_4().pt_3();
        if editable {
            out = out.child(
                div().flex().justify_end().child(
                    crate::edit::secondary("gl-context-add", "Add previously merged commits")
                        .on_click(cx.listener(|v, _, window, cx| v.open_context(window, cx))),
                ),
            );
        }
        if self.context.open {
            let q = self.context.search.read(cx).value().to_lowercase();
            let list: Vec<Commit> = match &self.context.candidates {
                Load::Ready(l) => l
                    .iter()
                    .filter(|cm| {
                        q.is_empty()
                            || cm.title.to_lowercase().contains(&q)
                            || cm.id.starts_with(&q)
                    })
                    .filter(|cm| !r.context_commits.iter().any(|x| x.id == cm.id))
                    .cloned()
                    .collect(),
                _ => vec![],
            };
            let n = self.context.ticked.len();
            out = out.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(CARD))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                        "Add or remove previously merged commits — {}",
                        r.mr.target_branch
                    )))
                    .child(
                        div()
                            .p_1()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(c(BORDER))
                            .bg(c(BG))
                            .child(Input::new(&self.context.search).appearance(false)),
                    )
                    .when(matches!(self.context.candidates, Load::Loading), |d| {
                        d.child(div().text_color(c(MUTED)).child("Loading…"))
                    })
                    .child(
                        div()
                            .id("gl-context-list")
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(list.into_iter().map(|cm| {
                                let on = self.context.ticked.contains(&cm.id);
                                let id = cm.id.clone();
                                div()
                                    .id(SharedString::from(format!("ctx-{}", cm.id)))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .py_1()
                                    .cursor_pointer()
                                    .child(icon(
                                        if on { "square-check" } else { "square" },
                                        16.,
                                        c(if on { FOCUS } else { MUTED }),
                                    ))
                                    .child(
                                        div()
                                            .flex_none()
                                            .font_family(mono.clone())
                                            .text_color(c(LINK))
                                            .child(cm.short_id.clone()),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_color(c(TEXT))
                                            .child(cm.title.clone()),
                                    )
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        if !v.context.ticked.remove(&id) {
                                            v.context.ticked.insert(id.clone());
                                        }
                                        cx.notify();
                                    }))
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                crate::edit::primary("gl-context-save", "Save changes", n > 0)
                                    .when(n > 0, |d| {
                                        d.on_click(cx.listener(|v, _, window, cx| {
                                            v.save_context(None, window, cx)
                                        }))
                                    }),
                            )
                            .child(
                                crate::edit::secondary("gl-context-cancel", "Cancel").on_click(
                                    cx.listener(|v, _, _, cx| {
                                        v.context.open = false;
                                        cx.notify();
                                    }),
                                ),
                            ),
                    ),
            );
        }
        if !r.context_commits.is_empty() {
            out = out
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Previously merged commits"),
                )
                .children(r.context_commits.iter().map(|cm| {
                    let sha = cm.id.clone();
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .py_1()
                        .border_b_1()
                        .border_color(c(RAISED))
                        .child(
                            div()
                                .flex_none()
                                .font_family(mono.clone())
                                .text_color(c(LINK))
                                .child(cm.short_id.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(c(TEXT))
                                .child(cm.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(c(MUTED))
                                .child(cm.author_name.clone()),
                        )
                        .when(editable, |d| {
                            d.child(
                                crate::edit::icon_button(
                                    SharedString::from(format!("ctx-remove-{}", cm.id)),
                                    app_icon("close", 12., c(MUTED)),
                                    "Remove",
                                )
                                .on_click(cx.listener(
                                    move |v, _, window, cx| {
                                        v.save_context(Some(sha.clone()), window, cx)
                                    },
                                )),
                            )
                        })
                }));
        }
        out
    }
}
