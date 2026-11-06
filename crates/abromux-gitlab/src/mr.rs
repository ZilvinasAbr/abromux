//! A merge request's page as GitLab lays it out: title and status line, the
//! tab bar, and the Overview (description, approval and merge widgets, the
//! activity timeline, the sidebar) and Commits tabs.

use crate::{
    Section,
    diff::Kind,
    discussion::code_line,
    review::{File, Review},
    style::*,
    time,
    types::{Discussion, Note, User},
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
            Entry::General(d) | Entry::OnLine(_, _, _, d) | Entry::Outdated(d) => &d.notes[0].created_at,
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
        let Some(end) = after.find("</li>") else { break };
        commits.push(after[..end].trim().to_string());
        rest = &after[end + 5..];
    }
    let compare = body.split("](").nth(1).and_then(|link| {
        let link = link.split(')').next()?;
        let query = link.split_once('?')?.1;
        let param = |k: &str| query.split('&').find_map(|p| p.strip_prefix(k)?.strip_prefix('=').map(str::to_string));
        Some((param("start_sha")?, param("diff_id")?.parse().ok()?))
    });
    (commits, compare)
}

/// GitLab's emoji row under the description: 👍 and 👎 with their counts
/// (no "add reaction" button yet).
fn awards(up: u32, down: u32) -> Div {
    let pill = |emoji: &'static str, n: u32| {
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(32.))
            .px_3()
            .rounded(px(8.))
            .bg(ca(0x89888d66))
            .text_color(c(TEXT))
            .child(emoji)
            .child(n.to_string())
    };
    div().flex().gap_2().child(pill("👍", up)).child(pill("👎", down))
}

/// What GitLab's merge widget says for a `detailed_merge_status`.
fn merge_status(status: &str) -> (&'static str, bool) {
    match status {
        "mergeable" => ("Ready to merge by members who can write to the target branch.", true),
        "not_approved" => ("Merge blocked: all required approvals must be given.", false),
        "draft_status" => ("Merge blocked: Select Mark as ready to remove it from Draft status.", false),
        "discussions_not_resolved" => ("Merge blocked: all threads must be resolved.", false),
        "conflict" => ("Merge blocked: merge conflicts must be resolved.", false),
        "need_rebase" => ("Merge blocked: the source branch must be rebased onto the target branch.", false),
        "ci_must_pass" => ("Merge blocked: pipeline must succeed.", false),
        "ci_still_running" => ("Merge blocked: pipeline must succeed. It's waiting for a manual action or still running.", false),
        "checking" | "unchecked" | "preparing" => ("Checking if merge request can be merged…", false),
        "not_open" => ("Merge request is not open.", false),
        "blocked_status" => ("Merge blocked: another merge request must be merged first.", false),
        _ => ("Merge blocked.", false),
    }
}

impl GitLabView {
    pub(crate) fn mr_author_id(&self) -> u64 {
        self.review.ready().map_or(0, |r| r.mr.author.id)
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
        let sections = [
            (Section::Overview, "Overview", r.mr.user_notes_count as usize),
            (Section::Commits, "Commits", r.commits.len()),
            (Section::Changes, "Changes", r.files.len()),
        ];
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
        div()
            .flex()
            .flex_col()
            .px_4()
            .child(
                div()
                    .pt_3()
                    .text_size(px(30.))
                    .line_height(px(37.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(HEADING))
                    .child(m.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .pt_2()
                    .text_color(c(MUTED))
                    .child(badge(state, c(bg), c(fg)).child(icon("merge", 12., c(fg))).flex_row_reverse())
                    .child(div().ml_1().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(m.author.name.clone()))
                    .child("requested to merge")
                    .child(branch(&m.source_branch, mono.clone()))
                    .child("into")
                    .child(branch(&m.target_branch, mono))
                    .child(when(&m.created_at)),
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
                            .h(px(32.))
                            .px_3()
                            .rounded(px(8.))
                            .bg(c(RAISED))
                            .text_color(c(TEXT))
                            .when(unresolved > 0, |d| {
                                let arrow = |id: &'static str, name: &'static str, forward: bool, cx: &mut Context<Self>| {
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
                                d.child(icon("check-circle", 16., c(GREEN))).child("All threads resolved")
                            }),
                    ),
            )
    }

    pub(crate) fn overview_page(&self, r: &Rc<Review>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let wide = self.sidebar_fits();
        let _ = window;
        // The list's length follows the activity's filter.
        // The top, the activity, the comment form.
        let n = 1 + self.activity_entries(r).len() + 1;
        if self.overview_list.item_count() != n {
            self.overview_list.reset(n);
        }
        let main = list(self.overview_list.clone(), cx.processor(|v, ix: usize, window, cx| v.overview_item(ix, window, cx)))
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
                        .child(self.sidebar(r)),
                )
            })
            .into_any_element()
    }

    fn description(&self, r: &Review, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match r.mr.description.as_deref().map(str::trim) {
            Some(d) if !d.is_empty() => self
                .markdown(("description", r.mr.id as usize), d, &[], None, window, cx)
                .into_any_element(),
            _ => div().text_color(c(MUTED)).italic().child("No description provided.").into_any_element(),
        }
    }

    /// GitLab's pipeline widget: "Pipeline #5 pending" and what it ran on.
    fn pipeline(&self, r: &Review, cx: &mut Context<Self>) -> Option<Div> {
        let p = r.mr.head_pipeline.as_ref()?;
        let (word, _) = p.label();
        let status_icon = |size: f32| pipeline_icon(p, size);
        let mono = self.mono(cx);
        let url = p.web_url.clone();
        let short = p.sha.get(..8).unwrap_or_default().to_string();
        Some(
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
                                        .id("gl-pipeline-link")
                                        .text_color(c(LINK))
                                        .cursor_pointer()
                                        .hover(|s| s.underline())
                                        .child(format!("#{}", p.id))
                                        .on_click(move |_, window, cx| abromux_plugin::open_url(&url, window, cx)),
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
                                .child(div().font_family(mono.clone()).text_color(c(TEXT)).child(short))
                                .child("on")
                                .child(branch(&p.git_ref, mono)),
                        ),
                )
                .child(status_icon(20.)),
        )
    }

    /// The approval and merge status cards.
    fn widgets(&self, r: &Review, mono: SharedString, cx: &mut Context<Self>) -> Div {
        let approvers: Vec<&User> = r.approvals.approved_by.iter().map(|a| &a.user).collect();
        let me = self.me.as_ref().map(|u| u.id);
        let approved_by_me = approvers.iter().any(|u| Some(u.id) == me);
        let card = || div().flex().items_center().gap_3().px_4().py_3().rounded(px(12.)).bg(c(RAISED)).text_color(c(TEXT));
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
                .child(div().font_weight(FontWeight::SEMIBOLD).child(if approved_by_me {
                    "Approved by you".to_string()
                } else {
                    "Approved by".to_string()
                }))
                .child(div().flex().gap_1().children(approvers.iter().map(|u| self.avatars.of(u, 24.))))
        };
        let (text, ok) = merge_status(r.mr.detailed_merge_status.as_deref().unwrap_or(""));
        let n = r.commits.len();
        let mut facts = vec![format!(
            "{n} commit{} and 1 merge commit will be added to {}.",
            if n == 1 { "" } else { "s" },
            r.mr.target_branch
        )];
        if r.mr.force_remove_source_branch || r.mr.should_remove_source_branch.unwrap_or(false) {
            facts.push("Source branch will be deleted.".into());
        }
        let merge = div()
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
                    .child(icon("check-circle", 18., c(if ok { GREEN } else { MUTED })))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(text)),
            )
            .when(ok, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(c(BG))
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .children(facts.into_iter().map(|f| div().flex().gap_2().child("•").child(f))),
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
        let (who, when) = if merged { (&m.merged_by, &m.merged_at) } else { (&m.closed_by, &m.closed_at) };
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
                            div().flex().child(code(s.get(..8).unwrap_or(s)).text_color(c(LINK))).child("."),
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(if merged { "Merged by" } else { "Closed by" }))
                    .when_some(who.as_ref(), |d, u| {
                        d.child(self.avatars.of(u, 16.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(LINK)).child(u.name.clone()))
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Merge details"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .children(facts.into_iter().map(|f| div().flex().gap_2().child("•").child(f))),
                    ),
            )
    }

    /// The activity's entries, filtered and in order.
    fn activity_entries<'a>(&self, r: &'a Review) -> Vec<Entry<'a>> {
        let mut entries: Vec<Entry> = r.system.iter().map(Entry::System).collect();
        entries.extend(r.general.iter().map(Entry::General));
        for f in &r.files {
            entries.extend(f.threads.iter().map(|(h, l, d)| Entry::OnLine(f, *h, *l, d)));
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
                    .child(div().w(px(14.)).when(self.activity == f, |d| d.child(app_icon("check", 14., c(TEXT)))))
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
                    .child(app_icon(if self.newest_first { "chevron-down" } else { "chevron-up" }, 14., c(TEXT)))
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
    fn overview_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
                .child(self.description(&r, window, cx))
                .child(awards(r.mr.upvotes, r.mr.downvotes))
                .children(self.pipeline(&r, cx))
                .child(self.widgets(&r, self.mono(cx), cx))
                .child(self.activity_header(cx))
                .into_any_element();
        }
        let entries = self.activity_entries(&r);
        // After the activity: GitLab's comment form.
        if ix == entries.len() + 1 {
            return pad(div()).pt_2().pb_6().child(self.comment_form(cx)).into_any_element();
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
                    .child(div().absolute().left(px(15.)).top_0().bottom_0().w(px(2.)).bg(c(RAISED)))
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
                            .when(!d.individual_note, |el| el.child(self.reply_bar(&d.id, d.resolvable(), d.resolved(), cx))),
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
            } else {
                // A commit's full sha, shortened as GitLab shows it.
                let short = (word.len() == 40 && word.bytes().all(|b| b.is_ascii_hexdigit())).then(|| &word[..8]);
                match short {
                    Some(s) => body.child(div().font_family(self.mono(cx)).text_color(c(LINK)).child(s.to_string())),
                    None => body.child(div().text_color(c(MUTED)).child(word.to_string())),
                }
            };
        }
        // GitLab marks an approval with a green check, merging and closing
        // with the merge icon in blue or red; other notes with a dot.
        let circle = |bg: u32| div().size(px(20.)).rounded_full().bg(c(bg)).flex().items_center().justify_center();
        let marker = if text.starts_with("approved this merge request") {
            circle(0x0d532a).child(app_icon("check", 12., c(0x91d4a8)))
        } else if text == "merged" || text.starts_with("merged ") {
            circle(0x22306a).child(icon("merge", 12., c(0xc8e0f9)))
        } else if text == "closed" || text.starts_with("closed ") {
            circle(0x641c28).child(icon("merge", 12., c(0xfdd4cd)))
        } else {
            div().size(px(8.)).rounded_full().bg(c(0x89888d)).border_2().border_color(c(BG))
        };
        let line = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().w(px(32.)).flex_none().flex().justify_center().child(marker))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(n.author.name.clone()))
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
            .map(|r| r.versions.iter().map(|v| (v.id, v.head_commit_sha.clone())).collect())
            .unwrap_or_default();
        let pick = compare.and_then(|(start, diff_id)| {
            // The previous version, by its head commit; this note's version, by id.
            let base = versions.iter().find(|(_, sha)| sha.starts_with(&start) || start.starts_with(sha.as_str()))?.0;
            let head = versions.iter().find(|(id, _)| *id == diff_id)?.0;
            Some(crate::Compare { base: Some(base), head: Some(head), commit: None })
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
                    div()
                        .flex()
                        .gap_2()
                        .text_color(c(MUTED))
                        .child("•")
                        .child(div().font_family(mono.clone()).text_color(c(TEXT)).child(sha.to_string()))
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(first.author.name.clone()))
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
                        None => div().text_color(c(TEXT)).child("an old version of the diff").into_any_element(),
                    })
                    .child(when(&first.created_at)),
            )
            .when_some(d.resolution().filter(|_| resolved), |e, (who, when)| {
                e.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(c(MUTED))
                        .child(format!("Resolved {} by {}", time::ago(when, time::now()), who.name)),
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
            .child(app_icon(if open { "chevron-down" } else { "chevron-right" }, 14., c(LINK)))
            .when(open, |e| e.child(if resolved { "Hide thread" } else { "Collapse replies" }))
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
                let set = if resolved { &mut v.open_resolved } else { &mut v.collapsed_replies };
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
                                el.cursor_pointer()
                                    .hover(|s| s.underline())
                                    .on_click(cx.listener(move |v, _, _, cx| v.show_thread(&id, cx)))
                            })
                            .child(path.clone())
                    })
                    .child(icon("copy", 14., c(MUTED)))
                    .when_some(at.map(|(f, _, _)| f), |e, f| {
                        e.when(f.meta.a_mode != f.meta.b_mode && !f.meta.a_mode.is_empty(), |e| {
                            e.child(div().text_size(px(12.)).text_color(c(TEXT)).child(format!("{} → {}", f.meta.a_mode, f.meta.b_mode)))
                        })
                    }),
            );
            if let Some((f, h, l)) = at {
                // Up to 16 lines of the hunk, ending at the thread's line.
                let lines = &f.body.hunks()[h].lines;
                let start = (l + 1).saturating_sub(16);
                card = card.children(
                    lines[start..=l]
                        .iter()
                        .filter(|x| x.kind != Kind::NoNewline)
                        .map(|x| code_line(x, &mono, None)),
                );
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
                    .child(self.reply_bar(&d.id, d.resolvable(), d.resolved(), cx).rounded_b(px(8.))),
            );
        }
        div()
            .flex()
            .gap_3()
            .child(self.avatars.of(&first.author, 32.))
            .child(card)
            .into_any_element()
    }

    fn sidebar(&self, r: &Review) -> Div {
        let approved: Vec<u64> = r.approvals.approved_by.iter().map(|a| a.user.id).collect();
        let block = |title: &str| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .pt_3()
                .pb_4()
                .border_b_1()
                .border_color(c(RAISED))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(title.to_string()))
        };
        let none = || div().text_color(c(MUTED)).child("None");
        // A reviewer: approved (a green check) or still to (a dashed circle).
        let person = |u: &User, review: Option<bool>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_color(c(TEXT))
                .child(self.avatars.of(u, 24.))
                .child(div().flex_1().child(u.name.clone()))
                .when(review == Some(true), |d| d.child(icon("check-circle", 16., c(GREEN))))
                .when(review == Some(false), |d| d.child(icon("circle-dashed", 16., c(MUTED))))
        };
        // GitLab's participants: the author, who's on it, and who commented.
        let mut participants: Vec<&User> = vec![&r.mr.author];
        let threads = r.general.iter().chain(r.outdated.iter()).chain(r.files.iter().flat_map(|f| f.threads.iter().map(|(_, _, d)| d)));
        let commenters = threads.flat_map(|d| d.notes.iter()).map(|n| &n.author);
        for u in r.mr.assignees.iter().chain(&r.mr.reviewers).chain(commenters) {
            if u.id != 0 && !participants.iter().any(|p| p.id == u.id) {
                participants.push(u);
            }
        }
        div()
            .w(px(290.))
            .flex_none()
            .flex()
            .flex_col()
            .child(
                block(if r.mr.assignees.len() > 1 { "Assignees" } else { "Assignee" })
                    .when(r.mr.assignees.is_empty(), |d| d.child(none()))
                    .children(r.mr.assignees.iter().map(|u| person(u, None))),
            )
            .child(
                block(if r.mr.reviewers.len() > 1 { "Reviewers" } else { "Reviewer" })
                    .when(r.mr.reviewers.is_empty(), |d| d.child(none()))
                    .children(r.mr.reviewers.iter().map(|u| person(u, Some(approved.contains(&u.id))))),
            )
            .child(
                block("Labels")
                    .when(r.mr.labels.is_empty(), |d| d.child(none()))
                    .child(div().flex().flex_wrap().gap_1().children(r.mr.labels.iter().map(label_pill))),
            )
            .child(block("Milestone").child(match &r.mr.milestone {
                Some(m) => div().text_color(c(TEXT)).child(m.title.clone()),
                None => none(),
            }))
            .child(
                block(&format!("{} Participant{}", participants.len(), if participants.len() == 1 { "" } else { "s" }))
                    .child(div().flex().flex_wrap().gap_1().children(participants.iter().map(|u| self.avatars.of(u, 24.)))),
            )
    }

    pub(crate) fn commits_page(&self, r: &Review, cx: &mut Context<Self>) -> AnyElement {
        let mono = self.mono(cx);
        let mut days: Vec<(String, Vec<&crate::types::Commit>)> = vec![];
        for cm in &r.commits {
            let date = if cm.authored_date.is_empty() { &cm.created_at } else { &cm.authored_date };
            let day = time::day(date);
            match days.last_mut() {
                Some((d, list)) if *d == day => list.push(cm),
                _ => days.push((day, vec![cm])),
            }
        }
        // The GitLab user behind a commit: by the email's name part, as
        // GitLab matches commit emails to accounts.
        let mut known: Vec<&User> = vec![&r.mr.author];
        known.extend(&r.mr.assignees);
        known.extend(&r.mr.reviewers);
        let user_of = |cm: &crate::types::Commit| -> User {
            let local = cm.author_email.split('@').next().unwrap_or_default();
            known
                .iter()
                .find(|u| u.username == local)
                .map(|u| (*u).clone())
                .unwrap_or_else(|| User { id: cm.author_email.len() as u64, name: cm.author_name.clone(), ..Default::default() })
        };
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
                    let when = if cm.authored_date.is_empty() { &cm.created_at } else { &cm.authored_date };
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
                                                .on_click(cx.listener(move |v, _, _, cx| v.open_commit(index, cx))),
                                        )
                                        // GitLab's ⋯ for the rest of the message.
                                        .when(cm.body().is_some(), |d| {
                                            let id = cm.id.clone();
                                            d.child(
                                                div()
                                                    .id(SharedString::from(format!("body-{}", cm.id)))
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
                                        .child(div().text_color(c(MUTED)).child(format!("authored {}", time::ago(when, time::now())))),
                                )
                                .when_some(cm.body().filter(|_| self.open_bodies.contains(&cm.id)), |d, body| {
                                    d.child(
                                        div()
                                            .mt_2()
                                            .font_family(mono.clone())
                                            .text_size(px(12.))
                                            .line_height(px(18.))
                                            .text_color(c(TEXT))
                                            .whitespace_normal()
                                            .children(body.lines().map(|l| div().child(if l.is_empty() { " ".to_string() } else { l.to_string() }))),
                                    )
                                }),
                        )
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
                                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()))),
                                ),
                        )
                }))
        });
        div()
            .id("gl-commits")
            .size_full()
            .overflow_y_scroll()
            .child(div().flex().flex_col().gap_4().px_4().py_2().children(groups))
            .into_any_element()
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
