//! The merge request list's "Bulk edit" and Actions, as GitLab's list has
//! them: tick merge requests, then close or reopen them, add or remove a
//! label, set or clear the milestone, assign yourself, or subscribe and
//! unsubscribe, all at once; "Export as CSV" (written here from the list,
//! with GitLab's columns, where GitLab mails it) and "Subscribe to RSS
//! feed".

#[allow(unused_imports)]
use crate::style::px;
use crate::{style::*, types::MergeRequest, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputState};
use serde_json::json;
use std::collections::HashSet;

pub struct BulkState {
    /// Bulk edit is on, and the merge requests ticked: (project, iid).
    pub on: bool,
    pub picked: HashSet<(u64, u64)>,
    /// The label or milestone typed for the action.
    pub value: Entity<InputState>,
    pub busy: bool,
    pub said: Option<String>,
    pub actions_open: bool,
    /// Recent searches (GitLab's "Toggle search history"), newest first,
    /// and whether their list is open.
    pub history: Vec<String>,
    pub history_open: bool,
}

impl BulkState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        BulkState {
            on: false,
            picked: HashSet::new(),
            value: cx.new(|cx| InputState::new(window, cx).placeholder("Label or milestone")),
            busy: false,
            said: None,
            actions_open: false,
            history: vec![],
            history_open: false,
        }
    }
}

/// What to do to every merge request ticked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bulk {
    Close,
    Reopen,
    AddLabel,
    RemoveLabel,
    Milestone,
    NoMilestone,
    AssignMe,
    Subscribe,
    Unsubscribe,
}

/// The list as GitLab's "Export as CSV" writes it.
pub fn csv(mrs: &[&MergeRequest]) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let names = |us: &[crate::types::User]| {
        us.iter()
            .map(|u| u.username.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = String::from(
        "Title,Description,MR IID,URL,State,Source Branch,Target Branch,Source Project ID,Target Project ID,Author,Author Username,Assignees,Assignee Usernames,Reviewers,Approved,Labels,Milestone,Created At (UTC),Updated At (UTC),Merged At (UTC)\n",
    );
    for m in mrs {
        let row = [
            q(&m.title),
            q(m.description.as_deref().unwrap_or("")),
            m.iid.to_string(),
            q(&m.web_url),
            q(&m.state),
            q(&m.source_branch),
            q(&m.target_branch),
            m.project_id.to_string(),
            m.project_id.to_string(),
            q(&m.author.name),
            q(&m.author.username),
            q(&m.assignees
                .iter()
                .map(|u| u.name.clone())
                .collect::<Vec<_>>()
                .join(", ")),
            q(&names(&m.assignees)),
            q(&names(&m.reviewers)),
            q(""),
            q(&m.labels
                .iter()
                .map(|l| l.name.clone())
                .collect::<Vec<_>>()
                .join(",")),
            q(m.milestone.as_ref().map_or("", |x| x.title.as_str())),
            q(&m.created_at),
            q(&m.updated_at),
            q(m.merged_at.as_deref().unwrap_or("")),
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

impl GitLabView {
    /// Do `what` to every ticked merge request, then load the list again.
    pub(crate) fn run_bulk(&mut self, what: Bulk, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        let picked: Vec<(u64, u64)> = self.bulk.picked.iter().copied().collect();
        if picked.is_empty() {
            return;
        }
        let value = self
            .bulk
            .value
            .read(cx)
            .value()
            .trim()
            .trim_start_matches(['~', '%'])
            .to_string();
        let me = self.me.as_ref().map_or(0, |u| u.id);
        self.bulk.busy = true;
        self.bulk.said = None;
        cx.spawn_in(window, async move |this, cx| {
            let n = picked.len();
            let got = cx
                .background_spawn(async move {
                    let mut failed = vec![];
                    for (p, iid) in picked {
                        let r = match what {
                            Bulk::Close => c
                                .update(p, iid, json!({ "state_event": "close" }))
                                .map(|_| ()),
                            Bulk::Reopen => c
                                .update(p, iid, json!({ "state_event": "reopen" }))
                                .map(|_| ()),
                            Bulk::AddLabel => {
                                c.update(p, iid, json!({ "add_labels": value })).map(|_| ())
                            }
                            Bulk::RemoveLabel => c
                                .update(p, iid, json!({ "remove_labels": value }))
                                .map(|_| ()),
                            Bulk::Milestone => {
                                c.milestones(p)
                                    .and_then(|ms| {
                                        let m = ms.iter().find(|m| m.title == value).ok_or_else(
                                            || anyhow::anyhow!("no open milestone {value:?}"),
                                        )?;
                                        c.update(p, iid, json!({ "milestone_id": m.id }))
                                    })
                                    .map(|_| ())
                            }
                            Bulk::NoMilestone => {
                                c.update(p, iid, json!({ "milestone_id": 0 })).map(|_| ())
                            }
                            Bulk::AssignMe => c
                                .update(p, iid, json!({ "assignee_ids": [me] }))
                                .map(|_| ()),
                            Bulk::Subscribe => c.subscribe(p, iid, true),
                            Bulk::Unsubscribe => c.subscribe(p, iid, false),
                        };
                        if let Err(e) = r {
                            failed.push(format!("!{iid}: {e:#}"));
                        }
                    }
                    failed
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.bulk.busy = false;
                v.bulk.said = Some(if got.is_empty() {
                    format!(
                        "Updated {n} merge request{}.",
                        if n == 1 { "" } else { "s" }
                    )
                } else {
                    got.join("; ")
                });
                v.bulk.picked.clear();
                v.refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Export as CSV": the list shown, to a file picked.
    pub(crate) fn export_csv(
        &mut self,
        mrs: Vec<MergeRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = csv(&mrs.iter().collect::<Vec<_>>());
        let dir = dirs_home();
        let picked = cx.prompt_for_new_path(&dir, Some("merge_requests.csv"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let r = std::fs::write(&path, text);
            let _ = this.update(cx, |v, cx| {
                v.bulk.said = Some(match r {
                    Ok(()) => format!("Exported to {}", path.display()),
                    Err(e) => format!("Couldn't write {}: {e}", path.display()),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// The list's Bulk edit button and Actions menu, by the sort.
    pub(crate) fn list_tools(
        &self,
        project_url: Option<String>,
        mrs: Vec<MergeRequest>,
        cx: &mut Context<Self>,
    ) -> Div {
        let on = self.bulk.on;
        let open = self.bulk.actions_open;
        let rss = project_url.map(|u| format!("{u}/-/merge_requests.atom"));
        div()
            .flex()
            .items_center()
            .gap_2()
            .when(self.me.is_some(), |d| {
                d.child(
                    crate::edit::secondary("gl-bulk-edit", if on { "Cancel" } else { "Bulk edit" })
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.bulk.on = !on;
                            v.bulk.picked.clear();
                            v.bulk.said = None;
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .relative()
                    .child(
                        crate::edit::secondary("gl-list-actions", "")
                            .px_2()
                            .child(icon("ellipsis-v", 16., c(TEXT)))
                            .tooltip(hint("Actions"))
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.bulk.actions_open = !v.bulk.actions_open;
                                cx.notify();
                            })),
                    )
                    .when(open, |d| {
                        let item = |id: &'static str, label: &'static str| {
                            div()
                                .id(id)
                                .px_3()
                                .py_1()
                                .cursor_pointer()
                                .hover(|s| s.bg(c(RAISED)))
                                .text_color(c(TEXT))
                                .child(label)
                        };
                        d.child(
                            deferred(
                                div()
                                    .id("gl-list-actions-menu")
                                    .absolute()
                                    .top(px(36.))
                                    .right_0()
                                    .min_w(px(200.))
                                    .py_1()
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(c(BORDER))
                                    .bg(c(CARD))
                                    .shadow_lg()
                                    .occlude()
                                    .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                                        v.bulk.actions_open = false;
                                        cx.notify();
                                    }))
                                    .child(item("gl-export-csv", "Export as CSV").on_click(
                                        cx.listener(move |v, _, window, cx| {
                                            v.bulk.actions_open = false;
                                            v.export_csv(mrs.clone(), window, cx);
                                        }),
                                    ))
                                    .children(rss.map(|u| {
                                        item("gl-rss", "Subscribe to RSS feed").on_click(
                                            cx.listener(move |v, _, window, cx| {
                                                v.bulk.actions_open = false;
                                                abromux_plugin::open_url(&u, window, cx);
                                            }),
                                        )
                                    })),
                            )
                            .with_priority(2),
                        )
                    }),
            )
    }

    /// GitLab's "Toggle search history": the recent searches, one a click.
    pub(crate) fn search_history(&self, cx: &mut Context<Self>) -> Div {
        let open = self.bulk.history_open;
        let list = self.bulk.history.clone();
        div()
            .relative()
            .child(
                crate::edit::icon_button(
                    "gl-search-history",
                    icon("clock", 15., c(MUTED)),
                    "Toggle search history",
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.bulk.history_open = !v.bulk.history_open;
                    cx.notify();
                })),
            )
            .when(open, |d| {
                d.child(
                    deferred(
                        div()
                            .id("gl-search-history-menu")
                            .absolute()
                            .top(px(32.))
                            .left_0()
                            .w(px(360.))
                            .py_1()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(c(BORDER))
                            .bg(c(CARD))
                            .shadow_lg()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                                v.bulk.history_open = false;
                                cx.notify();
                            }))
                            .when(list.is_empty(), |d| {
                                d.child(
                                    div()
                                        .px_3()
                                        .py_1()
                                        .text_color(c(MUTED))
                                        .child("You don't have any recent searches"),
                                )
                            })
                            .children(list.into_iter().enumerate().map(|(i, q)| {
                                let text = q.clone();
                                div()
                                    .id(("gl-recent", i))
                                    .px_3()
                                    .py_1()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(c(RAISED)))
                                    .text_color(c(TEXT))
                                    .truncate()
                                    .child(q)
                                    .on_click(cx.listener(move |v, _, window, cx| {
                                        v.bulk.history_open = false;
                                        let t = text.clone();
                                        v.list_search
                                            .update(cx, |s, cx| s.set_value(t, window, cx));
                                        cx.notify();
                                    }))
                            })),
                    )
                    .with_priority(2),
                )
            })
    }

    /// Bulk edit's bar over the list: how many are ticked and what to do.
    pub(crate) fn bulk_bar(&self, all: Vec<(u64, u64)>, cx: &mut Context<Self>) -> Option<Div> {
        if !self.bulk.on {
            return None;
        }
        let n = self.bulk.picked.len();
        let live = n > 0 && !self.bulk.busy;
        let act = |id: &'static str, label: &'static str, what: Bulk| {
            crate::edit::secondary(id, label)
                .h(px(28.))
                .when(!live, |d| d.opacity(0.5))
                .when(live, |d| {
                    d.on_click(cx.listener(move |v, _, window, cx| v.run_bulk(what, window, cx)))
                })
        };
        let every = n == all.len() && n > 0;
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(c(RAISED))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            crate::edit::checkbox("gl-bulk-all", every, "Select all").on_click(
                                cx.listener(move |v, _, _, cx| {
                                    v.bulk.picked = if every {
                                        HashSet::new()
                                    } else {
                                        all.iter().copied().collect()
                                    };
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(div().text_color(c(MUTED)).child(format!("{n} selected")))
                        .child(act("gl-bulk-close", "Close", Bulk::Close))
                        .child(act("gl-bulk-reopen", "Reopen", Bulk::Reopen))
                        .child(act("gl-bulk-assign-me", "Assign to me", Bulk::AssignMe))
                        .child(act("gl-bulk-subscribe", "Subscribe", Bulk::Subscribe))
                        .child(act("gl-bulk-unsubscribe", "Unsubscribe", Bulk::Unsubscribe))
                        .child(act(
                            "gl-bulk-no-milestone",
                            "Remove milestone",
                            Bulk::NoMilestone,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .w(px(220.))
                                .p_1()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(c(BORDER))
                                .bg(c(BG))
                                .child(Input::new(&self.bulk.value).appearance(false)),
                        )
                        .child(act("gl-bulk-add-label", "Add label", Bulk::AddLabel))
                        .child(act(
                            "gl-bulk-remove-label",
                            "Remove label",
                            Bulk::RemoveLabel,
                        ))
                        .child(act("gl-bulk-milestone", "Set milestone", Bulk::Milestone)),
                )
                .children(
                    self.bulk
                        .said
                        .clone()
                        .map(|s| div().text_size(px(12.)).text_color(c(MUTED)).child(s)),
                ),
        )
    }
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var_os("HOME").map_or_else(|| std::path::PathBuf::from("/"), std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    #[test]
    fn csv_quotes_and_has_gitlabs_columns() {
        let m = crate::types::MergeRequest {
            iid: 3,
            title: "Say \"hi\"".into(),
            state: "opened".into(),
            ..Default::default()
        };
        let out = super::csv(&[&m]);
        assert!(out.starts_with("Title,Description,MR IID,URL,State"));
        assert!(
            out.lines()
                .nth(1)
                .unwrap()
                .starts_with("\"Say \"\"hi\"\"\",\"\",3,")
        );
    }
}
