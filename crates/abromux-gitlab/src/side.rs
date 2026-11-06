//! The merge request's sidebar, as GitLab has it: assignees, reviewers,
//! labels, milestone and participants, each with GitLab's "Edit" dropdown.
//!
//! Assignees, reviewers and labels pick several, and like GitLab's the
//! dropdown saves when it closes (a click outside, Edit again, or Escape).
//! A milestone is one pick, saved at once. "assign yourself" and a label's
//! × save straight away too.

use crate::{
    review::Review,
    style::*,
    types::{Label, User},
    view::GitLabView,
};
use gpui::{prelude::*, *};
use gpui_component::input::Input;
use serde_json::json;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideMenu {
    Assignees,
    Reviewers,
    Labels,
    Milestone,
}

impl SideMenu {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "assignees" | "assignee" => SideMenu::Assignees,
            "reviewers" | "reviewer" => SideMenu::Reviewers,
            "labels" => SideMenu::Labels,
            "milestone" => SideMenu::Milestone,
            _ => return None,
        })
    }

    fn title(self, multiple: (bool, bool)) -> &'static str {
        match self {
            SideMenu::Assignees if multiple.0 => "Select assignees",
            SideMenu::Assignees => "Select assignee",
            SideMenu::Reviewers if multiple.1 => "Select reviewers",
            SideMenu::Reviewers => "Select reviewer",
            SideMenu::Labels => "Select labels",
            SideMenu::Milestone => "Assign milestone",
        }
    }
}

/// What an open dropdown has ticked, not yet saved.
#[derive(Clone, Debug, Default)]
pub struct Pick {
    pub people: Vec<u64>,
    pub labels: Vec<String>,
    /// When it closed on a click outside: a click on its own Edit button
    /// right after shouldn't open it again.
    pub closed: Option<(SideMenu, Instant)>,
}

impl GitLabView {
    /// Open dropdown `m` with what's set now ticked (or close it if open).
    pub(crate) fn toggle_side_menu(&mut self, m: SideMenu, window: &mut Window, cx: &mut Context<Self>) {
        if self.side_menu == Some(m) {
            self.close_side_menu(window, cx);
            return;
        }
        if self.pick.closed.is_some_and(|(c, at)| c == m && at.elapsed() < Duration::from_millis(300)) {
            return;
        }
        if self.side_menu.is_some() {
            self.close_side_menu(window, cx);
        }
        let Some(r) = self.review.ready() else { return };
        self.pick.people = match m {
            SideMenu::Assignees => r.mr.assignees.iter().map(|u| u.id).collect(),
            SideMenu::Reviewers => r.mr.reviewers.iter().map(|u| u.id).collect(),
            _ => vec![],
        };
        self.pick.labels = r.mr.labels.iter().map(|l| l.name.clone()).collect();
        self.side_menu = Some(m);
        self.boxes.search.update(cx, |b, cx| {
            b.set_value("", window, cx);
            b.focus(window, cx);
        });
        cx.notify();
    }

    /// Close the open dropdown, saving its picks if they changed.
    pub(crate) fn close_side_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.side_menu.take() else { return };
        window.focus(&self.focus);
        cx.notify();
        let Some(r) = self.review.ready() else { return };
        let ids = |us: &[User]| us.iter().map(|u| u.id).collect::<Vec<_>>();
        let fields = match m {
            SideMenu::Assignees if ids(&r.mr.assignees) != self.pick.people => json!({ "assignee_ids": self.pick.people }),
            SideMenu::Reviewers if ids(&r.mr.reviewers) != self.pick.people => json!({ "reviewer_ids": self.pick.people }),
            SideMenu::Labels if r.mr.labels.iter().map(|l| &l.name).ne(self.pick.labels.iter()) => {
                json!({ "labels": self.pick.labels.join(",") })
            }
            _ => return,
        };
        self.save_side(fields, window, cx);
    }

    /// PUT the merge request's `fields`, then fetch it again.
    pub(crate) fn save_side(&mut self, fields: serde_json::Value, window: &mut Window, cx: &mut Context<Self>) {
        self.write(crate::edit::Write::Sidebar, move |c, p, iid| c.update(p, iid, fields).map(|_| ()), |_, _, _| {}, window, cx);
    }

    /// Tick or untick a person or a label in the open dropdown; pick a
    /// milestone (0: none) and save.
    pub(crate) fn pick_person(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        // One allowed (GitLab's free tier): a pick replaces, and saves.
        let multiple = self.review.ready().map(|r| r.multiple);
        let single = match (self.side_menu, multiple) {
            (Some(SideMenu::Assignees), Some((m, _))) | (Some(SideMenu::Reviewers), Some((_, m))) => !m,
            _ => false,
        };
        if single {
            self.pick.people = if self.pick.people == [id] { vec![] } else { vec![id] };
            self.close_side_menu(window, cx);
            return;
        }
        if let Some(i) = self.pick.people.iter().position(|p| *p == id) {
            self.pick.people.remove(i);
        } else {
            self.pick.people.push(id);
        }
        cx.notify();
    }

    pub(crate) fn pick_label(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.pick.labels.iter().position(|l| l == name) {
            self.pick.labels.remove(i);
        } else {
            self.pick.labels.push(name.to_string());
        }
        cx.notify();
    }

    pub(crate) fn pick_milestone(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.side_menu = None;
        window.focus(&self.focus);
        self.save_side(json!({ "milestone_id": id }), window, cx);
    }

    /// The people picker's search, on a project with more members than were
    /// loaded: asked of GitLab, the latest query's answer kept.
    pub(crate) fn search_people(&mut self, cx: &mut Context<Self>) {
        let people = matches!(self.side_menu, Some(SideMenu::Assignees | SideMenu::Reviewers));
        let (Some(r), Some(c)) = (self.review.ready().cloned(), self.client.clone()) else { return };
        let query = self.boxes.search.read(cx).value().trim().to_lowercase();
        if !people || !r.members_more || query.is_empty() {
            self.people_found = None;
            return;
        }
        let project = r.mr.project_id;
        self._people_task = Some(cx.spawn(async move |this, cx| {
            let q = query.clone();
            let got = cx.background_spawn(async move { c.search_members(project, &q) }).await;
            this.update(cx, |v, cx| {
                if let Ok(found) = got {
                    let mut people: Vec<User> = vec![];
                    for m in found.iter().filter(|m| m.state.is_empty() || m.state == "active") {
                        if !people.iter().any(|p| p.id == m.id) {
                            people.push(m.user());
                        }
                    }
                    v.avatars.want(people.iter().collect::<Vec<_>>(), cx, |v| &mut v.avatars);
                    v.people_found = Some((query, people));
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    // ---- what it looks like ---------------------------------------------

    pub(crate) fn sidebar(&self, r: &Review, cx: &mut Context<Self>) -> Div {
        let approved: Vec<u64> = r.approvals.approved_by.iter().map(|a| a.user.id).collect();
        // Only someone signed in edits, and not while comparing versions.
        let editable = self.me.is_some();
        let none = || div().text_color(c(MUTED)).child("None");
        // A reviewer: their review's state as GitLab's icon, with its words.
        let person = |u: &User, review: Option<&str>| {
            let state = review.map(|s| match s {
                "approved" => ("check-circle", c(GREEN), "Reviewer approved changes"),
                "requested_changes" => ("minus-circle-filled", c(RED), "Reviewer requested changes"),
                "reviewed" => ("comment-lines", c(LINK), "Reviewer commented"),
                "review_started" => ("comment", c(LINK), "Reviewer started review"),
                _ => ("circle-dashed", c(MUTED), "Awaiting review"),
            });
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_color(c(TEXT))
                .child(self.avatars.of(u, 24.))
                .child(div().flex_1().child(u.name.clone()))
                .when_some(state, |d, (name, color, tip)| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("review-state-{}", u.id)))
                            .child(icon(name, 16., color))
                            .tooltip(hint(tip)),
                    )
                })
        };
        // GitLab's participants: the author, who's on it, who commented, and
        // whoever its own notes name as acting ("Administrator added 1 commit").
        let mut participants: Vec<&User> = vec![&r.mr.author];
        let threads = r.general.iter().chain(r.outdated.iter()).chain(r.files.iter().flat_map(|f| f.threads.iter().map(|(_, _, d)| d)));
        let commenters = threads.flat_map(|d| d.notes.iter()).chain(&r.system).map(|n| &n.author);
        for u in r.mr.assignees.iter().chain(&r.mr.reviewers).chain(commenters) {
            if u.id != 0 && !participants.iter().any(|p| p.id == u.id) {
                participants.push(u);
            }
        }
        let me = self.me.clone();
        // "None - assign yourself", as GitLab offers.
        let assign_me = me.filter(|_| editable && r.mr.assignees.is_empty()).map(|me| {
            div()
                .flex()
                .gap_1()
                .text_color(c(MUTED))
                .child("None -")
                .child(
                    div()
                        .id("gl-assign-me")
                        .text_color(c(LINK))
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child("assign yourself")
                        .on_click(cx.listener(move |v, _, window, cx| v.save_side(json!({ "assignee_ids": [me.id] }), window, cx))),
                )
        });
        let labels = div().flex().flex_wrap().gap_1().children(r.mr.labels.iter().map(|l| {
            let name = l.name.clone();
            label_pill(l).when(editable, |p| {
                // GitLab's × on a label removes it.
                let fg = if l.text_color.is_empty() { c(TEXT) } else { hex(&l.text_color) };
                p.flex().items_center().gap_1().child(
                    div()
                        .id(SharedString::from(format!("gl-unlabel-{name}")))
                        .cursor_pointer()
                        .rounded_full()
                        .hover(|s| s.bg(ca(0x00000033)))
                        .child(app_icon("close", 12., fg))
                        .on_click(cx.listener(move |v, _, window, cx| {
                            v.save_side(json!({ "remove_labels": name }), window, cx)
                        })),
                )
            })
        }));
        div()
            .w(px(290.))
            .flex_none()
            .flex()
            .flex_col()
            .child(
                self.block(if r.mr.assignees.len() > 1 { "Assignees" } else { "Assignee" }, editable.then_some(SideMenu::Assignees), r, cx)
                    .when(r.mr.assignees.is_empty(), |d| match assign_me {
                        Some(a) => d.child(a),
                        None => d.child(none()),
                    })
                    .children(r.mr.assignees.iter().map(|u| person(u, None))),
            )
            .child(
                self.block(if r.mr.reviewers.len() > 1 { "Reviewers" } else { "Reviewer" }, editable.then_some(SideMenu::Reviewers), r, cx)
                    .when(r.mr.reviewers.is_empty(), |d| d.child(none()))
                    .children(r.mr.reviewers.iter().map(|u| {
                        // GitLab's own word on it, else what the approvals say.
                        let state = r
                            .review_states
                            .get(&u.id)
                            .map(String::as_str)
                            .unwrap_or(if approved.contains(&u.id) { "approved" } else { "unreviewed" });
                        person(u, Some(state))
                    })),
            )
            .child(
                self.block("Labels", editable.then_some(SideMenu::Labels), r, cx)
                    .when(r.mr.labels.is_empty(), |d| d.child(none()))
                    .child(labels),
            )
            .child(self.block("Milestone", editable.then_some(SideMenu::Milestone), r, cx).child(match &r.mr.milestone {
                Some(m) => div().text_color(c(TEXT)).child(m.title.clone()),
                None => none(),
            }))
            // GitLab's "Work items": what merging closes.
            .child(
                self.block("Work items", None, r, cx)
                    .when(r.closes.is_empty(), |d| d.child(none()))
                    .children(r.closes.iter().map(|i| {
                        let url = i.web_url.clone();
                        div()
                            .id(SharedString::from(format!("closes-{}", i.iid)))
                            .flex()
                            .gap_1()
                            .min_w(px(0.))
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child(div().flex_none().text_color(c(MUTED)).child(format!("#{}", i.iid)))
                            .child(div().truncate().text_color(c(if i.state == "closed" { MUTED } else { TEXT })).child(i.title.clone()))
                            .on_click(move |_, window, cx| abromux_plugin::open_url(&url, window, cx))
                    })),
            )
            .child(self.block("Time tracking", None, r, cx).child(time_tracking(&r.mr.time_stats)))
            .child(
                self.block(&format!("{} Participant{}", participants.len(), if participants.len() == 1 { "" } else { "s" }), None, r, cx)
                    .child(div().flex().flex_wrap().gap_1().children(participants.iter().map(|u| self.avatars.of(u, 24.)))),
            )
    }

    /// A sidebar block: its title, GitLab's "Edit" and, open, its dropdown.
    fn block(&self, title: &str, menu: Option<SideMenu>, r: &Review, cx: &mut Context<Self>) -> Div {
        let head = div()
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .h(px(24.))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(title.to_string()))
            .children(menu.map(|m| {
                let open = self.side_menu == Some(m);
                div()
                    .id(SharedString::from(format!("gl-edit-{m:?}")))
                    .px_2()
                    .py(px(2.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c(TEXT))
                    .hover(|s| s.bg(ca(0x89888d40)))
                    .when(open, |d| d.bg(ca(0x89888d40)))
                    .child("Edit")
                    .on_click(cx.listener(move |v, _, window, cx| v.toggle_side_menu(m, window, cx)))
            }))
            .children(menu.filter(|m| self.side_menu == Some(*m)).map(|m| self.dropdown(m, r, cx)));
        div().flex().flex_col().gap_1().pt_3().pb_4().border_b_1().border_color(c(RAISED)).child(head)
    }

    fn dropdown(&self, m: SideMenu, r: &Review, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.boxes.search.read(cx).value().trim().to_lowercase();
        let hits = |s: &str| query.is_empty() || s.to_lowercase().contains(&query);
        let check = |on: bool| div().w(px(16.)).flex_none().when(on, |d| d.child(app_icon("check", 14., c(TEXT))));
        let item = |id: SharedString| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .mx_1()
                .px_2()
                .py(px(6.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(c(RAISED)))
        };
        let section = |t: &'static str| {
            div().px_3().pt_2().pb_1().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(t)
        };
        let mut rows: Vec<AnyElement> = vec![];
        match m {
            SideMenu::Assignees | SideMenu::Reviewers => {
                // Who's on it first, as GitLab lists them, then everyone else.
                let current = if m == SideMenu::Assignees { &r.mr.assignees } else { &r.mr.reviewers };
                let mut people: Vec<&User> = current.iter().collect();
                // A big project's search is GitLab's (`search_people`); else the loaded members'.
                let found = self.people_found.as_ref().filter(|(q, _)| *q == query).map(|(_, f)| f);
                let pool = found.unwrap_or(&r.members);
                people.extend(pool.iter().filter(|u| !current.iter().any(|c| c.id == u.id)));
                let people: Vec<&User> = people.into_iter().filter(|u| hits(&u.name) || hits(&u.username)).collect();
                let first = people.iter().take_while(|u| current.iter().any(|c| c.id == u.id)).count();
                let person = |u: &User, cx: &mut Context<Self>| {
                    let id = u.id;
                    item(SharedString::from(format!("gl-pick-{id}")))
                        .child(check(self.pick.people.contains(&id)))
                        .child(self.avatars.of(u, 32.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w(px(0.))
                                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(u.name.clone()))
                                .child(div().text_color(c(MUTED)).child(format!("@{}", u.username))),
                        )
                        .on_click(cx.listener(move |v, _, window, cx| v.pick_person(id, window, cx)))
                        .into_any_element()
                };
                if first > 0 {
                    rows.push(section(if m == SideMenu::Assignees { "Assignees" } else { "Reviewers" }).into_any_element());
                }
                for (i, u) in people.iter().enumerate() {
                    if i == first && first > 0 {
                        rows.push(div().my_1().h(px(1.)).bg(c(RAISED)).into_any_element());
                    }
                    rows.push(person(u, cx));
                }
            }
            SideMenu::Labels => {
                for l in r.labels.iter().filter(|l| hits(&l.name)) {
                    let name = l.name.clone();
                    rows.push(
                        item(SharedString::from(format!("gl-pick-label-{name}")))
                            .child(check(self.pick.labels.contains(&l.name)))
                            .child(swatch(l))
                            .child(div().text_color(c(TEXT)).child(l.name.clone()))
                            .on_click(cx.listener(move |v, _, _, cx| v.pick_label(&name, cx)))
                            .into_any_element(),
                    );
                }
            }
            SideMenu::Milestone => {
                let now = r.mr.milestone.as_ref().map(|m| m.title.as_str());
                rows.push(
                    item("gl-pick-no-milestone".into())
                        .child(check(now.is_none()))
                        .child(div().text_color(c(TEXT)).child("No milestone"))
                        .on_click(cx.listener(|v, _, window, cx| v.pick_milestone(0, window, cx)))
                        .into_any_element(),
                );
                for ms in r.milestones.iter().filter(|ms| hits(&ms.title)) {
                    let id = ms.id;
                    rows.push(
                        item(SharedString::from(format!("gl-pick-ms-{id}")))
                            .child(check(now == Some(ms.title.as_str())))
                            .child(div().text_color(c(TEXT)).child(ms.title.clone()))
                            .on_click(cx.listener(move |v, _, window, cx| v.pick_milestone(id, window, cx)))
                            .into_any_element(),
                    );
                }
            }
        }
        if rows.is_empty() {
            rows.push(div().px_3().py_2().text_color(c(MUTED)).child("No matching results").into_any_element());
        }
        let people = matches!(m, SideMenu::Assignees | SideMenu::Reviewers);
        deferred(
            div()
                .id("gl-side-menu")
                .absolute()
                .top(px(28.))
                .right_0()
                .w(px(272.))
                .rounded(px(8.))
                .border_1()
                .border_color(c(BORDER))
                .bg(c(CARD))
                .shadow_lg()
                .occlude()
                .on_mouse_down_out(cx.listener(move |v, _, window, cx| {
                    v.close_side_menu(window, cx);
                    v.pick.closed = Some((m, Instant::now()));
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(c(RAISED))
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(14.)).text_color(c(TEXT)).child(m.title(r.multiple)))
                        .when(people, |d| {
                            d.child(
                                div()
                                    .id("gl-unassign")
                                    .px_1()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .text_size(px(14.))
                                    .text_color(c(TEXT))
                                    .hover(|s| s.bg(c(RAISED)))
                                    .child("Unassign")
                                    .on_click(cx.listener(|v, _, window, cx| {
                                        v.pick.people.clear();
                                        v.close_side_menu(window, cx);
                                    })),
                            )
                        }),
                )
                .child(
                    div().p_2().child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .h(px(32.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(c(0x1068bf))
                            .bg(c(BG))
                            .child(app_icon("search", 14., c(MUTED)))
                            .child(div().flex_1().child(Input::new(&self.boxes.search).appearance(false))),
                    ),
                )
                .child(div().id("gl-side-menu-items").max_h(px(300.)).overflow_y_scroll().pb_1().children(rows)),
        )
        .with_priority(1)
    }
}

/// A label's colour, in its dropdown row.
fn swatch(l: &Label) -> Div {
    let bg = if l.color.is_empty() { c(RAISED) } else { hex(&l.color) };
    div().size(px(12.)).flex_none().rounded(px(3.)).bg(bg)
}

/// GitLab's time tracking line: what's spent of the estimate, or that
/// there's neither.
fn time_tracking(t: &crate::types::TimeStats) -> Div {
    let spent = t.human_total_time_spent.clone().filter(|_| t.total_time_spent > 0);
    let estimate = t.human_time_estimate.clone().filter(|_| t.time_estimate > 0);
    let text = match (spent, estimate) {
        (None, None) => return div().text_color(c(MUTED)).child("No estimate or time spent"),
        (Some(s), None) => format!("Spent: {s}"),
        (None, Some(e)) => format!("Estimate: {e}"),
        (Some(s), Some(e)) => format!("Spent: {s} · Estimate: {e}"),
    };
    div().text_color(c(TEXT)).child(text)
}
