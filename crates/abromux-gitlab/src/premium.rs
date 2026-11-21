//! What a paid GitLab (Premium, Ultimate) adds to a merge request's page,
//! drawn when the server has it: approval rules in the approval widget,
//! merge trains in the merge widget ("Add to merge train", the train's
//! state, "Remove from merge train"), and dependencies ("Depends on 1 merge
//! request being merged", added and removed there). A free GitLab answers
//! 404 for all three, and nothing extra shows.

#[allow(unused_imports)]
use crate::style::px;
use crate::{review::Review, style::*, types::*, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputState};

/// GitLab's words for the approvals the rules still need.
pub fn rules_line(s: &ApprovalState) -> Option<String> {
    let left: Vec<&ApprovalRule> = s
        .rules
        .iter()
        .filter(|r| !r.approved && r.approvals_required > 0)
        .collect();
    if s.rules.is_empty() {
        return None;
    }
    if left.is_empty() {
        return Some("All required approvals given.".into());
    }
    let need: u32 = left
        .iter()
        .map(|r| {
            r.approvals_required
                .saturating_sub(r.approved_by.len() as u32)
        })
        .sum();
    let names: Vec<&str> = left.iter().map(|r| r.name.as_str()).collect();
    Some(format!(
        "Requires {need} approval{} from {}.",
        if need == 1 { "" } else { "s" },
        names.join(", ")
    ))
}

/// GitLab's words for a train entry's state.
pub fn train_line(t: &TrainEntry, target: &str) -> String {
    let who = t
        .user
        .as_ref()
        .map_or("someone".to_string(), |u| u.name.clone());
    match t.status.as_str() {
        "merging" => format!("Merging into {target} from the merge train."),
        "merged" => format!("Merged into {target} by the merge train."),
        "skip_merged" => "Merged, skipping the merge train.".into(),
        _ => {
            format!("Added to the merge train by {who}. The changes will be merged into {target}.")
        }
    }
}

impl GitLabView {
    /// The approval rules, under the approval widget's line.
    pub(crate) fn approval_rules(&self, r: &Review) -> Option<Div> {
        let s = r.premium.rules.as_ref().filter(|s| !s.rules.is_empty())?;
        let line = rules_line(s)?;
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .px_4()
                .pb_3()
                .child(div().text_color(c(MUTED)).child(line))
                .children(s.rules.iter().map(|rule| {
                    let got = rule.approved_by.len() as u32;
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .py_1()
                        .border_t_1()
                        .border_color(c(BG))
                        .child(icon(
                            if rule.approved {
                                "check-circle-filled"
                            } else {
                                "circle-dashed"
                            },
                            14.,
                            c(if rule.approved { GREEN } else { MUTED }),
                        ))
                        .child(div().flex_1().text_color(c(TEXT)).child(
                            if rule.rule_type == "any_approver" {
                                "All eligible users".to_string()
                            } else {
                                rule.name.clone()
                            },
                        ))
                        .child(
                            div()
                                .text_color(c(MUTED))
                                .child(format!("{got} of {}", rule.approvals_required)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .children(rule.approved_by.iter().map(|u| self.avatars.of(u, 20.))),
                        )
                })),
        )
    }

    /// The merge train's line in the merge widget, with "Remove from merge
    /// train" (GitLab's cancel auto-merge).
    pub(crate) fn train_status(&self, r: &Review, cx: &mut Context<Self>) -> Option<Div> {
        let t = r
            .premium
            .train
            .as_ref()
            .filter(|t| matches!(t.status.as_str(), "idle" | "fresh" | "stale" | "merging"))?;
        Some(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_3()
                .child(icon("status-pending", 18., c(LINK)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(train_line(t, &r.mr.target_branch)),
                )
                .when(self.me.is_some() && t.status != "merging", |d| {
                    d.child(
                        crate::edit::secondary("gl-train-remove", "Remove from merge train")
                            .flex_none()
                            .on_click(
                                cx.listener(|v, _, window, cx| v.cancel_auto_merge(window, cx)),
                            ),
                    )
                }),
        )
    }

    /// "Depends on N merge requests being merged", each with its state and
    /// a ×, and a field to add one (`!12`).
    pub(crate) fn dependencies(&self, r: &Review, cx: &mut Context<Self>) -> Option<Div> {
        let p = &r.premium;
        // Only where the server has dependencies: it answered for blocks.
        if p.blocks.is_empty() && p.rules.is_none() {
            return None;
        }
        let open: Vec<&Block> = p
            .blocks
            .iter()
            .filter(|b| {
                b.blocking_merge_request
                    .as_ref()
                    .is_none_or(|m| m.state != "merged")
            })
            .collect();
        let title = match (p.blocks.len(), open.len()) {
            (0, _) => "No dependencies.".to_string(),
            (_, 0) => "All dependencies are merged.".to_string(),
            (_, n) => format!(
                "Depends on {n} merge request{} being merged.",
                if n == 1 { "" } else { "s" }
            ),
        };
        let can_edit = self.me.is_some() && r.mr.state == "opened";
        Some(
            div()
                .flex()
                .flex_col()
                .rounded(px(12.))
                .bg(c(RAISED))
                .text_color(c(TEXT))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .child(icon(
                            if open.is_empty() {
                                "check-circle-filled"
                            } else {
                                "minus-circle-filled"
                            },
                            18.,
                            c(if open.is_empty() { GREEN } else { ORANGE }),
                        ))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(title)),
                )
                .children(p.blocks.iter().map(|b| {
                    let id = b.id;
                    let (label, url, state) = match &b.blocking_merge_request {
                        Some(m) => (
                            format!("{} {}", m.references.full, m.title),
                            m.web_url.clone(),
                            m.state.clone(),
                        ),
                        None => (
                            "A merge request you can't see".to_string(),
                            String::new(),
                            String::new(),
                        ),
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_4()
                        .py_2()
                        .border_t_1()
                        .border_color(c(BG))
                        .child(
                            div()
                                .id(SharedString::from(format!("gl-dep-{id}")))
                                .flex_1()
                                .truncate()
                                .text_color(c(LINK))
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child(label)
                                .on_click(move |_, window, cx| {
                                    if !url.is_empty() {
                                        abromux_plugin::open_url(&url, window, cx)
                                    }
                                }),
                        )
                        .child(div().text_size(px(12.)).text_color(c(MUTED)).child(state))
                        .when(can_edit, |d| {
                            d.child(
                                crate::edit::icon_button(
                                    SharedString::from(format!("gl-dep-remove-{id}")),
                                    app_icon("close", 12., c(MUTED)),
                                    "Remove dependency",
                                )
                                .on_click(cx.listener(
                                    move |v, _, window, cx| v.remove_dependency(id, window, cx),
                                )),
                            )
                        })
                }))
                .when(can_edit, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_4()
                            .py_2()
                            .border_t_1()
                            .border_color(c(BG))
                            .child(
                                div()
                                    .flex_1()
                                    .p_1()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(c(BORDER))
                                    .bg(c(BG))
                                    .child(Input::new(&self.more.dependency).appearance(false)),
                            )
                            .child(
                                crate::edit::secondary("gl-dep-add", "Add dependency").on_click(
                                    cx.listener(|v, _, window, cx| v.add_dependency(window, cx)),
                                ),
                            ),
                    )
                }),
        )
    }

    /// Make it depend on the `!12` typed.
    pub(crate) fn add_dependency(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self
            .more
            .dependency
            .read(cx)
            .value()
            .trim()
            .trim_start_matches('!')
            .to_string();
        let Ok(blocking) = typed.parse::<u64>() else {
            self.write_error = Some("Type the merge request's number, like !12".into());
            cx.notify();
            return;
        };
        self.more
            .dependency
            .update(cx, |b, cx| b.set_value("", window, cx));
        self.write(
            crate::edit::Write::Sidebar,
            None,
            move |c, p, iid| c.add_block(p, iid, blocking),
            |_, _, _| {},
            window,
            cx,
        );
    }

    pub(crate) fn remove_dependency(
        &mut self,
        block: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.write(
            crate::edit::Write::Sidebar,
            None,
            move |c, p, iid| c.remove_block(p, iid, block),
            |_, _, _| {},
            window,
            cx,
        );
    }
}

/// The dependency field, made with the tab.
pub fn dependency_field(window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder("!12"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shaped as GitLab's API docs show them (Premium; a free GitLab
    // answers 404, which the client takes as "none").
    #[test]
    fn reads_premium_answers() {
        let s: ApprovalState = serde_json::from_value(serde_json::json!({
            "approval_rules_overwritten": true,
            "rules": [{
                "id": 1, "name": "Ruby", "rule_type": "regular",
                "eligible_approvers": [{ "id": 5, "name": "Group Member 1", "username": "group_member_1" }],
                "approvals_required": 2,
                "users": [], "groups": [], "contains_hidden_groups": false,
                "approved_by": [{ "id": 5, "name": "Group Member 1", "username": "group_member_1" }],
                "source_rule": null, "approved": false, "overridden": false
            }]
        }))
        .unwrap();
        assert_eq!(rules_line(&s).unwrap(), "Requires 1 approval from Ruby.");
        let t: TrainEntry = serde_json::from_value(serde_json::json!({
            "id": 267,
            "merge_request": { "id": 273, "iid": 1, "project_id": 597 },
            "user": { "id": 1, "name": "Administrator", "username": "root" },
            "pipeline": { "id": 1, "sha": "abc", "ref": "refs/merge-requests/1/train", "status": "pending" },
            "created_at": "2022-10-31T19:06:05.725Z",
            "target_branch": "main", "status": "idle", "merged_at": null, "duration": null
        }))
        .unwrap();
        assert_eq!(
            train_line(&t, "main"),
            "Added to the merge train by Administrator. The changes will be merged into main."
        );
        let b: Vec<Block> = serde_json::from_value(serde_json::json!([{
            "id": 1,
            "blocking_merge_request": { "id": 145, "iid": 12, "project_id": 7, "title": "Interesting MR", "state": "opened",
                "references": { "short": "!12", "relative": "!12", "full": "group/project!12" } },
            "blocked_merge_request": { "id": 146, "iid": 13 },
            "project_id": 7
        }]))
        .unwrap();
        assert_eq!(b[0].blocking_merge_request.as_ref().unwrap().iid, 12);
    }
}
