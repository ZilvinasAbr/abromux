//! The rest of what GitLab's merge request page does, beyond reviewing:
//!
//! - suggestions: "Apply suggestion", and "Add suggestion to batch" then
//!   "Apply N suggestions", in one commit each time;
//! - the header's to-do ("Add a to-do item" / "Mark as done") and
//!   notifications bell, and its Code dropdown (the checkout commands,
//!   the Web IDE, patches and the plain diff);
//! - the ⋮ menu's Lock discussion;
//! - the merge widget's "Edit commit message", and once merged, Revert and
//!   Cherry-pick (onto a branch, or in a new merge request).

#[allow(unused_imports)]
use crate::style::px;
use crate::{edit::Write, optimistic::Patch, review::Review, style::*, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputState};
use serde_json::json;

/// Revert or cherry-pick, being asked where to.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub cherry: bool,
    /// Start a new merge request with the change (else commit to the branch).
    pub new_mr: bool,
}

/// This module's state in a tab.
pub struct More {
    /// Suggestions added to the batch, by id.
    pub batch: Vec<u64>,
    /// The header's Code dropdown is open.
    pub code_menu: bool,
    /// The merge widget's "Edit commit message" is ticked, and its fields.
    pub edit_message: bool,
    pub merge_msg: Entity<InputState>,
    pub squash_msg: Entity<InputState>,
    /// The commit message of an applied suggestion (empty: GitLab's).
    pub suggestion_msg: Entity<InputState>,
    /// The Apply dropdown open under a suggestion, by id.
    pub applying: Option<u64>,
    /// Revert or Cherry-pick open, and the branch typed for it.
    pub pick: Option<Pick>,
    pub branch: Entity<InputState>,
    /// What the last revert or cherry-pick did, to say so.
    pub picked: Option<String>,
    /// A comment on several lines: the first (the form is under the last).
    pub range_from: Option<crate::edit::LineKey>,
    /// The "Commenting on lines" start picker is open.
    pub range_menu: bool,
    /// Time tracking's form open, and its fields (duration, summary).
    pub time_open: bool,
    pub time_spent: Entity<InputState>,
    pub time_summary: Entity<InputState>,
    /// "Show whitespace changes" turned off.
    pub hide_whitespace: bool,
    /// Boxes showing their Preview instead of the text.
    pub previewing: std::collections::HashSet<crate::compose::MdBox>,
    /// "Your review": the panel open, its verdict, and the summary.
    pub review_panel: bool,
    pub verdict: Verdict,
    pub summary: Entity<InputState>,
    /// The merge request (global id) the fields are of.
    of: Option<u64>,
}

impl More {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let field = |rows: usize, placeholder: &'static str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(rows > 1)
                    .auto_grow(rows, 12)
                    .placeholder(placeholder)
            })
        };
        More {
            batch: vec![],
            code_menu: false,
            edit_message: false,
            merge_msg: field(4, "Merge commit message", window, cx),
            squash_msg: field(3, "Squash commit message", window, cx),
            suggestion_msg: field(1, "Apply suggestion commit message", window, cx),
            applying: None,
            pick: None,
            branch: cx.new(|cx| InputState::new(window, cx).placeholder("Branch")),
            picked: None,
            previewing: Default::default(),
            hide_whitespace: false,
            time_open: false,
            time_spent: cx.new(|cx| InputState::new(window, cx).placeholder("Time, e.g. 1h 30m")),
            time_summary: cx
                .new(|cx| InputState::new(window, cx).placeholder("Summary (optional)")),
            range_from: None,
            range_menu: false,
            review_panel: false,
            verdict: Verdict::Comment,
            summary: field(3, "Add optional summary content…", window, cx),
            of: None,
        }
    }

    /// The merge's commit message, when "Edit commit message" is ticked:
    /// the squashed commit's with `squash`.
    pub fn merge_message(&self, _r: &Review, squash: bool, cx: &App) -> Option<String> {
        if !self.edit_message {
            return None;
        }
        let b = if squash {
            &self.squash_msg
        } else {
            &self.merge_msg
        };
        Some(b.read(cx).value().to_string()).filter(|m| !m.trim().is_empty())
    }

    pub fn fields(&self) -> [&Entity<InputState>; 7] {
        [
            &self.time_spent,
            &self.time_summary,
            &self.summary,
            &self.merge_msg,
            &self.squash_msg,
            &self.suggestion_msg,
            &self.branch,
        ]
    }
}

/// GitLab's default merge commit message.
pub fn default_merge_message(r: &Review) -> String {
    let m = &r.mr;
    let mut s = format!(
        "Merge branch '{}' into '{}'\n\n{}",
        m.source_branch, m.target_branch, m.title
    );
    let closes: Vec<String> = r.closes.iter().map(|i| format!("#{}", i.iid)).collect();
    if !closes.is_empty() {
        s.push_str(&format!("\n\nCloses {}", closes.join(", ")));
    }
    s.push_str(&format!("\n\nSee merge request {}", m.references.full));
    s
}

/// The commands GitLab's "Check out branch" gives.
pub fn checkout_commands(r: &Review) -> String {
    // GitLab's modal: fetch, then a local branch tracking the remote one.
    let b = &r.mr.source_branch;
    format!("git fetch origin\ngit checkout -B '{b}' 'origin/{b}'")
}

impl GitLabView {
    /// One of this module's fields has the keys.
    pub(crate) fn more_typing(&self, window: &Window, cx: &App) -> bool {
        self.more
            .fields()
            .iter()
            .chain(self.new_mr.fields().iter())
            .any(|b| b.read(cx).focus_handle(cx).is_focused(window))
    }

    /// The fields belong to the merge request on screen: another one
    /// showing starts them afresh.
    pub(crate) fn more_for(&mut self, r: &Review, window: &mut Window, cx: &mut Context<Self>) {
        if self.more.of == Some(r.mr.id) {
            return;
        }
        self.more.of = Some(r.mr.id);
        self.more.batch.clear();
        self.more.applying = None;
        self.more.pick = None;
        self.more.picked = None;
        self.more.edit_message = false;
        let merge =
            r.mr.merge_commit_message
                .clone()
                .unwrap_or_else(|| default_merge_message(r));
        let squash =
            r.mr.squash_commit_message
                .clone()
                .unwrap_or_else(|| r.mr.title.clone());
        let target = r.mr.target_branch.clone();
        self.more
            .merge_msg
            .update(cx, |b, cx| b.set_value(merge, window, cx));
        self.more
            .squash_msg
            .update(cx, |b, cx| b.set_value(squash, window, cx));
        self.more
            .branch
            .update(cx, |b, cx| b.set_value(target, window, cx));
    }

    // ---- suggestions --------------------------------------------------

    /// Apply suggestions `ids` in one commit (GitLab's own message unless
    /// one was typed). They show as applied at once.
    pub(crate) fn apply_suggestions(
        &mut self,
        ids: Vec<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        let msg = self.more.suggestion_msg.read(cx).value().trim().to_string();
        self.more
            .suggestion_msg
            .update(cx, |b, cx| b.set_value("", window, cx));
        self.more.batch.retain(|b| !ids.contains(b));
        self.more.applying = None;
        let run = ids.clone();
        self.write(
            Write::Suggestion,
            Some(Patch::Suggestions { ids }),
            move |c, _, _| {
                c.apply_suggestions(
                    &run,
                    Some(&msg).filter(|m| !m.is_empty()).map(|m| m.as_str()),
                )
            },
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Add suggestion `id` to the batch, or take it out.
    pub(crate) fn toggle_batch(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(i) = self.more.batch.iter().position(|b| *b == id) {
            self.more.batch.remove(i);
        } else {
            self.more.batch.push(id);
        }
        self.more.applying = None;
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// The buttons in a suggestion's header, as GitLab has them: Apply
    /// suggestion (a dropdown with the commit message) and Add suggestion
    /// to batch; with a batch, Apply N suggestions and Remove from batch.
    pub(crate) fn suggestion_buttons(
        &self,
        s: &crate::types::Suggestion,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let r = self.review.ready()?;
        if s.applied
            || !s.appliable
            || s.id == 0
            || self.me.is_none()
            || self.is_local()
            || r.mr.state != "opened"
        {
            return None;
        }
        let id = s.id;
        let in_batch = self.more.batch.contains(&id);
        let n = self.more.batch.len();
        let busy = self.busy == Some(Write::Suggestion);
        let small = |el: Stateful<Div>| el.h(px(24.)).px_2().text_size(px(12.));
        let mut row = div().flex().items_center().gap_2();
        if in_batch {
            row = row
                .child(
                    small(crate::edit::secondary(
                        SharedString::from(format!("gl-sugg-unbatch-{id}")),
                        "Remove from batch",
                    ))
                    .on_click(cx.listener(move |v, _, _, cx| v.toggle_batch(id, cx))),
                )
                .child(
                    small(crate::edit::primary(
                        SharedString::from(format!("gl-sugg-apply-batch-{id}")),
                        "Apply suggestions",
                        !busy,
                    ))
                    .child(crate::style::count_badge(n))
                    .gap_1()
                    .when(!busy, |d| {
                        d.on_click(cx.listener(|v, _, window, cx| {
                            let ids = v.more.batch.clone();
                            v.apply_suggestions(ids, window, cx)
                        }))
                    }),
                );
        } else {
            let open = self.more.applying == Some(id);
            row = row
                .child(
                    small(crate::edit::secondary(
                        SharedString::from(format!("gl-sugg-batch-{id}")),
                        "Add suggestion to batch",
                    ))
                    .on_click(cx.listener(move |v, _, _, cx| v.toggle_batch(id, cx))),
                )
                .child(
                    div()
                        .relative()
                        .child(
                            small(crate::edit::secondary(
                                SharedString::from(format!("gl-sugg-apply-{id}")),
                                "Apply suggestion",
                            ))
                            .gap_1()
                            .child(app_icon("chevron-down", 12., c(TEXT)))
                            .on_click(cx.listener(
                                move |v, _, window, cx| {
                                    v.more.applying = if v.more.applying == Some(id) {
                                        None
                                    } else {
                                        Some(id)
                                    };
                                    if v.more.applying.is_some() {
                                        v.more
                                            .suggestion_msg
                                            .update(cx, |b, cx| b.focus(window, cx));
                                    }
                                    v.rebuild_rows(cx);
                                    cx.notify();
                                },
                            )),
                        )
                        .children(open.then(|| self.apply_dropdown(id, busy, cx))),
                );
        }
        Some(row)
    }

    /// "Apply suggestion"'s dropdown: the commit message, and Apply.
    fn apply_dropdown(&self, id: u64, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        deferred(
            div()
                .id(SharedString::from(format!("gl-sugg-menu-{id}")))
                .absolute()
                .top(px(30.))
                .right_0()
                .w(px(320.))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(8.))
                .border_1()
                .border_color(c(BORDER))
                .bg(c(CARD))
                .shadow_lg()
                .occlude()
                .text_size(px(14.))
                .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                    v.more.applying = None;
                    v.rebuild_rows(cx);
                    cx.notify();
                }))
                .child(Input::new(&self.more.suggestion_msg))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(c(MUTED))
                        .child("Leave it empty for GitLab's default message."),
                )
                .child(
                    crate::edit::primary(
                        SharedString::from(format!("gl-sugg-do-apply-{id}")),
                        if busy { "Applying…" } else { "Apply" },
                        !busy,
                    )
                    .justify_center()
                    .when(!busy, |d| {
                        d.on_click(cx.listener(move |v, _, window, cx| {
                            v.apply_suggestions(vec![id], window, cx)
                        }))
                    }),
                ),
        )
        .with_priority(2)
    }

    // ---- the header: to-do, notifications, Code ------------------------

    /// Add it to your to-dos, or mark them done.
    pub(crate) fn toggle_todo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.ready() else { return };
        let todos = r.todos.clone();
        let add = todos.is_empty();
        self.write(
            Write::Todo,
            Some(Patch::Todo { add }),
            move |c, p, iid| {
                if add {
                    c.add_todo(p, iid)
                } else {
                    todos.iter().try_for_each(|t| c.todo_done(*t))
                }
            },
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Turn its notifications on or off.
    pub(crate) fn set_subscribed(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.write(
            Write::Sidebar,
            Some(Patch::Subscribed { on }),
            move |c, p, iid| c.subscribe(p, iid, on),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Lock the discussion (only members comment), or unlock it.
    pub(crate) fn set_locked(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.mr_menu = false;
        self.write(
            Write::MrState,
            Some(Patch::Fields {
                fields: json!({ "discussion_locked": on }),
                people: vec![],
            }),
            move |c, p, iid| {
                c.update(p, iid, json!({ "discussion_locked": on }))
                    .map(|_| ())
            },
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// The header's buttons right of Edit, as GitLab 19 has them: the
    /// to-do and the notifications bell.
    pub(crate) fn header_toggles(&self, r: &Review, cx: &mut Context<Self>) -> Option<Div> {
        if self.me.is_none() || self.is_local() {
            return None;
        }
        let has_todo = !r.todos.is_empty();
        let subscribed = r.mr.subscribed.unwrap_or(false);
        let square = |el: Stateful<Div>| el.w(px(32.)).px_0().justify_center();
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    square(crate::edit::secondary("gl-todo", ""))
                        .child(icon(
                            if has_todo { "todo-done" } else { "todo-add" },
                            16.,
                            c(TEXT),
                        ))
                        .tooltip(hint(if has_todo {
                            "Mark as done"
                        } else {
                            "Add a to-do item"
                        }))
                        .on_click(cx.listener(|v, _, window, cx| v.toggle_todo(window, cx))),
                )
                .child(
                    square(crate::edit::secondary("gl-subscribe", ""))
                        .child(icon(
                            if subscribed { "bell" } else { "bell-off" },
                            16.,
                            c(TEXT),
                        ))
                        .tooltip(hint(if subscribed {
                            "Notifications on: turn off"
                        } else {
                            "Notifications off: turn on"
                        }))
                        .on_click(cx.listener(move |v, _, window, cx| {
                            v.set_subscribed(!subscribed, window, cx)
                        })),
                ),
        )
    }

    /// The header's Code dropdown: the commands to check out the branch
    /// (copied), the Web IDE, and the patches and plain diff.
    pub(crate) fn code_button(&self, r: &Review, cx: &mut Context<Self>) -> Option<Div> {
        if self.is_local() || r.mr.web_url.is_empty() {
            return None;
        }
        let web = r.mr.web_url.clone();
        let project = web.split("/-/").next().unwrap_or_default().to_string();
        let path = project
            .strip_prefix(self.host().trim_end_matches('/'))
            .unwrap_or(&project)
            .trim_start_matches('/')
            .to_string();
        let ide = format!(
            "{}/-/ide/project/{path}/merge_requests/{}",
            self.host().trim_end_matches('/'),
            r.mr.iid
        );
        let commands = checkout_commands(r);
        let open = self.more.code_menu;
        let item = |id: &'static str, label: &'static str, ic: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .cursor_pointer()
                .hover(|st| st.bg(c(RAISED)))
                .text_color(c(TEXT))
                .child(icon(ic, 14., c(MUTED)))
                .child(label)
        };
        let heading = |t: &'static str| {
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(MUTED))
                .child(t)
        };
        let close = |v: &mut GitLabView, cx: &mut Context<GitLabView>| {
            v.more.code_menu = false;
            cx.notify();
        };
        let menu = open.then(|| {
            let c2 = commands.clone();
            let (patch, diff) = (format!("{web}.patch"), format!("{web}.diff"));
            let ide = ide.clone();
            deferred(
                div()
                    .id("gl-code-menu")
                    .absolute()
                    .top(px(36.))
                    .right_0()
                    .min_w(px(240.))
                    .py_1()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(CARD))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(move |v, _, _, cx| close(v, cx)))
                    .child(
                        item(
                            "gl-code-copy-checkout",
                            "Check out branch (copy commands)",
                            "copy",
                        )
                        .on_click(cx.listener(move |v, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(c2.clone()));
                            close(v, cx);
                        })),
                    )
                    .child(
                        item("gl-code-ide", "Open in Web IDE", "external-link").on_click(
                            cx.listener(move |v, _, window, cx| {
                                close(v, cx);
                                abromux_plugin::open_url(&ide, window, cx);
                            }),
                        ),
                    )
                    .child(heading("Download"))
                    .child(
                        item("gl-code-patch", "Patches", "download").on_click(cx.listener(
                            move |v, _, window, cx| {
                                close(v, cx);
                                abromux_plugin::open_url(&patch, window, cx);
                            },
                        )),
                    )
                    .child(
                        item("gl-code-diff", "Plain diff", "download").on_click(cx.listener(
                            move |v, _, window, cx| {
                                close(v, cx);
                                abromux_plugin::open_url(&diff, window, cx);
                            },
                        )),
                    ),
            )
            .with_priority(1)
        });
        Some(
            div()
                .relative()
                .child(
                    crate::edit::secondary("gl-code", "Code")
                        .gap_1()
                        .child(app_icon("chevron-down", 14., c(TEXT)))
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.more.code_menu = !v.more.code_menu;
                            cx.notify();
                        })),
                )
                .children(menu),
        )
    }

    // ---- merging: the commit message; merged: revert, cherry-pick -------

    /// The merge widget's "Edit commit message" and its fields.
    pub(crate) fn merge_message_editor(
        &self,
        r: &Review,
        squash: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let on = self.more.edit_message;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .child(
                crate::edit::checkbox("gl-merge-edit-message", on, "Edit commit message").on_click(
                    cx.listener(move |v, _, _, cx| {
                        v.more.edit_message = !on;
                        cx.notify();
                    }),
                ),
            )
            .when(on, |d| {
                let field = |label: &'static str, b: &Entity<InputState>| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(label))
                        .child(Input::new(b))
                };
                d.when(squash || r.mr.squash_on_merge, |d| {
                    d.child(field("Squash commit message", &self.more.squash_msg))
                })
                .child(field("Merge commit message", &self.more.merge_msg))
            })
    }

    /// Open Revert or Cherry-pick's form (or close it).
    pub(crate) fn open_pick(&mut self, cherry: bool, cx: &mut Context<Self>) {
        self.more.pick = match &self.more.pick {
            Some(p) if p.cherry == cherry => None,
            _ => Some(Pick {
                cherry,
                new_mr: true,
            }),
        };
        self.more.picked = None;
        cx.notify();
    }

    /// Revert the merge, or cherry-pick it, onto the branch typed: GitLab's
    /// buttons on a merged merge request. With `new_mr`, as GitLab's "Start
    /// a new merge request with these changes": onto a new branch made from
    /// that one, with a merge request into it.
    pub(crate) fn run_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(r), Some(pick)) = (self.review.ready(), self.more.pick.clone()) else {
            return;
        };
        let Some(sha) =
            r.mr.merge_commit_sha
                .clone()
                .or_else(|| r.mr.squash_commit_sha.clone())
                .or_else(|| r.mr.sha.clone())
        else {
            self.write_error = Some("GitLab didn't say which commit merged it".into());
            return;
        };
        let branch = self.more.branch.read(cx).value().trim().to_string();
        if branch.is_empty() {
            return;
        }
        let title = r.mr.title.clone();
        let reference = r.mr.references.full.clone();
        let short = sha.get(..8).unwrap_or(&sha).to_string();
        let what = if pick.cherry { "cherry-pick" } else { "revert" };
        let done = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let said = done.clone();
        self.more.pick = None;
        self.write(
            Write::Pick,
            None,
            move |c, p, _| {
                let onto = if pick.new_mr {
                    let new = format!("{what}-{short}");
                    c.create_branch(p, &new, &branch)?;
                    new
                } else {
                    branch.clone()
                };
                c.pick_commit(p, &sha, &onto, pick.cherry)?;
                let msg = if pick.new_mr {
                    let t = if pick.cherry {
                        format!("Cherry-pick '{title}' into '{branch}'")
                    } else {
                        format!("Revert \"{title}\"")
                    };
                    let mr = c.create_mr(
                        p,
                        &onto,
                        &branch,
                        &t,
                        &format!(
                            "This {} {reference}.",
                            if pick.cherry {
                                "cherry-picks"
                            } else {
                                "reverts"
                            }
                        ),
                    )?;
                    format!("Opened {}: {}", mr.references.full, mr.web_url)
                } else {
                    format!("Committed the {what} to {onto}.")
                };
                *said.lock().unwrap() = Some(msg);
                Ok(())
            },
            |_, _, _| {},
            window,
            cx,
        );
        // Say what happened once it has.
        cx.spawn(async move |this, cx| {
            for _ in 0..600 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let got = done.lock().unwrap().take();
                if let Some(msg) = got {
                    let _ = this.update(cx, |v, cx| {
                        v.more.picked = Some(msg);
                        cx.notify();
                    });
                    return;
                }
            }
        })
        .detach();
    }

    /// Revert and Cherry-pick on a merged merge request, and the form one
    /// opens.
    pub(crate) fn pick_buttons(&self, r: &Review, cx: &mut Context<Self>) -> Option<Div> {
        if self.me.is_none() || self.is_local() || r.mr.state != "merged" {
            return None;
        }
        let busy = self.busy == Some(Write::Pick);
        let form = self.more.pick.clone().map(|p| {
            let new_mr = p.new_mr;
            div()
                .flex()
                .flex_col()
                .gap_2()
                .w_full()
                .pt_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(if p.cherry {
                    "Pick into branch"
                } else {
                    "Revert in branch"
                }))
                .child(Input::new(&self.more.branch))
                .child(
                    crate::edit::checkbox(
                        "gl-pick-new-mr",
                        new_mr,
                        "Start a new merge request with these changes",
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if let Some(p) = v.more.pick.as_mut() {
                            p.new_mr = !new_mr;
                        }
                        cx.notify();
                    })),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            crate::edit::primary(
                                "gl-pick-go",
                                if p.cherry { "Cherry-pick" } else { "Revert" },
                                !busy,
                            )
                            .when(!busy, |d| {
                                d.on_click(cx.listener(|v, _, window, cx| v.run_pick(window, cx)))
                            }),
                        )
                        .child(crate::edit::secondary("gl-pick-cancel", "Cancel").on_click(
                            cx.listener(|v, _, _, cx| {
                                v.more.pick = None;
                                cx.notify();
                            }),
                        )),
                )
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .px_4()
                .pb_3()
                .pl(px(48.))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            crate::edit::secondary("gl-revert", "Revert")
                                .on_click(cx.listener(|v, _, _, cx| v.open_pick(false, cx))),
                        )
                        .child(
                            crate::edit::secondary("gl-cherry-pick", "Cherry-pick")
                                .on_click(cx.listener(|v, _, _, cx| v.open_pick(true, cx))),
                        ),
                )
                .children(form)
                .children(
                    self.more
                        .picked
                        .clone()
                        .map(|m| div().text_size(px(12.)).text_color(c(GREEN)).child(m)),
                ),
        )
    }
}

/// Pending comments' stand-in note ids: GitLab's draft id above this, below
/// the optimistic ones (`optimistic::is_pending_note`).
const DRAFT_BASE: u64 = 1 << 50;

/// The draft a stand-in note is, if it is one (and GitLab has it).
pub fn draft_of(note: u64) -> Option<u64> {
    (DRAFT_BASE..DRAFT_BASE * 4)
        .contains(&note)
        .then(|| note - DRAFT_BASE)
}

/// A note of your review not yet submitted.
pub fn is_draft(n: &crate::types::Note) -> bool {
    n.kind.as_deref() == Some("DraftNote")
}

/// Your review's pending comments shown where they'll go, as GitLab shows
/// them (with "Pending"): on their line, under the thread they reply to, or
/// in the activity. Stand-in threads are `optimistic-draft-…`, so nothing
/// but editing and deleting is offered on them.
pub fn place_drafts(r: &mut Review, me: &crate::types::User) {
    use crate::types::{Discussion, Note};
    let drafts = r.drafts.clone();
    for d in drafts {
        let note = Note {
            id: DRAFT_BASE + d.id,
            kind: Some("DraftNote".into()),
            body: d.note.clone(),
            author: me.clone(),
            created_at: crate::time::iso(crate::time::now()),
            position: d.position.clone(),
            ..Default::default()
        };
        let thread = |n: Note| Discussion {
            id: format!("optimistic-draft-{}", d.id),
            individual_note: n.position.is_none(),
            notes: vec![n],
        };
        if let Some(t) = d.discussion_id.as_deref() {
            let on_files = r.files.iter_mut().flat_map(|f| {
                f.threads
                    .iter_mut()
                    .map(|(_, _, d)| d)
                    .chain(f.outdated.iter_mut())
            });
            let found = r
                .general
                .iter_mut()
                .chain(r.outdated.iter_mut())
                .chain(on_files)
                .find(|x| x.id == t);
            if let Some(x) = found {
                x.notes.push(note);
                continue;
            }
            r.general.push(thread(note));
            continue;
        }
        let placed = d.position.as_ref().and_then(|p| {
            let f = r.files.iter_mut().find(|f| f.holds(p))?;
            let (h, l) = if p.position_type == "file" {
                (crate::review::FILE_THREAD, 0)
            } else {
                f.find(p)?
            };
            Some((f, h, l))
        });
        match placed {
            Some((f, h, l)) => {
                let at = f
                    .threads
                    .partition_point(|(fh, fl, _)| (*fh, *fl) <= (h, l));
                f.threads.insert(at, (h, l, thread(note)));
            }
            None => r.general.push(thread(note)),
        }
    }
}

/// What submitting your review says, as GitLab's "Review approval".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verdict {
    #[default]
    Comment,
    Approve,
    RequestChanges,
}

impl GitLabView {
    /// Change a pending comment (from its own Edit, `save_note`).
    pub(crate) fn save_draft(
        &mut self,
        id: u64,
        body: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let note = DRAFT_BASE + id;
        let back = body.clone();
        self.write(
            Write::EditNote,
            Some(Patch::EditDraft {
                id,
                body: Some(body.clone()),
            }),
            move |c, p, iid| c.edit_draft(p, iid, id, &body),
            move |v, window, cx| {
                v.editing_note = Some(note);
                v.boxes
                    .note
                    .update(cx, |b, cx| b.set_value(back, window, cx));
            },
            window,
            cx,
        );
    }

    /// Take a pending comment out of your review.
    pub(crate) fn delete_draft(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.write(
            Write::DeleteNote,
            Some(Patch::EditDraft { id, body: None }),
            move |c, p, iid| c.delete_draft(p, iid, id),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Submit your review as GitLab's "Your review" does: its pending
    /// comments, the summary as a comment, and the verdict (an approval,
    /// or "requested changes").
    pub(crate) fn submit_review_as(
        &mut self,
        verdict: Verdict,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let summary = self.more.summary.read(cx).value().trim().to_string();
        self.more
            .summary
            .update(cx, |b, cx| b.set_value("", window, cx));
        self.more.review_panel = false;
        let state = match verdict {
            Verdict::RequestChanges => Some("requested_changes".to_string()),
            Verdict::Comment => Some("reviewed".to_string()),
            Verdict::Approve => None,
        };
        let (s, st, back) = (summary.clone(), state.clone(), summary.clone());
        self.write(
            Write::SubmitReview,
            Some(Patch::Submit {
                summary: summary.clone(),
                state: state.clone(),
            }),
            move |c, p, iid| {
                let pending: Vec<String> =
                    c.draft_notes(p, iid)?.into_iter().map(|d| d.note).collect();
                c.publish_review(
                    p,
                    iid,
                    Some(&s).filter(|s| !s.is_empty()).map(|s| s.as_str()),
                    st.as_deref(),
                )?;
                // GitLab publishes in the background: wait (a little) until
                // the comments are threads, so the refresh after shows them.
                for _ in 0..10 {
                    let shown = c.discussions(p, iid)?;
                    let bodies: Vec<&str> = shown
                        .iter()
                        .flat_map(|d| &d.notes)
                        .map(|n| n.body.as_str())
                        .collect();
                    if pending.iter().all(|b| bodies.contains(&b.as_str())) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Ok(())
            },
            move |v, window, cx| {
                v.more
                    .summary
                    .update(cx, |b, cx| b.set_value(back, window, cx));
            },
            window,
            cx,
        );
        if verdict == Verdict::Approve {
            self.set_approved(true, window, cx);
        }
    }

    /// GitLab's "Your review" button, in the tab bar: its pending comments
    /// counted, and a click opens the submit panel.
    pub(crate) fn your_review_button(&self, r: &Review, cx: &mut Context<Self>) -> Option<Div> {
        let n = r.drafts.len();
        if self.me.is_none() || (self.is_local() && n == 0) || r.mr.state != "opened" && n == 0 {
            return None;
        }
        let (label, _) =
            crate::edit::busy_label(self.busy, Write::SubmitReview, "Your review", "Submitting…");
        let button = if n > 0 {
            crate::edit::primary("gl-your-review", label, true).child(count_badge(n))
        } else {
            crate::edit::secondary("gl-your-review", label)
        };
        Some(
            div()
                .relative()
                .child(button.gap_2().on_click(cx.listener(|v, _, window, cx| {
                    v.more.review_panel = !v.more.review_panel;
                    if v.more.review_panel {
                        v.more.summary.update(cx, |b, cx| b.focus(window, cx));
                    }
                    cx.notify();
                })))
                .children(self.more.review_panel.then(|| self.review_panel(r, cx))),
        )
    }

    /// The panel: Review approval (Comment, Approve, Request changes), the
    /// summary, Submit review and Continue review, then the pending
    /// comments, each with where it goes and a way to take it out.
    fn review_panel(&self, r: &Review, cx: &mut Context<Self>) -> impl IntoElement {
        let me = self.me.as_ref().map_or(0, |m| m.id);
        let approved = r.approvals.approved_by.iter().any(|a| a.user.id == me);
        let can_approve = r.approvals.user_can_approve && !approved;
        let pick = self.more.verdict;
        let option = |v: Verdict, title: &'static str, about: &'static str, live: bool| {
            let on = pick == v;
            div()
                .id(SharedString::from(format!("gl-verdict-{v:?}")))
                .flex()
                .gap_2()
                .when(live, |d| d.cursor_pointer())
                .when(!live, |d| d.opacity(0.5))
                .child(
                    div()
                        .mt(px(3.))
                        .size(px(14.))
                        .flex_none()
                        .rounded_full()
                        .border_1()
                        .border_color(c(if on { FOCUS } else { BORDER }))
                        .when(on, |d| d.border_4()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_color(c(TEXT)).child(title))
                        .child(div().text_size(px(12.)).text_color(c(MUTED)).child(about)),
                )
                .when(live, |d| {
                    d.on_click(cx.listener(move |view, _, _, cx| {
                        view.more.verdict = v;
                        cx.notify();
                    }))
                })
        };
        let busy = self.busy == Some(Write::SubmitReview);
        let pending = r.drafts.iter().map(|d| {
            let id = d.id;
            let place = match (&d.position, &d.discussion_id) {
                (_, Some(_)) => "Reply".to_string(),
                (Some(p), _) => format!(
                    "{}:{}",
                    p.new_path
                        .as_deref()
                        .or(p.old_path.as_deref())
                        .unwrap_or(""),
                    p.new_line
                        .or(p.old_line)
                        .map(|l| l.to_string())
                        .unwrap_or_default()
                ),
                _ => "Comment".to_string(),
            };
            let pending = crate::optimistic::is_pending_note(id);
            div()
                .flex()
                .items_start()
                .gap_2()
                .py_1()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(c(MUTED))
                                .truncate()
                                .child(place),
                        )
                        .child(
                            div()
                                .text_color(c(TEXT))
                                .truncate()
                                .child(d.note.lines().next().unwrap_or_default().to_string()),
                        ),
                )
                .when(!pending, |el| {
                    el.child(
                        crate::edit::icon_button(
                            SharedString::from(format!("gl-draft-delete-{id}")),
                            icon("remove", 14., c(MUTED)),
                            "Delete comment",
                        )
                        .on_click(
                            cx.listener(move |v, _, window, cx| v.delete_draft(id, window, cx)),
                        ),
                    )
                })
        });
        let n = r.drafts.len();
        deferred(
            div()
                .id("gl-review-panel")
                .absolute()
                .top(px(38.))
                .right_0()
                .w(px(420.))
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .rounded(px(8.))
                .border_1()
                .border_color(c(BORDER))
                .bg(c(CARD))
                .shadow_lg()
                .occlude()
                .text_size(px(14.))
                .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                    v.more.review_panel = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(HEADING))
                        .child("Review approval"),
                )
                .child(option(
                    Verdict::Comment,
                    "Comment",
                    "Submit general feedback without explicit approval.",
                    true,
                ))
                .child(option(
                    Verdict::Approve,
                    "Approve",
                    "Submit feedback and approve these changes.",
                    can_approve && !self.is_local(),
                ))
                .child(option(
                    Verdict::RequestChanges,
                    "Request changes",
                    "Submit feedback that should be addressed before merging.",
                    !self.is_local(),
                ))
                .child(Input::new(&self.more.summary))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            crate::edit::primary(
                                "gl-submit-review",
                                if busy {
                                    "Submitting…"
                                } else {
                                    "Submit review"
                                },
                                !busy,
                            )
                            .when(!busy, |d| {
                                d.on_click(cx.listener(move |v, _, window, cx| {
                                    let verdict = v.more.verdict;
                                    v.submit_review_as(verdict, window, cx)
                                }))
                            }),
                        )
                        .child(
                            crate::edit::secondary("gl-continue-review", "Continue review")
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.more.review_panel = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(div().h(px(1.)).bg(c(RAISED)))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(HEADING))
                        .child(match n {
                            0 => "No pending comments".to_string(),
                            1 => "1 pending comment".to_string(),
                            n => format!("{n} pending comments"),
                        }),
                )
                .children(pending),
        )
        .with_priority(2)
    }

    /// "Start a review" (or "Add to review" once one is under way) beside
    /// a reply's or the overview comment's button: the text goes into
    /// your review instead of out at once. `overview`: the comment box.
    pub(crate) fn to_review_button(
        &self,
        id: SharedString,
        enabled: bool,
        overview: bool,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let r = self.review.ready()?;
        if self.me.is_none() || self.is_local() || r.mr.state != "opened" {
            return None;
        }
        let label = if r.drafts.is_empty() {
            "Start a review"
        } else {
            "Add to review"
        };
        Some(crate::edit::secondary(id, label).when(enabled, |d| {
            d.on_click(cx.listener(move |v, _, window, cx| {
                if overview {
                    v.comment_to_review(window, cx)
                } else {
                    v.reply_to_review(window, cx)
                }
            }))
        }))
    }

    /// The overview's comment, into your review.
    pub(crate) fn comment_to_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.boxes.comment.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        self.boxes
            .comment
            .update(cx, |b, cx| b.set_value("", window, cx));
        let back = body.clone();
        self.write(
            Write::LineDraft,
            Some(Patch::Draft {
                body: body.clone(),
                position: None,
                discussion: None,
            }),
            move |c, p, iid| c.draft_comment(p, iid, &body, None, false).map(|_| ()),
            move |v, window, cx| {
                v.boxes
                    .comment
                    .update(cx, |b, cx| b.set_value(back, window, cx))
            },
            window,
            cx,
        );
    }

    /// The open reply, into your review.
    pub(crate) fn reply_to_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.replying.clone() else {
            return;
        };
        let body = self.boxes.reply.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        self.boxes
            .reply
            .update(cx, |b, cx| b.set_value("", window, cx));
        self.replying = None;
        let (thread, back) = (id.clone(), body.clone());
        self.write(
            Write::LineDraft,
            Some(Patch::Draft {
                body: body.clone(),
                position: None,
                discussion: Some(id.clone()),
            }),
            move |c, p, iid| c.draft_comment(p, iid, &body, Some(&id), false).map(|_| ()),
            move |v, window, cx| {
                v.replying = Some(thread);
                v.boxes
                    .reply
                    .update(cx, |b, cx| b.set_value(back, window, cx));
            },
            window,
            cx,
        );
        self.rebuild_rows(cx);
    }

    /// Shift-click on another line's comment button while writing one:
    /// the comment covers the lines between (the form moves to the last).
    pub(crate) fn extend_comment(
        &mut self,
        key: crate::edit::LineKey,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(at) = self.commenting else {
            return false;
        };
        let Some(r) = self.shown() else { return false };
        if at.0 != key.0 {
            return false;
        }
        let (Some(a), Some(b)) = (line_point(&r, at), line_point(&r, key)) else {
            return false;
        };
        let first = self.more.range_from.unwrap_or(at);
        let lo = line_point(&r, first).map_or(a.1, |p| p.1);
        // The far end from what's clicked stays.
        if b.1 < a.1 {
            self.more.range_from = Some(key);
        } else {
            self.more.range_from = Some(if lo < a.1 { first } else { at });
            self.commenting = Some(key);
        }
        if self.more.range_from == self.commenting {
            self.more.range_from = None;
        }
        self.more.range_menu = false;
        self.rebuild_rows(cx);
        cx.notify();
        true
    }

    /// The range being commented on, when it's more than a line.
    pub(crate) fn comment_range(&self) -> Option<crate::api::LineRange> {
        let r = self.shown()?;
        let end = self.commenting?;
        let start = self.more.range_from.filter(|s| *s != end)?;
        let (s, e) = (line_point(&r, start)?, line_point(&r, end)?);
        Some(crate::api::LineRange {
            start: s.0,
            end: e.0,
        })
    }

    /// The lines of the range as text (for a suggestion), and how many.
    pub(crate) fn range_text(&self) -> Option<(String, usize)> {
        let r = self.shown()?;
        let end = self.commenting?;
        let (fi, ..) = end;
        let lines: Vec<&crate::diff::Line> = r
            .files
            .get(fi)?
            .body()
            .hunks()
            .iter()
            .flat_map(|h| &h.lines)
            .collect();
        let e = line_point(&r, end)?.1;
        let s = self
            .more
            .range_from
            .and_then(|k| line_point(&r, k))
            .map_or(e, |p| p.1);
        // A suggestion replaces new lines: the removed ones aren't in it.
        let picked: Vec<String> = lines[s.min(e)..=e]
            .iter()
            .filter(|l| {
                l.kind != crate::diff::Kind::Removed && l.kind != crate::diff::Kind::NoNewline
            })
            .map(|l| l.text.clone())
            .collect();
        let n = picked.len().max(1);
        Some((picked.join("\n"), n))
    }

    /// The line form's head: "Commenting on line +15", or on lines from a
    /// start picked in GitLab's dropdown (the lines above, in the file).
    pub(crate) fn range_header(&self, cx: &mut Context<Self>) -> Option<Div> {
        let r = self.shown()?;
        let end = self.commenting?;
        let (fi, ..) = end;
        let label = |k: crate::edit::LineKey| match k {
            (_, _, Some(n)) => format!("+{n}"),
            (_, Some(o), None) => format!("-{o}"),
            _ => String::new(),
        };
        let lines: Vec<&crate::diff::Line> = r
            .files
            .get(fi)?
            .body()
            .hunks()
            .iter()
            .flat_map(|h| &h.lines)
            .collect();
        let e = line_point(&r, end)?.1;
        let start = self.more.range_from.unwrap_or(end);
        let text = if start == end {
            format!("Commenting on line {}", label(end))
        } else {
            format!("Commenting on lines {} to {}", label(start), label(end))
        };
        let open = self.more.range_menu;
        let menu = open.then(|| {
            let from = e.saturating_sub(30);
            let items = lines[from..=e]
                .iter()
                .filter(|l| l.kind != crate::diff::Kind::NoNewline)
                .map(|l| (fi, l.old, l.new, l.text.clone()))
                .collect::<Vec<_>>();
            deferred(
                div()
                    .id("gl-range-menu")
                    .absolute()
                    .top(px(26.))
                    .left_0()
                    .w(px(420.))
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .py_1()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(CARD))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                        v.more.range_menu = false;
                        cx.notify();
                    }))
                    .children(items.into_iter().map(|(fi, o, n, t)| {
                        let key = (fi, o, n);
                        let on = key == start;
                        div()
                            .id(SharedString::from(format!("gl-range-{o:?}-{n:?}")))
                            .flex()
                            .gap_2()
                            .px_3()
                            .py(px(2.))
                            .cursor_pointer()
                            .when(on, |d| d.bg(c(RAISED)))
                            .hover(|s| s.bg(c(RAISED)))
                            .child(
                                div()
                                    .w(px(44.))
                                    .flex_none()
                                    .text_color(c(MUTED))
                                    .child(label(key)),
                            )
                            .child(div().truncate().font_family(self.mono(cx)).child(t))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.more.range_from = (Some(key) != v.commenting).then_some(key);
                                v.more.range_menu = false;
                                v.rebuild_rows(cx);
                                cx.notify();
                            }))
                    })),
            )
            .with_priority(2)
        });
        Some(
            div()
                .relative()
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(13.))
                .text_color(c(MUTED))
                .child(
                    div()
                        .id("gl-range-pick")
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .hover(|s| s.text_color(c(TEXT)))
                        .child(text)
                        .child(app_icon("chevron-down", 12., c(MUTED)))
                        .tooltip(hint(
                            "Pick the first line (or shift-click another line's comment button)",
                        ))
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.more.range_menu = !v.more.range_menu;
                            cx.notify();
                        })),
                )
                .children(menu),
        )
    }

    /// Whether line `key` is in the range being commented on (drawn
    /// marked, as GitLab marks it).
    pub(crate) fn in_comment_range(&self, key: crate::edit::LineKey) -> bool {
        let (Some(end), Some(start)) = (self.commenting, self.more.range_from) else {
            return false;
        };
        let Some(r) = self.shown() else { return false };
        if key.0 != end.0 {
            return false;
        }
        match (
            line_point(&r, start),
            line_point(&r, end),
            line_point(&r, key),
        ) {
            (Some(s), Some(e), Some(k)) => (s.1.min(e.1)..=s.1.max(e.1)).contains(&k.1),
            _ => false,
        }
    }
}

impl GitLabView {
    /// Ask reviewer `u` for another review.
    pub(crate) fn rerequest(
        &mut self,
        u: &crate::types::User,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, name) = (u.id, u.username.clone());
        self.write(
            Write::Sidebar,
            Some(Patch::ReviewState {
                user: id,
                state: "unreviewed".into(),
            }),
            move |c, p, iid| c.rerequest_review(p, iid, &name),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// GitLab's ↻ by a reviewer who has reviewed.
    pub(crate) fn rerequest_button(
        &self,
        u: &crate::types::User,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let u = u.clone();
        crate::edit::icon_button(
            SharedString::from(format!("gl-rerequest-{}", u.id)),
            icon("retry", 14., c(MUTED)),
            "Re-request review",
        )
        .size(px(22.))
        .on_click(cx.listener(move |v, _, window, cx| v.rerequest(&u, window, cx)))
    }

    /// Time tracking's +: log time, or set the estimate.
    pub(crate) fn time_plus(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        (self.me.is_some() && !self.is_local()).then(|| {
            crate::edit::icon_button("gl-time-add", icon("plus", 15., c(TEXT)), "Add time entry")
                .size(px(24.))
                .on_click(cx.listener(|v, _, window, cx| {
                    v.more.time_open = !v.more.time_open;
                    if v.more.time_open {
                        v.more.time_spent.update(cx, |b, cx| b.focus(window, cx));
                    }
                    cx.notify();
                }))
                .into_any_element()
        })
    }

    /// Log the time typed (`estimate`: set it as the estimate instead).
    pub(crate) fn save_time(
        &mut self,
        estimate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let d = self.more.time_spent.read(cx).value().trim().to_string();
        if d.is_empty() {
            return;
        }
        let summary = self.more.time_summary.read(cx).value().trim().to_string();
        for b in [self.more.time_spent.clone(), self.more.time_summary.clone()] {
            b.update(cx, |b, cx| b.set_value("", window, cx));
        }
        self.more.time_open = false;
        self.write(
            Write::Sidebar,
            None,
            move |c, p, iid| {
                if estimate {
                    c.set_estimate(p, iid, &d)
                } else {
                    c.add_spent_time(p, iid, &d, &summary)
                }
            },
            |_, _, _| {},
            window,
            cx,
        );
    }

    pub(crate) fn time_form(&self, cx: &mut Context<Self>) -> Option<Div> {
        self.more.time_open.then(|| {
            let field = |b: &Entity<InputState>| {
                div()
                    .p_1()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(BG))
                    .child(Input::new(b).appearance(false))
            };
            div()
                .flex()
                .flex_col()
                .gap_2()
                .pt_2()
                .child(field(&self.more.time_spent))
                .child(field(&self.more.time_summary))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            crate::edit::primary("gl-time-save", "Add time", true).on_click(
                                cx.listener(|v, _, window, cx| v.save_time(false, window, cx)),
                            ),
                        )
                        .child(
                            crate::edit::secondary("gl-time-estimate", "Set estimate").on_click(
                                cx.listener(|v, _, window, cx| v.save_time(true, window, cx)),
                            ),
                        ),
                )
        })
    }
}

/// The review with whitespace-only changes hidden (diff.rs), its threads
/// placed again on the lines now shown.
pub fn hide_whitespace(r: &mut Review) {
    use crate::diff::Body;
    for f in &mut r.files {
        let Body::Text(hunks) = &f.body else { continue };
        f.body = Body::Text(std::sync::Arc::new(crate::diff::without_whitespace(hunks)));
        let threads = std::mem::take(&mut f.threads);
        for (_, _, d) in threads {
            let Some(p) = d.position().cloned() else {
                f.outdated.push(d);
                continue;
            };
            if p.position_type == "file" {
                f.threads.push((crate::review::FILE_THREAD, 0, d));
                continue;
            }
            let at = f.find(&p).or_else(|| {
                // A removed or added line that's unchanged now.
                f.body().hunks().iter().enumerate().find_map(|(hi, h)| {
                    h.lines
                        .iter()
                        .position(|l| {
                            (p.new_line.is_some() && l.new == p.new_line)
                                || (p.new_line.is_none()
                                    && p.old_line.is_some()
                                    && l.old == p.old_line)
                        })
                        .map(|li| (hi, li))
                })
            });
            match at {
                Some((h, l)) => f.threads.push((h, l, d)),
                None => f.outdated.push(d),
            }
        }
        f.threads.sort_by_key(|(h, l, _)| (*h, *l));
    }
}

/// Line `key`'s GitLab point, and its index among the file's lines.
pub fn line_point(r: &Review, key: crate::edit::LineKey) -> Option<(crate::api::LinePoint, usize)> {
    use crate::diff::Kind;
    let (fi, old, new) = key;
    let f = r.files.get(fi)?;
    let mut ix = 0;
    for h in f.body().hunks().iter() {
        // GitLab counts from the header as written: `-0,0` (a new file)
        // starts the old side at 0, where the parsed hunk says 1.
        let (mut o, mut n) = literal_starts(&h.header).unwrap_or((h.old_start, h.new_start));
        for l in &h.lines {
            let (op, np) = (o, n);
            match l.kind {
                Kind::Context => {
                    o += 1;
                    n += 1;
                }
                Kind::Added => n += 1,
                Kind::Removed => o += 1,
                Kind::NoNewline => {}
            }
            if l.old == old && l.new == new && l.kind != Kind::NoNewline {
                let kind = match l.kind {
                    Kind::Added => "new",
                    Kind::Removed => "old",
                    _ => "",
                };
                return Some((
                    crate::api::LinePoint {
                        old,
                        new,
                        old_pos: op,
                        new_pos: np,
                        kind,
                    },
                    ix,
                ));
            }
            ix += 1;
        }
    }
    None
}

/// A hunk header's starts as written (`@@ -0,0 +1,5 @@` → (0, 1)).
fn literal_starts(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace().skip(1);
    let mut start = |sign: char| -> Option<u32> {
        parts
            .next()?
            .strip_prefix(sign)?
            .split(',')
            .next()?
            .parse()
            .ok()
    };
    let old = start('-')?;
    let new = start('+')?;
    Some((old, new))
}

#[cfg(test)]
mod tests {
    #[test]
    fn line_codes_as_gitlab_writes_them() {
        assert_eq!(super::literal_starts("@@ -0,0 +1,5 @@"), Some((0, 1)));
        assert_eq!(super::literal_starts("@@ -3,5 +3,5 @@ fn x"), Some((3, 3)));
        // Read off GitLab 19.5's discussions.json for the seeded !1.
        assert_eq!(
            crate::api::line_code("src/limiter.rs", 0, 6),
            "7b998ff499bd195a78fe7b99f0e697fa3c22b054_0_6"
        );
    }
}
