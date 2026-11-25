//! GitLab's merge checks, the list its merge widget opens to ("Merge
//! request must be open.", "Merge conflicts must be resolved." …), each
//! passed, failed or a warning.

#[allow(unused_imports)]
use crate::style::px;
use crate::{review::Review, style::*, view::GitLabView};
use gpui::{prelude::*, *};

/// GitLab's words for a check, by its identifier.
pub fn check_text(id: &str) -> Option<&'static str> {
    Some(match id {
        "NOT_OPEN" => "Merge request must be open.",
        "DRAFT_STATUS" => "Merge request must not be draft.",
        "COMMITS_STATUS" => "Source branch exists and contains commits.",
        "CONFLICT" => "Merge conflicts must be resolved.",
        "CI_MUST_PASS" => "Pipeline must succeed.",
        "DISCUSSIONS_NOT_RESOLVED" => "Unresolved discussions must be resolved.",
        "NEED_REBASE" => {
            "Merge request must be rebased, because a fast-forward merge is not possible."
        }
        "NOT_APPROVED" => "All required approvals must be given.",
        "REQUESTED_CHANGES" => "Change requests must be approved by the requesting user.",
        "MERGE_REQUEST_BLOCKED" => "Merge request dependencies must be merged.",
        "STATUS_CHECKS_MUST_PASS" => "Status checks must pass.",
        "JIRA_ASSOCIATION_MISSING" => {
            "Either the title or description must reference a Jira issue."
        }
        "SECURITY_POLICY_VIOLATIONS" => "All policy rules must be satisfied.",
        "MERGE_TIME" => "Cannot merge until the merge after time.",
        "LOCKED_PATHS" => "All paths must be unlocked.",
        "LOCKED_LFS_FILES" => "All LFS files must be unlocked.",
        _ => return None,
    })
}

impl GitLabView {
    /// The checks GitLab ran (not those that don't apply), worst first.
    pub(crate) fn merge_checks_list(&self, r: &Review) -> Option<Div> {
        if !self.more.checks_open {
            return None;
        }
        let mut shown: Vec<&(String, String)> = r
            .checks
            .iter()
            .filter(|(id, st)| st != "INACTIVE" && !(id == "LOCKED_LFS_FILES" && st == "SUCCESS"))
            .collect();
        shown.sort_by_key(|(_, st)| match st.as_str() {
            "FAILED" => 0,
            "WARNING" => 1,
            _ => 2,
        });
        Some(
            div()
                .flex()
                .flex_col()
                .children(shown.into_iter().map(|(id, st)| {
                    let (name, color) = match st.as_str() {
                        "SUCCESS" => ("check-circle-filled", GREEN),
                        "FAILED" => ("minus-circle-filled", RED),
                        _ => ("status-pending", ORANGE),
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pl(px(46.))
                        .pr_4()
                        .py_2()
                        .border_t_1()
                        .border_color(c(BG))
                        .child(icon(name, 14., c(color)))
                        .child(
                            div().child(
                                check_text(id)
                                    .map(str::to_string)
                                    .unwrap_or_else(|| id.to_lowercase().replace('_', " ")),
                            ),
                        )
                })),
        )
    }

    /// The widget's chevron: GitLab's "Expand merge checks".
    pub(crate) fn checks_toggle(
        &self,
        r: &Review,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if r.checks.is_empty() {
            return None;
        }
        let open = self.more.checks_open;
        Some(
            crate::edit::icon_button(
                "gl-merge-checks",
                app_icon(
                    if open { "chevron-up" } else { "chevron-down" },
                    16.,
                    c(TEXT),
                ),
                if open {
                    "Collapse merge checks"
                } else {
                    "Expand merge checks"
                },
            )
            .on_click(cx.listener(move |v, _, _, cx| {
                v.more.checks_open = !open;
                v.rebuild_rows(cx);
                cx.notify();
            })),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn words_for_checks() {
        assert_eq!(
            super::check_text("CONFLICT"),
            Some("Merge conflicts must be resolved.")
        );
        assert_eq!(super::check_text("SOMETHING_NEW"), None);
    }
}
