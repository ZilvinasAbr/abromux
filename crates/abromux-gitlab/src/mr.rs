//! A merge request's page as GitLab lays it out: title and status line, the
//! tab bar, and the Overview (description, approval and merge widgets, the
//! activity timeline, the sidebar) and Commits tabs.

#[allow(unused_imports)]
use crate::style::px;
use crate::{
    Section,
    diff::Kind,
    review::{File, Review},
    style::*,
    time,
    types::{AwardEmoji, Discussion, Note, User},
    view::GitLabView,
};
use gpui::{prelude::*, *};
use std::rc::Rc;

/// GitLab's activity filter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ActivityFilter {
    #[default]
    All,
    Comments,
    History,
}

impl ActivityFilter {
    pub fn label(self) -> &'static str {
        match self {
            ActivityFilter::All => "All activity",
            ActivityFilter::Comments => "Only comments",
            ActivityFilter::History => "Only history",
        }
    }
}

/// One entry of the activity timeline, by when it happened.
enum Entry<'a> {
    System(&'a Note),
    General(&'a Discussion),
    /// A thread on a line: its file and where in the hunks.
    OnLine(&'a File, usize, usize, &'a Discussion),
    /// On a line no longer in the diff.
    Outdated(&'a Discussion),
}

impl Entry<'_> {
    fn at(&self) -> &str {
        match self {
            Entry::System(n) => &n.created_at,
            Entry::General(d) | Entry::OnLine(_, _, _, d) | Entry::Outdated(d) => {
                &d.notes[0].created_at
            }
        }
    }
}

/// The extras of an "added N commits" note: the commits ("273815b8 -
/// Document LIMIT") and its "Compare with previous version" link's
/// (start_sha, diff_id).
fn pushed(body: &str) -> (Vec<String>, Option<(String, u64)>) {
    let mut commits = vec![];
    let mut rest = body;
    while let Some(at) = rest.find("<li>") {
        let after = &rest[at + 4..];
        let Some(end) = after.find("</li>") else {
            break;
        };
        commits.push(after[..end].trim().to_string());
        rest = &after[end + 5..];
    }
    let compare = body.split("](").nth(1).and_then(|link| {
        let link = link.split(')').next()?;
        let query = link.split_once('?')?.1;
        let param = |k: &str| {
            query
                .split('&')
                .find_map(|p| p.strip_prefix(k)?.strip_prefix('=').map(str::to_string))
        };
        Some((param("start_sha")?, param("diff_id")?.parse().ok()?))
    });
    (commits, compare)
}

impl GitLabView {
    /// GitLab's emoji row under the description: 👍 and 👎 always, then
    /// each other reaction, and the "Add reaction" picker.
    fn awards(&self, r: &Review, cx: &mut Context<Self>) -> Div {
        let open = r.mr.state != "locked";
        // A merge request fetched without the award list still has its votes.
        let votes = |name: &str| match name {
            "thumbsup" => r.mr.upvotes as usize,
            "thumbsdown" => r.mr.downvotes as usize,
            _ => 0,
        };
        let pills = self.award_pills(
            AwardOn::Mr,
            &r.awards,
            &["thumbsup", "thumbsdown"],
            &votes,
            open,
            cx,
        );
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(pills)
            .when(open, |d| d.child(self.add_reaction(AwardOn::Mr, cx)))
    }

    /// A comment's reactions, under its text; none, nothing.
    pub(crate) fn note_awards(&self, note: u64, cx: &mut Context<Self>) -> Option<Div> {
        let r = self.review.ready()?;
        let awards = r.note_awards.get(&note).filter(|a| !a.is_empty())?;
        let open = r.mr.state != "locked" && self.me.is_some();
        let pills = self.award_pills(AwardOn::Note(note), awards, &[], &|_| 0, open, cx);
        Some(div().flex().flex_wrap().gap_2().pt_1().children(pills))
    }

    /// A pill for each emoji in `awards` (after `always`, shown even at 0):
    /// the emoji and how many, yours highlighted, a click toggling yours.
    fn award_pills(
        &self,
        on: AwardOn,
        awards: &[AwardEmoji],
        always: &[&'static str],
        fallback: &dyn Fn(&str) -> usize,
        open: bool,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let me = self.me.as_ref().map(|u| u.id);
        let mut names: Vec<&str> = always.to_vec();
        for a in awards {
            if !names.contains(&a.name.as_str()) {
                names.push(&a.name);
            }
        }
        let pill = |name: &str, cx: &mut Context<Self>| {
            let who: Vec<&str> = awards
                .iter()
                .filter(|a| a.name == name)
                .map(|a| a.user.name.as_str())
                .collect();
            let n = if who.is_empty() {
                fallback(name)
            } else {
                who.len()
            };
            let mine = awards
                .iter()
                .any(|a| a.name == name && Some(a.user.id) == me);
            let tip = match who.len() {
                0 => String::new(),
                1 => format!("{} reacted with :{name}:", who[0]),
                _ => format!(
                    "{} and {} reacted with :{name}:",
                    who[..who.len() - 1].join(", "),
                    who[who.len() - 1]
                ),
            };
            let emoji = crate::emoji::of(name).map_or_else(|| format!(":{name}:"), str::to_string);
            let name = name.to_string();
            div()
                .id(SharedString::from(format!("gl-award-{}{name}", on.key())))
                .flex()
                .items_center()
                .gap_2()
                .h(px(if on == AwardOn::Mr { 32. } else { 28. }))
                .px_3()
                .rounded(px(8.))
                .border_1()
                .text_color(c(TEXT))
                .map(|d| {
                    if mine {
                        d.bg(ca(0x1068bf40)).border_color(ca(0x428fdcff))
                    } else {
                        d.bg(ca(0x89888d66)).border_color(ca(0x00000000))
                    }
                })
                .cursor_pointer()
                .hover(|s| s.bg(ca(0x89888d99)))
                .child(emoji)
                .child(n.to_string())
                .when(!tip.is_empty(), |d| {
                    d.tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                })
                .when(open, |d| {
                    d.on_click(
                        cx.listener(move |v, _, window, cx| v.toggle_award(on, &name, window, cx)),
                    )
                })
        };
        names.iter().map(|n| pill(n, cx)).collect()
    }

    /// GitLab's "Add reaction" button, and its picker while open. On a
    /// comment it sits at the header's right, so the picker opens leftward.
    pub(crate) fn add_reaction(&self, on: AwardOn, cx: &mut Context<Self>) -> Div {
        let note = on != AwardOn::Mr;
        let picker = (self.emoji_picker == Some(on)).then(|| {
            let choice = |(name, emoji): (&'static str, &'static str), cx: &mut Context<Self>| {
                div()
                    .id(SharedString::from(format!("gl-pick-{}{name}", on.key())))
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .text_size(px(18.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(RAISED)))
                    .child(emoji)
                    .on_click(
                        cx.listener(move |v, _, window, cx| v.toggle_award(on, name, window, cx)),
                    )
            };
            let choices: Vec<_> = crate::emoji::picks().map(|p| choice(p, cx)).collect();
            deferred(
                div()
                    .id(SharedString::from(format!("gl-emoji-picker{}", on.key())))
                    .absolute()
                    .top(px(if note { 32. } else { 36. }))
                    .map(|d| if note { d.right_0() } else { d.left_0() })
                    .w(px(8. * 32. + 16. + 2.))
                    .p_2()
                    .flex()
                    .flex_wrap()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(CARD))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                        v.emoji_picker = None;
                        cx.notify();
                    }))
                    .children(choices),
            )
            .with_priority(1)
        });
        let button = div()
            .id(SharedString::from(format!("gl-add-reaction{}", on.key())))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(if note { 6. } else { 8. }))
            .cursor_pointer()
            .hover(|s| s.bg(ca(0x89888d99)))
            .map(|d| {
                if note {
                    d.size(px(28.)).child(icon("smile-plus", 16., c(MUTED)))
                } else {
                    d.size(px(32.))
                        .bg(ca(0x89888d66))
                        .child(icon("smile-plus", 16., c(TEXT)))
                }
            })
            .tooltip(|window, cx| {
                gpui_component::tooltip::Tooltip::new("Add reaction").build(window, cx)
            })
            .on_click(cx.listener(move |v, _, _, cx| {
                v.emoji_picker = if v.emoji_picker == Some(on) {
                    None
                } else {
                    Some(on)
                };
                cx.notify();
            }));
        div().relative().child(button).children(picker)
    }
}

/// The width of the pipelines' Retry / Cancel column.
const PIPELINE_ACTIONS_W: f32 = 64.;

/// What a reaction is on: the merge request, or one of its comments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AwardOn {
    Mr,
    Note(u64),
}

impl AwardOn {
    /// Its part of an element id: nothing for the merge request's (so those
    /// stay as they were), `note-12-` for a comment's.
    fn key(self) -> String {
        match self {
            AwardOn::Mr => String::new(),
            AwardOn::Note(id) => format!("note-{id}-"),
        }
    }
}

/// What GitLab's merge widget says for a `detailed_merge_status`.
enum MergeState {
    Ready,
    Checking,
    /// The check that failed, as GitLab 19 lists it under "Merge blocked".
    Blocked(&'static str),
}

fn merge_status(status: &str) -> MergeState {
    use MergeState::*;
    match status {
        "mergeable" => Ready,
        "checking" | "unchecked" | "preparing" | "approvals_syncing" => Checking,
        "not_approved" => Blocked("All required approvals must be given."),
        "draft_status" => Blocked("Merge request must not be draft."),
        "discussions_not_resolved" => Blocked("All threads must be resolved."),
        "conflict" => Blocked("Merge conflicts must be resolved."),
        "need_rebase" => {
            Blocked("Merge request must be rebased, because a fast-forward merge is not possible.")
        }
        "ci_must_pass" => Blocked("Pipeline must succeed."),
        "ci_still_running" => {
            Blocked("Pipeline must succeed. It's waiting for a manual action or still running.")
        }
        "blocked_status" => Blocked("Merge request dependencies must be merged."),
        "not_open" => Blocked("Merge request must be open."),
        "jira_association_missing" => {
            Blocked("Either the title or description must reference a Jira issue.")
        }
        "requested_changes" => Blocked("The change requests must be completed or resolved."),
        _ => Blocked("Merge request can't be merged."),
    }
}

/// The GitLab user behind a commit: by the email's name part, as GitLab
/// matches commit emails to accounts; else just the commit's name.
pub(crate) fn commit_author(r: &Review, cm: &crate::types::Commit) -> User {
    let local = cm.author_email.split('@').next().unwrap_or_default();
    std::iter::once(&r.mr.author)
        .chain(&r.mr.assignees)
        .chain(&r.mr.reviewers)
        .chain(&r.members)
        .find(|u| u.username == local)
        .cloned()
        .unwrap_or_else(|| User {
            id: cm.author_email.len() as u64,
            name: cm.author_name.clone(),
            ..Default::default()
        })
}

/// A title without its draft marker, as "Mark as ready" leaves it.
#[cfg(test)]
fn ready_title(title: &str) -> String {
    crate::edit::draft_title(title, false)
}

impl GitLabView {
    pub(crate) fn mr_author_id(&self) -> u64 {
        self.review.ready().map_or(0, |r| r.mr.author.id)
    }

    /// A title with the Jira tickets it names as links, as GitLab's Jira
    /// integration shows them (when abromux knows the Jira: `jira.host`).
    pub(crate) fn linked_title(&self, id: &'static str, title: &str, cx: &App) -> AnyElement {
        let keys: Vec<(std::ops::Range<usize>, String)> = abromux_plugin::jira_keys(title)
            .into_iter()
            .filter_map(|r| Some((r.clone(), abromux_plugin::jira_url(&title[r], cx)?)))
            .collect();
        if keys.is_empty() {
            return SharedString::from(title.to_string()).into_any_element();
        }
        let link = HighlightStyle {
            color: Some(c(LINK)),
            ..Default::default()
        };
        let text = StyledText::new(title.to_string())
            .with_highlights(keys.iter().map(|(r, _)| (r.clone(), link)));
        let ranges: Vec<_> = keys.iter().map(|(r, _)| r.clone()).collect();
        InteractiveText::new(id, text)
            .on_click(ranges, move |i, window, cx| {
                if let Some((_, url)) = keys.get(i) {
                    abromux_plugin::open_url(url, window, cx);
                }
            })
            .into_any_element()
    }

    /// Title, status line and tabs.
    pub(crate) fn mr_header(&self, r: &Review, cx: &mut Context<Self>) -> Div {
        let m = &r.mr;
        let mono = self.mono(cx);
        let (state, bg, fg) = match m.state.as_str() {
            "merged" => ("Merged", 0x64a7e9, 0x1d2153),
            "closed" => ("Closed", 0xfc7988, 0x5a0e17),
            _ => ("Open", GREEN, 0x002c26),
        };
        let unresolved = r.unresolved();
        let local = self.is_local();
        // Built before the tabs, which hold `cx` till the end.
        let stack = if local {
            None
        } else {
            self.stack_button(r, mono.clone(), cx)
        };
        let actions = self.mr_actions(r, cx);
        let sections = [
            (
                Section::Overview,
                "Overview",
                r.mr.user_notes_count as usize,
            ),
            (Section::Commits, "Commits", r.commits.len()),
            (Section::Pipelines, "Pipelines", r.pipelines.len()),
            // What the Changes tab shows: a commit or a version's files, as GitLab counts them.
            (
                Section::Changes,
                "Changes",
                self.shown().map_or(r.files.len(), |s| s.files.len()),
            ),
        ];
        // A local diff has no pipelines.
        let sections = sections
            .into_iter()
            .filter(|(s, ..)| !(local && *s == Section::Pipelines));
        let tabs = sections.map(|(s, label, n)| {
            let on = self.loc.section == s;
            div()
                .id(SharedString::from(format!("gl-section-{label}")))
                .flex()
                .items_center()
                .gap_2()
                .h(px(48.))
                .px_3()
                .border_b_2()
                .border_color(if on { c(HEADING) } else { transparent_black() })
                .cursor_pointer()
                .text_color(c(TEXT))
                .when(!on, |d| d.hover(|s| s.border_color(c(BORDER))))
                .child(label)
                .child(count_badge(n))
                .on_click(cx.listener(move |v, _, _, cx| v.set_section(s, cx)))
        });
        // Scrolled down the overview or the changes, GitLab folds the header
        // into one line (state, title, branches) over the tabs.
        let top = match self.loc.section {
            Section::Changes => Some(self.diff_list.logical_scroll_top()),
            Section::Overview => Some(self.overview_list.logical_scroll_top()),
            Section::Commits | Section::Pipelines => None,
        };
        if top.is_some_and(|t| t.item_ix > 0 || t.offset_in_item > px(0.)) {
            return div()
                .flex()
                .flex_col()
                .px_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pt_2()
                        .text_color(c(MUTED))
                        .when(!local, |d| {
                            d.child(
                                badge(state, c(bg), c(fg))
                                    .child(icon("merge", 12., c(fg)))
                                    .flex_row_reverse()
                                    .flex_none(),
                            )
                        })
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(HEADING))
                                .child(self.linked_title("gl-title-folded", &m.title, cx)),
                        )
                        .child(branch(&m.source_branch, mono.clone()).flex_none())
                        .child(
                            div()
                                .flex_none()
                                .child(if local { "against" } else { "into" }),
                        )
                        .child(branch(&m.target_branch, mono).flex_none()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(c(RAISED))
                        .children(tabs)
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .children(self.thread_nav(r, unresolved, cx))
                                .children(self.review_button(r, cx)),
                        ),
                );
        }
        div()
            .flex()
            .flex_col()
            .px_4()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .pt_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(30.))
                            .line_height(px(37.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(HEADING))
                            .child(self.linked_title("gl-title", &m.title, cx)),
                    )
                    .children(actions.map(|a| a.mt_1())),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .pt_2()
                    .text_color(c(MUTED))
                    // A local diff: "<head> compared with <base>", no state or author.
                    .when(!local, |d| {
                        d.child(
                            badge(state, c(bg), c(fg))
                                .child(icon("merge", 12., c(fg)))
                                .flex_row_reverse(),
                        )
                        .child(
                            div()
                                .ml_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(TEXT))
                                .child(m.author.name.clone()),
                        )
                        .child("requested to merge")
                        .children(stack)
                    })
                    .child(branch(&m.source_branch, mono.clone()))
                    // GitLab's "Copy branch name".
                    .child({
                        let name = m.source_branch.clone();
                        div()
                            .id("gl-copy-branch")
                            .p(px(3.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.bg(ca(0x89888d66)))
                            .child(icon("copy", 14., c(MUTED)))
                            .tooltip(hint("Copy branch name"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(name.clone()))
                            })
                    })
                    .child(if local { "compared with" } else { "into" })
                    .child(branch(&m.target_branch, mono))
                    .when(!local, |d| d.child(when(&m.created_at))),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(c(RAISED))
                    .children(tabs)
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .children(self.thread_nav(r, unresolved, cx))
                            .children(self.review_button(r, cx)),
                    ),
            )
    }

    /// GitLab's "Stacked merge requests" button ("1 of 2") and, while
    /// open, its list of the stack, top first; nothing off a stack.
    fn stack_button(&self, r: &Review, mono: SharedString, cx: &mut Context<Self>) -> Option<Div> {
        let stack = &r.stack;
        let at = stack.iter().position(|m| m.iid == r.mr.iid)?;
        let project = r.mr.project_id;
        let menu = self.stack_menu.then(|| {
            let row = |m: &crate::types::StackedMr, cx: &mut Context<Self>| {
                let me = m.iid == r.mr.iid;
                let iid = m.iid;
                let state = match m.state.as_str() {
                    "merged" => "Merged",
                    "closed" => "Closed",
                    _ => "Open",
                };
                div()
                    .id(SharedString::from(format!("gl-stack-{iid}")))
                    .flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .when(!me, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(c(RAISED)))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.stack_menu = false;
                                v.open_mr(project, iid, cx);
                            }))
                    })
                    .child(
                        div()
                            .w(px(20.))
                            .pt(px(2.))
                            .flex_none()
                            .when(me, |d| d.child(icon("arrow-right", 16., c(TEXT)))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(div().text_color(c(TEXT)).child(m.title.clone()))
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .text_size(px(12.))
                                    .text_color(c(MUTED))
                                    .child(state)
                                    .child(when(&m.created_at)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .h(px(20.))
                            .text_color(c(MUTED))
                            .child(icon("file-diff", 14., c(MUTED)))
                            .child(m.files.to_string())
                            .child(
                                div()
                                    .ml_1()
                                    .text_color(c(GREEN))
                                    .child(format!("+{}", m.additions)),
                            )
                            .child(
                                div()
                                    .text_color(c(RED))
                                    .child(format!("\u{2212}{}", m.deletions)),
                            ),
                    )
            };
            let rows: Vec<_> = stack.iter().rev().map(|m| row(m, cx)).collect();
            let bottom = stack
                .first()
                .map(|m| m.target_branch.clone())
                .unwrap_or_default();
            deferred(
                div()
                    .id("gl-stack-menu")
                    .absolute()
                    .top(px(32.))
                    .left_0()
                    .w(px(480.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(BORDER))
                    .bg(c(CARD))
                    .shadow_lg()
                    .occlude()
                    .text_color(c(TEXT))
                    .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                        v.stack_menu = false;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(c(BORDER))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Stacked merge requests"),
                    )
                    .child(div().py_1().children(rows))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .px_3()
                            .py_2()
                            .border_t_1()
                            .border_color(c(BORDER))
                            .text_size(px(12.))
                            .text_color(c(MUTED))
                            .child("Bottom of stack merges into")
                            .child(
                                branch(&bottom, mono)
                                    .child(app_icon("git-branch", 12., c(0xc8e0f9)))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .flex_row_reverse(),
                            )
                            .child("first."),
                    ),
            )
            .with_priority(1)
        });
        let open = self.stack_menu;
        let button = div()
            .id("gl-stack-button")
            .flex()
            .items_center()
            .gap_1()
            .h(px(24.))
            .px_2()
            .rounded(px(6.))
            .border_1()
            .border_color(if open { c(FOCUS) } else { c(BORDER) })
            .cursor_pointer()
            .text_color(c(TEXT))
            .hover(|s| s.bg(ca(0x89888d66)))
            .child(icon("layers", 14., c(TEXT)))
            .child(format!("{} of {}", at + 1, stack.len()))
            .child(app_icon("chevron-down", 14., c(TEXT)))
            .tooltip(hint("Stacked merge requests"))
            .on_click(cx.listener(|v, _, _, cx| {
                v.stack_menu = !v.stack_menu;
                cx.notify();
            }));
        Some(div().relative().mx_1().child(button).children(menu))
    }

    /// GitLab's unresolved-threads counter with its up and down arrows;
    /// nothing when the merge request has no threads to resolve.
    fn thread_nav(&self, r: &Review, unresolved: usize, cx: &mut Context<Self>) -> Option<Div> {
        if !r.has_resolvable() {
            return None;
        }
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(32.))
                .px_3()
                .rounded(px(8.))
                .bg(c(RAISED))
                .text_color(c(TEXT))
                .when(unresolved > 0, |d| {
                    let arrow = |id: &'static str,
                                 name: &'static str,
                                 forward: bool,
                                 cx: &mut Context<Self>| {
                        div()
                            .id(id)
                            .size(px(24.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(ca(0x89888d66)))
                            .child(app_icon(name, 16., c(TEXT)))
                            .on_click(cx.listener(move |v, _, _, cx| v.jump_thread(forward, cx)))
                    };
                    d.child(icon("comment", 16., c(TEXT)))
                        .child(unresolved.to_string())
                        .child(arrow("gl-thread-prev", "chevron-up", false, cx))
                        .child(arrow("gl-thread-next", "chevron-down", true, cx))
                })
                .when(unresolved == 0, |d| {
                    d.child(icon("check-circle", 16., c(GREEN)))
                        .child("All threads resolved")
                }),
        )
    }

    pub(crate) fn overview_page(
        &self,
        r: &Rc<Review>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wide = self.sidebar_fits();
        let _ = window;
        // The list's length follows the activity's filter.
        // The top, the activity, the comment form.
        let n = 1 + self.activity_entries(r).len() + 1;
        if self.overview_list.item_count() != n {
            self.overview_list.reset(n);
        }
        let main = list(
            self.overview_list.clone(),
            cx.processor(|v, ix: usize, window, cx| v.overview_item(ix, window, cx)),
        )
        .flex_1()
        .min_w(px(0.))
        .h_full();
        div()
            .size_full()
            .flex()
            .child(main)
            // GitLab's sidebar stays put while the page scrolls.
            .when(wide, |d| {
                d.child(
                    div()
                        .id("gl-sidebar")
                        .h_full()
                        .flex_none()
                        .overflow_y_scroll()
                        .pt_4()
                        .pr_4()
                        .child(self.sidebar(r, cx)),
                )
            })
            .into_any_element()
    }

    /// The description; GitLab shows nothing at all for an empty one.
    fn description(&self, r: &Review, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        let d =
            r.mr.description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())?;
        Some(self.markdown(("description", r.mr.id as usize), d, &[], None, window, cx))
    }

    /// GitLab's pipeline widget: "Pipeline #5 pending" and what it ran on.
    /// `id` tells two apart: the source's and, merged, the target's.
    fn pipeline(
        &self,
        p: &crate::types::Pipeline,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        let (word, _) = p.label();
        let status_icon = |size: f32| pipeline_icon(p, size);
        let mono = self.mono(cx);
        let url = p.web_url.clone();
        let short = p.sha.get(..8).unwrap_or_default().to_string();
        {
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_3()
                .rounded(px(12.))
                .bg(c(RAISED))
                .text_color(c(TEXT))
                .child(status_icon(20.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Pipeline")
                                .child(
                                    div()
                                        .id(id)
                                        .text_color(c(LINK))
                                        .cursor_pointer()
                                        .hover(|s| s.underline())
                                        .child(format!("#{}", p.id))
                                        .on_click(move |_, window, cx| {
                                            abromux_plugin::open_url(&url, window, cx)
                                        }),
                                )
                                .child(word),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_1()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(c(MUTED))
                                .child(format!("Pipeline {word} for"))
                                .child(
                                    div()
                                        .font_family(mono.clone())
                                        .text_color(c(TEXT))
                                        .child(short),
                                )
                                .child("on")
                                .child(branch(&p.git_ref, mono)),
                        ),
                )
                .child(status_icon(20.))
        }
    }

    /// The approval and merge status cards.
    fn widgets(&self, r: &Review, mono: SharedString, cx: &mut Context<Self>) -> Div {
        let approvers: Vec<&User> = r.approvals.approved_by.iter().map(|a| &a.user).collect();
        let me = self.me.as_ref().map(|u| u.id);
        let approved_by_me = approvers.iter().any(|u| Some(u.id) == me);
        let card = || {
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_3()
                .rounded(px(12.))
                .bg(c(RAISED))
                .text_color(c(TEXT))
        };
        // GitLab's Approve / Revoke approval leads the card; only while open.
        let button = (r.mr.state == "opened").then(|| self.approve_button(approved_by_me, cx));
        let approval = if approvers.is_empty() {
            card()
                .children(button)
                .child(icon("check-circle", 16., c(MUTED)))
                .child(div().text_color(c(MUTED)).child("Approval is optional"))
        } else {
            card()
                .children(button)
                .child(icon("check-circle", 16., c(GREEN)))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if approved_by_me {
                            "Approved by you".to_string()
                        } else {
                            "Approved by".to_string()
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .children(approvers.iter().map(|u| self.avatars.of(u, 24.))),
                )
        };
        let status = merge_status(r.mr.detailed_merge_status.as_deref().unwrap_or(""));
        let n = r.commits.len();
        let line = || div().flex().flex_wrap().gap_x_1();
        let bold = |s: String| div().font_weight(FontWeight::BOLD).child(s);
        let link = |id: &'static str, s: &'static str| {
            div()
                .id(id)
                .text_color(c(LINK))
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(s)
        };
        let mut facts: Vec<Div> = vec![];
        // GitLab's "The source branch is 2 commits behind the target branch."
        if let Some(behind) = r.mr.diverged_commits_count.filter(|&b| b > 0) {
            let project = r.mr.web_url.split("/-/").next().unwrap_or_default();
            let url = format!("{project}/-/commits/{}", r.mr.target_branch);
            facts.push(
                line()
                    .child("The source branch is")
                    .child(
                        div()
                            .id("gl-behind")
                            .text_color(c(LINK))
                            .underline()
                            .cursor_pointer()
                            .child(format!(
                                "{behind} commit{} behind",
                                if behind == 1 { "" } else { "s" }
                            ))
                            .on_click(move |_, window, cx| {
                                abromux_plugin::open_url(&url, window, cx)
                            }),
                    )
                    .child("the target branch.")
                    .when(r.mr.state == "opened", |d| {
                        d.child(
                            link("gl-rebase", "Rebase source branch")
                                .on_click(cx.listener(|v, _, window, cx| v.rebase(window, cx))),
                        )
                    }),
            );
        }
        let squash = self.merge_squash(r);
        let n = if squash { n.min(1) } else { n };
        facts.push(
            line()
                .child(bold(n.to_string()))
                .child(format!("commit{} and", if n == 1 { "" } else { "s" }))
                .child(bold("1".into()))
                .child("merge commit will be added to")
                .child(
                    div()
                        .flex()
                        .child(
                            div()
                                .font_family(mono.clone())
                                .child(r.mr.target_branch.clone()),
                        )
                        .child("."),
                ),
        );
        if self.merge_remove(r) {
            facts.push(line().child("Source branch will be deleted."));
        }
        // Merging: who may, and whether it waits for a pipeline still going.
        let can_merge = self.me.is_some()
            && r.mr.state == "opened"
            && r.mr.user.as_ref().is_some_and(|u| u.can_merge);
        let running =
            r.mr.head_pipeline
                .as_ref()
                .is_some_and(|p| crate::edit::pipeline_active(&p.status));
        let waits = matches!(
            r.mr.detailed_merge_status.as_deref(),
            Some("ci_still_running" | "ci_must_pass")
        );
        let auto_set = r.mr.merge_when_pipeline_succeeds;
        let controls = (can_merge
            && !auto_set
            && (matches!(status, MergeState::Ready) || (running && waits)))
            .then(|| {
                let remove = self.merge_remove(r);
                let auto = running;
                let (label, live) = crate::edit::busy_label(
                    self.busy,
                    crate::edit::Write::Merge,
                    if auto { "Set to auto-merge" } else { "Merge" },
                    if auto { "Setting…" } else { "Merging…" },
                );
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_4()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(c(BG))
                    .child(
                        crate::edit::primary("gl-merge", label, live).when(live, |d| {
                            d.on_click(
                                cx.listener(move |v, _, window, cx| v.merge(auto, window, cx)),
                            )
                        }),
                    )
                    .child(
                        crate::edit::checkbox("gl-merge-remove", remove, "Delete source branch")
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.merge_ticks.1 = Some(!remove);
                                cx.notify();
                            })),
                    )
                    .when(!r.mr.squash_on_merge, |d| {
                        d.child(
                            crate::edit::checkbox("gl-merge-squash", squash, "Squash commits")
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.merge_ticks.0 = Some(!squash);
                                    cx.notify();
                                })),
                        )
                    })
            });
        // Set to auto-merge: who did, and GitLab's Cancel auto-merge.
        let auto_line = (auto_set && r.mr.state == "opened").then(|| {
            let who =
                r.mr.merge_user
                    .as_ref()
                    .map_or_else(|| "someone".to_string(), |u| u.name.clone());
            let (label, live) = crate::edit::busy_label(
                self.busy,
                crate::edit::Write::Merge,
                "Cancel auto-merge",
                "Canceling…",
            );
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
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!(
                            "Set by {who} to be merged automatically when all merge checks pass"
                        )),
                )
                .when(self.me.is_some(), |d| {
                    d.child(
                        crate::edit::secondary("gl-cancel-auto-merge", label).when(live, |d| {
                            d.on_click(
                                cx.listener(|v, _, window, cx| v.cancel_auto_merge(window, cx)),
                            )
                        }),
                    )
                })
        });
        let merge = div()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(c(RAISED))
            .text_color(c(TEXT))
            .children(auto_line)
            .when(!auto_set, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .child(match status {
                            MergeState::Ready => icon("check-circle-filled", 18., c(GREEN)),
                            MergeState::Checking => icon("loader", 18., c(MUTED)),
                            MergeState::Blocked(_) => icon("minus-circle-filled", 18., c(RED)),
                        })
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(match status {
                            MergeState::Ready if can_merge => "Ready to merge!",
                            MergeState::Ready => {
                                "Ready to merge by members who can write to the target branch."
                            }
                            MergeState::Checking => "Checking if merge request can be merged…",
                            MergeState::Blocked(_) => "Merge blocked: 1 check failed",
                        })),
                )
            })
            .children(controls)
            // The failed check, with GitLab's fix for a draft.
            .when_some(
                match status {
                    MergeState::Blocked(check) => Some(check),
                    _ => None,
                },
                |d, check| {
                    let draft = r.mr.detailed_merge_status.as_deref() == Some("draft_status")
                        && self.me.is_some();
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .pl(px(46.))
                            .pr_4()
                            .py_2()
                            .border_t_1()
                            .border_color(c(BG))
                            .child(icon("minus-circle-filled", 12., c(RED)))
                            .child(div().flex_1().child(check))
                            .when(draft, |d| {
                                d.child(
                                    crate::edit::secondary("gl-mark-ready", "Mark as ready")
                                        .on_click(cx.listener(|v, _, window, cx| {
                                            v.set_draft(false, window, cx)
                                        })),
                                )
                            }),
                    )
                },
            )
            .when(!matches!(status, MergeState::Checking), |d| {
                let blocked = matches!(status, MergeState::Blocked(_));
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .pl(px(if blocked { 46. } else { 16. }))
                        .pr_4()
                        .py_3()
                        .border_t_1()
                        .border_color(c(BG))
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .gap_0p5()
                        // Blocked, GitLab heads the facts "Merge details".
                        .when(blocked, |d| {
                            d.child(
                                div()
                                    .pb_1()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Merge details"),
                            )
                        })
                        .children(
                            facts
                                .into_iter()
                                .map(|f| div().flex().gap_2().child("•").child(f.flex_1())),
                        ),
                )
            });
        let merge = match r.mr.state.as_str() {
            "merged" | "closed" => self.outcome(r, mono),
            _ => merge,
        };
        div().flex().flex_col().gap_3().child(approval).child(merge)
    }

    /// The widget of a merge request that's done: who merged or closed it,
    /// when, and what became of the changes.
    fn outcome(&self, r: &Review, mono: SharedString) -> Div {
        let m = &r.mr;
        let merged = m.state == "merged";
        let (who, when) = if merged {
            (&m.merged_by, &m.merged_at)
        } else {
            (&m.closed_by, &m.closed_at)
        };
        let mut facts: Vec<Div> = vec![];
        let code = |s: &str| div().font_family(mono.clone()).child(s.to_string());
        if merged {
            let sha = m.squash_commit_sha.as_ref().or(m.merge_commit_sha.as_ref());
            facts.push(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child("Changes merged into")
                    .child(code(&m.target_branch))
                    .when_some(sha, |d, s| {
                        d.child("with").child(
                            div()
                                .flex()
                                .child(code(s.get(..8).unwrap_or(s)).text_color(c(LINK)))
                                .child("."),
                        )
                    })
                    .when(sha.is_none(), |d| d.child(".")),
            );
            if m.force_remove_source_branch || m.should_remove_source_branch.unwrap_or(false) {
                facts.push(div().child("Deleted the source branch."));
            }
        } else {
            facts.push(
                div()
                    .flex()
                    .gap_1()
                    .child("The changes were not merged into")
                    .child(div().flex().child(code(&m.target_branch)).child(".")),
            );
        }
        div()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .overflow_hidden()
            .bg(c(RAISED))
            .text_color(c(TEXT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .bg(c(if merged { 0x22306a } else { 0x641c28 }))
                    .child(icon("merge", 16., c(TEXT)))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(if merged {
                        "Merged by"
                    } else {
                        "Closed by"
                    }))
                    .when_some(who.as_ref(), |d, u| {
                        d.child(self.avatars.of(u, 16.)).child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(LINK))
                                .child(u.name.clone()),
                        )
                    })
                    .when_some(when.as_ref(), |d, t| d.child(crate::style::when(t))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pl(px(48.))
                    .pr_4()
                    .py_3()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Merge details"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .children(
                                facts
                                    .into_iter()
                                    .map(|f| div().flex().gap_2().child("•").child(f)),
                            ),
                    ),
            )
    }

    /// The activity's entries, filtered and in order.
    fn activity_entries<'a>(&self, r: &'a Review) -> Vec<Entry<'a>> {
        let mut entries: Vec<Entry> = r.system.iter().map(Entry::System).collect();
        entries.extend(r.general.iter().map(Entry::General));
        for f in &r.files {
            entries.extend(
                f.threads
                    .iter()
                    .map(|(h, l, d)| Entry::OnLine(f, *h, *l, d)),
            );
            entries.extend(f.outdated.iter().map(Entry::Outdated));
        }
        entries.extend(r.outdated.iter().map(Entry::Outdated));
        // GitLab's filter: comments only drops its own notes, history keeps only those.
        entries.retain(|e| match self.activity {
            ActivityFilter::All => true,
            ActivityFilter::Comments => !matches!(e, Entry::System(_)),
            ActivityFilter::History => matches!(e, Entry::System(_)),
        });
        entries.sort_by(|a, b| a.at().cmp(b.at()));
        if self.newest_first {
            entries.reverse();
        }
        entries
    }

    /// "Activity", with GitLab's filter menu and sort button.
    fn activity_header(&self, cx: &mut Context<Self>) -> Div {
        let open = self.activity_menu;
        let menu = open.then(|| {
            let option = |f: ActivityFilter, cx: &mut Context<Self>| {
                div()
                    .id(f.label())
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(c(RAISED)))
                    .child(div().w(px(14.)).when(self.activity == f, |d| {
                        d.child(app_icon("check", 14., c(TEXT)))
                    }))
                    .child(div().text_color(c(TEXT)).child(f.label()))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.activity = f;
                        v.activity_menu = false;
                        cx.notify();
                    }))
            };
            deferred(
                div()
                    .id("gl-activity-menu")
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
                        v.activity_menu = false;
                        cx.notify();
                    }))
                    .child(option(ActivityFilter::All, cx))
                    .child(option(ActivityFilter::Comments, cx))
                    .child(option(ActivityFilter::History, cx)),
            )
            .with_priority(1)
        });
        let control = |id: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .h(px(32.))
                .px_3()
                .bg(ca(0x89888d66))
                .cursor_pointer()
                .hover(|s| s.bg(ca(0x89888d99)))
                .text_color(c(TEXT))
        };
        let controls = div()
            .relative()
            .flex()
            .gap(px(1.))
            .child(
                control("gl-activity-filter")
                    .rounded_l(px(8.))
                    .child(self.activity.label())
                    .child(app_icon("chevron-down", 14., c(TEXT)))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.activity_menu = !v.activity_menu;
                        cx.notify();
                    })),
            )
            .child(
                control("gl-activity-sort")
                    .rounded_r(px(8.))
                    // GitLab's sort icon: oldest first reads upward.
                    .child(icon(
                        if self.newest_first {
                            "sort-desc"
                        } else {
                            "sort-asc"
                        },
                        16.,
                        c(TEXT),
                    ))
                    .tooltip(hint(if self.newest_first {
                        "Sort direction: Descending"
                    } else {
                        "Sort direction: Ascending"
                    }))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.newest_first = !v.newest_first;
                        cx.notify();
                    })),
            )
            .children(menu);
        div()
            .flex()
            .items_center()
            .pt_2()
            .child(
                div()
                    .flex_1()
                    .text_size(px(25.))
                    .line_height(px(31.25))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(HEADING))
                    .child("Activity"),
            )
            .child(controls)
    }

    /// One row of the overview's list: 0 is everything above the activity
    /// (description, widgets, the Activity heading), then one per entry.
    /// Only the rows on screen are laid out, as in the Changes tab.
    fn overview_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(r) = self.review.ready().cloned() else {
            return div().into_any_element();
        };
        let wide = self.sidebar_fits();
        let pad = |d: Div| d.w_full().pl_4().pr(px(if wide { 24. } else { 16. }));
        if ix == 0 {
            return pad(div())
                .pt_4()
                .pb_4()
                .flex()
                .flex_col()
                .gap_4()
                .children(self.write_alert(cx))
                .when(self.editing_mr, |d| d.child(self.mr_edit_form(cx)))
                .when(!self.editing_mr, |d| {
                    d.children(self.description(&r, window, cx))
                })
                // Reactions, pipelines, approving and merging are GitLab's;
                // a local diff has none of them.
                .when(!self.is_local(), |d| {
                    d.child(self.awards(&r, cx))
                        .children(
                            r.mr.head_pipeline
                                .as_ref()
                                .map(|p| self.pipeline(p, "gl-pipeline-link", cx)),
                        )
                        .child(self.widgets(&r, self.mono(cx), cx))
                })
                // Merged, GitLab follows with the merge commit's pipeline on the target.
                .children(
                    r.merge_pipeline
                        .as_ref()
                        .map(|p| self.pipeline(p, "gl-merge-pipeline-link", cx)),
                )
                .child(self.activity_header(cx))
                .into_any_element();
        }
        let entries = self.activity_entries(&r);
        // After the activity: GitLab's comment form.
        if ix == entries.len() + 1 {
            return pad(div())
                .pt_2()
                .pb_6()
                .child(self.comment_form(cx))
                .into_any_element();
        }
        let Some(e) = entries.get(ix - 1) else {
            return div().into_any_element();
        };
        pad(div())
            .child(
                div()
                    .relative()
                    .pb_4()
                    // The timeline's line, under the avatars.
                    .child(
                        div()
                            .absolute()
                            .left(px(15.))
                            .top_0()
                            .bottom_0()
                            .w(px(2.))
                            .bg(c(RAISED)),
                    )
                    .child(self.entry(e, window, cx)),
            )
            .into_any_element()
    }

    fn entry(&self, e: &Entry, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match e {
            Entry::System(n) => self.system_note(n, cx).into_any_element(),
            Entry::General(d) => {
                let n = &d.notes[0];
                div()
                    .flex()
                    .gap_3()
                    .child(self.avatars.of(&n.author, 32.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(c(BORDER))
                            .bg(c(CARD))
                            .flex()
                            .flex_col()
                            .children(self.thread_notes(d, None, false, true, window, cx))
                            // A thread (not a lone comment) takes replies.
                            .when(!d.individual_note, |el| {
                                el.child(self.reply_bar(&d.id, d.resolvable(), d.resolved(), cx))
                            }),
                    )
                    .into_any_element()
            }
            Entry::OnLine(f, h, l, d) => self.line_thread(Some((f, *h, *l)), d, window, cx),
            Entry::Outdated(d) => self.line_thread(None, d, window, cx),
        }
    }

    /// "Alice Author requested review from @bob 1 hour ago"
    fn system_note(&self, n: &Note, cx: &mut Context<Self>) -> Div {
        let me = self.me.as_ref().map(|u| u.username.as_str());
        let mut body = div().flex().flex_wrap().items_center().gap_1();
        let text = n.body.lines().next().unwrap_or_default();
        for word in text.split(' ').filter(|w| !w.is_empty()) {
            let name = word.trim_end_matches([',', '.']);
            body = if name.starts_with('@') && name.len() > 1 {
                body.child(mention(name, Some(&name[1..]) == me))
            } else if let Some(label) = word.strip_prefix('~').filter(|l| !l.is_empty()) {
                // A label, in its colours (the project's, by name).
                let name = crate::review::untoken(label);
                let full = self
                    .review
                    .ready()
                    .and_then(|r| r.labels.iter().find(|l| l.name == name).cloned());
                body.child(label_pill(&full.unwrap_or(crate::types::Label {
                    name,
                    ..Default::default()
                })))
            } else if let Some(ms) = word.strip_prefix('%').filter(|m| !m.is_empty()) {
                body.child(
                    div()
                        .text_color(c(LINK))
                        .child(format!("%{}", crate::review::untoken(ms))),
                )
            } else {
                // A commit's full sha, shortened as GitLab shows it.
                let short = (word.len() == 40 && word.bytes().all(|b| b.is_ascii_hexdigit()))
                    .then(|| &word[..8]);
                // GitLab's own markdown in its notes: "marked … as **ready**".
                let strong = word.strip_prefix("**").and_then(|w| {
                    let (inner, rest) = w.split_once("**")?;
                    Some((inner.to_string(), rest.to_string()))
                });
                match (short, strong) {
                    (Some(s), _) => body.child(
                        div()
                            .font_family(self.mono(cx))
                            .text_color(c(LINK))
                            .child(s.to_string()),
                    ),
                    (None, Some((inner, rest))) => body.child(
                        div()
                            .flex()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c(TEXT))
                                    .child(inner),
                            )
                            .when(!rest.is_empty(), |d| {
                                d.child(div().text_color(c(MUTED)).child(rest))
                            }),
                    ),
                    (None, None) => body.child(div().text_color(c(MUTED)).child(word.to_string())),
                }
            };
        }
        // GitLab marks an approval with a green check, merging and closing
        // with the merge icon in blue or red; other notes with a dot.
        let circle = |bg: u32| {
            div()
                .size(px(20.))
                .rounded_full()
                .bg(c(bg))
                .flex()
                .items_center()
                .justify_center()
        };
        let marker = if text.starts_with("approved this merge request") {
            circle(0x0d532a).child(app_icon("check", 12., c(0x91d4a8)))
        } else if text == "merged" || text.starts_with("merged ") {
            circle(0x22306a).child(icon("merge", 12., c(0xc8e0f9)))
        } else if text == "closed" || text.starts_with("closed ") {
            circle(0x641c28).child(icon("merge", 12., c(0xfdd4cd)))
        } else {
            div()
                .size(px(8.))
                .rounded_full()
                .bg(c(0x89888d))
                .border_2()
                .border_color(c(BG))
        };
        let line = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(32.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(marker),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(TEXT))
                            .child(n.author.name.clone()),
                    )
                    .child(body)
                    .child(when(&n.created_at).text_color(c(MUTED))),
            );
        // "added 1 commit": the commits, and the link to what they changed.
        let (commits, compare) = pushed(&n.body);
        if commits.is_empty() {
            return line;
        }
        let mono = self.mono(cx);
        let versions: Vec<(u64, String)> = self
            .review
            .ready()
            .map(|r| {
                r.versions
                    .iter()
                    .map(|v| (v.id, v.head_commit_sha.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let pick = compare.and_then(|(start, diff_id)| {
            // The previous version, by its head commit; this note's version, by id.
            let base = versions
                .iter()
                .find(|(_, sha)| sha.starts_with(&start) || start.starts_with(sha.as_str()))?
                .0;
            let head = versions.iter().find(|(id, _)| *id == diff_id)?.0;
            Some(crate::Compare {
                base: Some(base),
                head: Some(head),
                commit: None,
            })
        });
        div().flex().flex_col().gap_1().child(line).child(
            div()
                .pl(px(44.))
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(12.))
                .line_height(px(18.))
                .children(commits.into_iter().map(|cm| {
                    let (sha, title) = cm.split_once(" - ").unwrap_or((&cm, ""));
                    // A link to the commit's changes, while it's still in the merge request.
                    let index = self.review.ready().and_then(|r| {
                        r.commits
                            .iter()
                            .position(|x| !sha.is_empty() && x.id.starts_with(sha))
                    });
                    let sha_el = div()
                        .id(SharedString::from(format!("note-commit-{}-{sha}", n.id)))
                        .font_family(mono.clone())
                        .text_color(c(if index.is_some() { LINK } else { TEXT }))
                        .child(sha.to_string())
                        .when_some(index, |d, i| {
                            d.cursor_pointer()
                                .hover(|s| s.underline())
                                .on_click(cx.listener(move |v, _, _, cx| v.open_commit(i, cx)))
                        });
                    div()
                        .flex()
                        .gap_2()
                        .text_color(c(MUTED))
                        .child("•")
                        .child(sha_el)
                        .child("-")
                        .child(title.to_string())
                }))
                .when_some(pick, |d, pick| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("compare-{}", n.id)))
                            .cursor_pointer()
                            .text_color(c(LINK))
                            .hover(|s| s.underline())
                            .child("Compare with previous version")
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.loc.section = Section::Changes;
                                v.set_compare(pick, cx);
                            })),
                    )
                }),
        )
    }

    /// A thread on the diff, as the overview shows it: who started it, the
    /// file and the code above the line, then the notes. Resolved ones fold.
    fn line_thread(
        &self,
        at: Option<(&File, usize, usize)>,
        d: &Discussion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let first = &d.notes[0];
        let resolved = d.resolved();
        let open = if resolved {
            self.open_resolved.contains(&d.id)
        } else {
            !self.collapsed_replies.contains(&d.id)
        };
        let mono = self.mono(cx);
        let id = d.id.clone();
        let header = div()
            .px_4()
            .py_3()
            .bg(c(CARD))
            .rounded_t(px(8.))
            .when(!open, |e| e.rounded_b(px(8.)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .text_color(c(MUTED))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(TEXT))
                            .child(first.author.name.clone()),
                    )
                    .child(format!("@{}", first.author.username))
                    .child("started a thread on")
                    .child(match at {
                        // A link to the thread on its line, as in GitLab.
                        Some(_) => {
                            let id = d.id.clone();
                            div()
                                .id(SharedString::from(format!("to-diff-{}", d.id)))
                                .text_color(c(TEXT))
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child("the diff")
                                .on_click(cx.listener(move |v, _, _, cx| v.show_thread(&id, cx)))
                                .into_any_element()
                        }
                        None => div()
                            .text_color(c(TEXT))
                            .child("an old version of the diff")
                            .into_any_element(),
                    })
                    .child(when(&first.created_at)),
            )
            .when_some(d.resolution().filter(|_| resolved), |e, (who, when)| {
                e.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(c(MUTED))
                        .child(format!(
                            "Resolved {} by {}",
                            time::ago(when, time::now()),
                            who.name
                        )),
                )
            });
        let path = at
            .map(|(f, _, _)| f.meta.new_path.clone())
            .or_else(|| d.position().and_then(|p| p.new_path.clone()))
            .unwrap_or_default();
        let toggle = div()
            .id(SharedString::from(format!("ov-toggle-{id}")))
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(c(RAISED))
            .bg(c(CARD))
            .when(!open, |e| e.rounded_b(px(8.)))
            .cursor_pointer()
            .text_color(c(LINK))
            .child(app_icon(
                if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                14.,
                c(LINK),
            ))
            .when(open, |e| {
                e.child(if resolved {
                    "Hide thread"
                } else {
                    "Collapse replies"
                })
            })
            .when(!open, |e| {
                let last = d.notes.last().unwrap();
                let n = d.notes.len();
                e.child(self.avatars.of(&last.author, 20.))
                    .child(format!("{n} repl{}", if n == 1 { "y" } else { "ies" }))
                    .child(div().text_color(c(MUTED)).child(format!(
                        "Last reply by {} {}",
                        last.author.name,
                        time::ago(&last.created_at, time::now())
                    )))
            })
            .on_click(cx.listener(move |v, _, _, cx| {
                // Resolved threads open on request; the rest fold on request.
                let set = if resolved {
                    &mut v.open_resolved
                } else {
                    &mut v.collapsed_replies
                };
                if !set.remove(&id) {
                    set.insert(id.clone());
                }
                cx.notify();
            }));
        let mut card = div()
            .flex_1()
            .min_w(px(0.))
            .rounded(px(8.))
            .border_1()
            .border_color(c(BORDER))
            .flex()
            .flex_col()
            .child(header)
            .child(toggle);
        if open {
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py(px(10.))
                    .bg(c(CARD))
                    .border_t_1()
                    .border_b_1()
                    .border_color(c(BORDER))
                    .child({
                        let id = d.id.clone();
                        div()
                            .id(SharedString::from(format!("to-file-{}", d.id)))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(TEXT))
                            .when(at.is_some(), |el| {
                                el.cursor_pointer().hover(|s| s.underline()).on_click(
                                    cx.listener(move |v, _, _, cx| v.show_thread(&id, cx)),
                                )
                            })
                            .child(path.clone())
                    })
                    .child(icon("copy", 14., c(MUTED)))
                    .when_some(at.map(|(f, _, _)| f), |e, f| {
                        e.when(
                            f.meta.a_mode != f.meta.b_mode && !f.meta.a_mode.is_empty(),
                            |e| {
                                e.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(c(TEXT))
                                        .child(format!("{} → {}", f.meta.a_mode, f.meta.b_mode)),
                                )
                            },
                        )
                    }),
            );
            if let Some((f, h, l)) = at {
                // Up to 16 lines of the hunk, ending at the thread's line.
                let lines = &f.body().hunks()[h].lines;
                let start = (l + 1).saturating_sub(16);
                let group = crate::select::Group::block(("snippet", &d.id));
                let rows: Vec<Div> = lines[start..=l]
                    .iter()
                    .filter(|x| x.kind != Kind::NoNewline)
                    .enumerate()
                    .map(|(i, x)| {
                        let (t, h) = crate::discussion::line_highlights(x);
                        let text = self.sel_text(t, h, group, i, cx);
                        crate::discussion::code_line_with(x, &mono, None, false, text)
                    })
                    .collect();
                card = card.children(rows);
            }
            card = card.child(
                div()
                    .bg(c(CARD))
                    .rounded_b(px(8.))
                    .border_t_1()
                    .border_color(c(BORDER))
                    .flex()
                    .flex_col()
                    .children(self.thread_notes(d, Some(&path), true, false, window, cx))
                    .child(
                        self.reply_bar(&d.id, d.resolvable(), d.resolved(), cx)
                            .rounded_b(px(8.)),
                    ),
            );
        }
        div()
            .flex()
            .gap_3()
            .child(self.avatars.of(&first.author, 32.))
            .child(card)
            .into_any_element()
    }

    /// GitLab's Pipelines tab: each of the merge request's pipelines, newest
    /// first, with its status, number, the commit it ran on and how it
    /// started.
    /// GitLab's "Run pipeline" over a merge request's pipelines.
    fn run_pipeline_button(&self, r: &Review, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        (self.me.is_some() && r.mr.state == "opened").then(|| {
            let (label, live) = crate::edit::busy_label(
                self.busy,
                crate::edit::Write::Pipeline,
                "Run pipeline",
                "Starting…",
            );
            crate::edit::secondary("gl-run-pipeline", label).when(live, |d| {
                d.on_click(
                    cx.listener(|v, _, window, cx| v.pipeline_action(None, true, window, cx)),
                )
            })
        })
    }

    /// A pipeline's Retry (failed or canceled) or Cancel (still going).
    fn pipeline_buttons(&self, p: &crate::types::Pipeline, cx: &mut Context<Self>) -> Div {
        let id = p.id;
        let active = crate::edit::pipeline_active(&p.status);
        let retry = matches!(p.status.as_str(), "failed" | "canceled");
        div()
            .w(px(PIPELINE_ACTIONS_W))
            .flex_none()
            .flex()
            .justify_end()
            .when(
                self.me.is_some() && self.busy != Some(crate::edit::Write::Pipeline),
                |d| {
                    d.when(retry, |d| {
                        d.child(
                            crate::edit::icon_button(
                                SharedString::from(format!("pipeline-retry-{id}")),
                                app_icon("rotate-cw", 16., c(TEXT)),
                                "Retry",
                            )
                            .on_click(cx.listener(
                                move |v, _, window, cx| {
                                    v.pipeline_action(Some(id), true, window, cx)
                                },
                            )),
                        )
                    })
                    .when(active, |d| {
                        d.child(
                            crate::edit::icon_button(
                                SharedString::from(format!("pipeline-cancel-{id}")),
                                app_icon("x", 16., c(RED)),
                                "Cancel pipeline",
                            )
                            .on_click(cx.listener(
                                move |v, _, window, cx| {
                                    v.pipeline_action(Some(id), false, window, cx)
                                },
                            )),
                        )
                    })
                },
            )
    }

    pub(crate) fn pipelines_page(&self, r: &Review, cx: &mut Context<Self>) -> AnyElement {
        let mono = self.mono(cx);
        if r.pipelines.is_empty() {
            return div()
                .px_4()
                .py_6()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .text_color(c(MUTED))
                .child("There are currently no pipelines.")
                .children(self.run_pipeline_button(r, cx))
                .into_any_element();
        }
        let head = |text: &'static str, w: Option<f32>| {
            div()
                .when_some(w, |d, w| d.w(px(w)).flex_none())
                .when(w.is_none(), |d| d.flex_1())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(TEXT))
                .child(text)
        };
        let chip = |text: String, mono: SharedString| {
            div()
                .px_1()
                .rounded(px(4.))
                .bg(c(RAISED))
                .font_family(mono)
                .text_size(px(12.))
                .text_color(c(TEXT))
                .child(text)
        };
        let run = self.run_pipeline_button(r, cx);
        let rows = r.pipelines.iter().enumerate().map(|(i, p)| {
            use crate::types::PipelineTone::*;
            let (word, tone) = p.label();
            let (bg, fg) = match tone {
                Good => (0x0d532a, 0x91d4a8),
                Bad => (0x8d1300, 0xfdd4cd),
                Busy => (0x064787, 0xcbe2f9),
                Waiting => (0x5c3900, 0xf5d9a8),
                Neutral => (0x4c4b51, 0xececef),
            };
            let label = {
                let mut w = word.to_string();
                if let Some(f) = w.get_mut(..1) {
                    f.make_ascii_uppercase();
                }
                w
            };
            let title = r
                .commits
                .iter()
                .find(|cm| cm.id == p.sha)
                .map(|cm| cm.title.clone());
            let url = p.web_url.clone();
            let title_url = p.web_url.clone();
            let source = match p.source.as_str() {
                "merge_request_event" => "merge request",
                "push" => "branch",
                "" => "",
                other => other,
            };
            div()
                .flex()
                .items_start()
                .gap_4()
                .py_3()
                .border_b_1()
                .border_color(c(RAISED))
                .child(
                    div()
                        .w(px(180.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div().flex().child(
                                badge(label, c(bg), c(fg))
                                    .child(pipeline_icon(p, 12.))
                                    .flex_row_reverse(),
                            ),
                        )
                        .child(when(&p.created_at).text_size(px(12.)).text_color(c(MUTED))),
                )
                // GitLab's: the number, the commit, where it ran, and its tags.
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .id(SharedString::from(format!("pipeline-{}", p.id)))
                                .flex()
                                .text_color(c(LINK))
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child(format!("#{}", p.id))
                                .on_click(move |_, window, cx| {
                                    abromux_plugin::open_url(&url, window, cx)
                                }),
                        )
                        .when_some(title, |d, t| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("pipeline-title-{}", p.id)))
                                    .truncate()
                                    .text_color(c(LINK))
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .child(t)
                                    .on_click(move |_, window, cx| {
                                        abromux_plugin::open_url(&title_url, window, cx)
                                    }),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_1()
                                .child(chip(p.git_ref.clone(), mono.clone()))
                                .child(chip(
                                    p.sha.get(..8).unwrap_or_default().to_string(),
                                    mono.clone(),
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .when(i == 0, |d| {
                                    d.child(badge("latest", c(0x0d532a), c(0x91d4a8)))
                                })
                                .when(!source.is_empty(), |d| {
                                    d.child(badge(source.to_string(), c(0x064787), c(0xcbe2f9)))
                                }),
                        ),
                )
                .child(div().w(px(120.)).flex_none().child(pipeline_icon(p, 20.)))
                .child(self.pipeline_buttons(p, cx))
        });
        div()
            .id("gl-pipelines")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .children(run.map(|b| div().flex().justify_end().pt_3().pb_1().child(b)))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .py_2()
                    .border_b_1()
                    .border_color(c(RAISED))
                    .child(head("Status", Some(180.)))
                    .child(head("Pipeline", None))
                    .child(head("Stages", Some(120.)))
                    .child(div().w(px(PIPELINE_ACTIONS_W)).flex_none()),
            )
            .children(rows)
            .into_any_element()
    }

    pub(crate) fn commits_page(&self, r: &Review, cx: &mut Context<Self>) -> AnyElement {
        let mono = self.mono(cx);
        let mut days: Vec<(String, Vec<&crate::types::Commit>)> = vec![];
        for cm in &r.commits {
            let date = if cm.authored_date.is_empty() {
                &cm.created_at
            } else {
                &cm.authored_date
            };
            let day = time::day(date);
            match days.last_mut() {
                Some((d, list)) if *d == day => list.push(cm),
                _ => days.push((day, vec![cm])),
            }
        }
        let user_of = |cm: &crate::types::Commit| commit_author(r, cm);
        let groups = days.into_iter().map(|(day, list)| {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .py_2()
                        .border_b_1()
                        .border_color(c(RAISED))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(TEXT))
                        .child(day),
                )
                .children(list.into_iter().map(|cm| {
                    let author = user_of(cm);
                    let sha = cm.id.clone();
                    let index = r.commits.iter().position(|x| x.id == cm.id).unwrap_or(0);
                    let when = if cm.authored_date.is_empty() {
                        &cm.created_at
                    } else {
                        &cm.authored_date
                    };
                    let group = ca(0x89888d66);
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .py_3()
                        .border_b_1()
                        .border_color(c(RAISED))
                        .child(self.avatars.of(&author, 32.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .min_w(px(0.))
                                        .child(
                                            // Its changes, in the Changes tab.
                                            div()
                                                .id(SharedString::from(format!("commit-{}", cm.id)))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(c(TEXT))
                                                .truncate()
                                                .cursor_pointer()
                                                .hover(|s| s.underline())
                                                .child(cm.title.clone())
                                                .on_click(cx.listener(move |v, _, _, cx| {
                                                    v.open_commit(index, cx)
                                                })),
                                        )
                                        // GitLab's ⋯ for the rest of the message.
                                        .when(cm.body().is_some(), |d| {
                                            let id = cm.id.clone();
                                            d.child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "body-{}",
                                                        cm.id
                                                    )))
                                                    .flex_none()
                                                    .px_1()
                                                    .rounded(px(4.))
                                                    .bg(ca(0x89888d66))
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(ca(0x89888d99)))
                                                    .text_size(px(12.))
                                                    .line_height(px(14.))
                                                    .text_color(c(TEXT))
                                                    .child("…")
                                                    .on_click(cx.listener(move |v, _, _, cx| {
                                                        if !v.open_bodies.remove(&id) {
                                                            v.open_bodies.insert(id.clone());
                                                        }
                                                        cx.notify();
                                                    })),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .child(div().text_color(c(TEXT)).child(author.name.clone()))
                                        .child(div().text_color(c(MUTED)).child(format!(
                                            "authored {}",
                                            time::ago(when, time::now())
                                        ))),
                                )
                                .when_some(
                                    cm.body().filter(|_| self.open_bodies.contains(&cm.id)),
                                    |d, body| {
                                        d.child(
                                            div()
                                                .mt_2()
                                                .font_family(mono.clone())
                                                .text_size(px(12.))
                                                .line_height(px(18.))
                                                .text_color(c(TEXT))
                                                .whitespace_normal()
                                                .children(body.lines().map(|l| {
                                                    div().child(if l.is_empty() {
                                                        " ".to_string()
                                                    } else {
                                                        l.to_string()
                                                    })
                                                })),
                                        )
                                    },
                                ),
                        )
                        // The commit's latest pipeline, as GitLab shows it
                        // left of the SHA; a click opens it.
                        .when_some(r.pipelines.iter().find(|p| p.sha == cm.id), |d, p| {
                            let url = p.web_url.clone();
                            let tip = format!("Pipeline: {}", p.label().0);
                            d.child(
                                div()
                                    .id(SharedString::from(format!("commit-pipeline-{}", cm.id)))
                                    .flex()
                                    .items_center()
                                    .h(px(32.))
                                    .px_2()
                                    .cursor_pointer()
                                    .child(pipeline_icon(p, 20.))
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new(tip.clone())
                                            .build(window, cx)
                                    })
                                    .on_click(move |_, window, cx| {
                                        abromux_plugin::open_url(&url, window, cx)
                                    }),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .h(px(32.))
                                        .px_3()
                                        .rounded_l(px(8.))
                                        .bg(group)
                                        .font_family(mono.clone())
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(c(TEXT))
                                        .child(cm.short_id.clone()),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("copy-sha-{}", cm.id)))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(32.))
                                        .rounded_r(px(8.))
                                        .bg(group)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(ca(0x89888d99)))
                                        .child(icon("copy", 16., c(TEXT)))
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                sha.clone(),
                                            ))
                                        }),
                                ),
                        )
                }))
        });
        div()
            .id("gl-commits")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .px_4()
                    .py_2()
                    .children(groups),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod ready_tests {
    use super::ready_title;

    #[test]
    fn mark_as_ready_drops_the_draft_marker() {
        assert_eq!(ready_title("Draft: big refactor"), "big refactor");
        assert_eq!(ready_title("[Draft] big refactor"), "big refactor");
        assert_eq!(ready_title("draft: lower case"), "lower case");
        assert_eq!(ready_title("Drafting notes"), "Drafting notes");
        assert_eq!(ready_title("Not a draft"), "Not a draft");
    }
}

#[cfg(test)]
mod tests {
    use super::pushed;

    #[test]
    fn reads_added_commits_notes() {
        let body = "added 1 commit\n\n<ul><li>273815b8 - Document LIMIT</li></ul>\n\n\
                    [Compare with previous version](/a/b/-/merge_requests/1/diffs?diff_id=18&start_sha=f4f3c8)";
        let (commits, compare) = pushed(body);
        assert_eq!(commits, ["273815b8 - Document LIMIT"]);
        assert_eq!(compare, Some(("f4f3c8".to_string(), 18)));
        assert_eq!(pushed("assigned to @alice"), (vec![], None));
    }
}
