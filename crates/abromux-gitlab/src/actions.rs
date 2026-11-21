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

    pub fn fields(&self) -> [&Entity<InputState>; 4] {
        [
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
