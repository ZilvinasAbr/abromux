//! The Projects tab: the projects you're a member of, most recently active
//! first, searched on GitLab as you type, each with its count of open merge
//! requests; picking one lists its merge requests (`Location::browse`).

use crate::{
    Browse,
    api::{Scope, count_path},
    style::*,
    time,
    types::Project,
    view::{Failure, GitLabView, Load},
};
use gpui::{prelude::*, *};

/// How many projects get their open merge requests counted (one short
/// request each, all in one batch).
const COUNTED: usize = 60;

impl GitLabView {
    /// The projects for the search box's text: the list first, then each
    /// one's count of open merge requests.
    pub(crate) fn load_projects(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        let search = self.list_search.read(cx).value().trim().to_string();
        // Keep showing what's there while a search runs.
        if self.projects.ready().is_none() {
            self.projects = Load::Loading;
        }
        self._projects_task = Some(cx.spawn(async move |this, cx| {
            let c2 = c.clone();
            let got = cx.background_spawn(async move { c2.projects(&search) }).await;
            let ids: Vec<u64> = got.as_ref().map(|ps| ps.iter().take(COUNTED).map(|p| p.id).collect()).unwrap_or_default();
            let ok = this
                .update(cx, |v, cx| {
                    v.projects = match got {
                        Ok(ps) => Load::Ready(std::rc::Rc::new(ps)),
                        Err(e) => Load::Failed(Failure::of(&e)),
                    };
                    cx.notify();
                })
                .is_ok();
            let wanted: Vec<u64> = this
                .update(cx, |v, _| ids.iter().copied().filter(|id| !v.project_counts.contains_key(id)).collect())
                .unwrap_or_default();
            if !ok || wanted.is_empty() {
                return;
            }
            let counts = cx
                .background_spawn(async move {
                    c.prefetch(wanted.iter().map(|id| count_path(&Scope::Project(*id), "opened")));
                    let counts: Vec<(u64, u64)> = wanted
                        .iter()
                        .filter_map(|id| Some((*id, c.count_merge_requests(&Scope::Project(*id), "opened").ok()??)))
                        .collect();
                    c.forget_prefetched();
                    counts
                })
                .await;
            this.update(cx, |v, cx| {
                v.project_counts.extend(counts);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// A project's merge requests.
    pub(crate) fn browse(&mut self, b: Browse, window: &mut Window, cx: &mut Context<Self>) {
        self.loc.browse = Some(b);
        self.list_search.update(cx, |s, cx| s.set_value("", window, cx));
        self.load_list(cx);
    }

    /// Back to the Projects tab from a project's merge requests.
    pub(crate) fn all_projects(&mut self, cx: &mut Context<Self>) {
        self.loc.browse = None;
        self.loc.scope = crate::ListScope::Projects;
        self.load_list(cx);
    }

    /// The Projects tab's rows: GitLab's project list, an identicon, the
    /// namespace and name, the description, open merge requests, activity.
    pub(crate) fn project_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(projects) = self.projects.ready() else { return vec![] };
        if projects.is_empty() {
            let text = if self.list_search.read(cx).value().trim().is_empty() {
                "You're not a member of any projects."
            } else {
                "No projects found."
            };
            return vec![div().py_6().flex().justify_center().text_color(c(MUTED)).child(text).into_any_element()];
        }
        let now = time::now();
        projects
            .iter()
            .map(|p| {
                let browse = Browse { id: p.id, path: p.path_with_namespace.clone() };
                let (namespace, _) = p.name_with_namespace.rsplit_once(" / ").unwrap_or(("", &p.name));
                let open = self.project_counts.get(&p.id).copied();
                div()
                    .id(SharedString::from(format!("project-{}", p.id)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py(px(12.))
                    .border_b_1()
                    .border_color(c(RAISED))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(CARD)))
                    .on_click(cx.listener(move |v, _, window, cx| v.browse(browse.clone(), window, cx)))
                    .child(identicon(p))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .when(!namespace.is_empty(), |d| d.child(div().text_color(c(MUTED)).child(format!("{namespace} /"))))
                                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(TEXT)).child(p.name.clone())),
                            )
                            .children(p.description.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(|d| {
                                div().truncate().text_size(px(12.)).line_height(px(18.)).text_color(c(MUTED)).child(d.to_string())
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(2.))
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(c(MUTED))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_color(c(if open.is_some_and(|n| n > 0) { TEXT } else { MUTED }))
                                    .child(icon("merge", 14., c(MUTED)))
                                    .child(open.map_or("–".to_string(), |n| n.to_string())),
                            )
                            .child(format!("Updated {}", time::ago(&p.last_activity_at, now))),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

/// GitLab's identicon for a project without an avatar: its initial on one
/// of seven colours picked by its id.
fn identicon(p: &Project) -> Div {
    const BG: [u32; 7] = [0x5b3a6e, 0x2f4a73, 0x1d5a46, 0x6b5b1f, 0x6e3a2f, 0x355a6b, 0x5e3a4f];
    const FG: [u32; 7] = [0xe9d1f5, 0xd2e4f9, 0xc4f0dc, 0xf5e6b3, 0xf9d3c9, 0xcde9f2, 0xf5d1e3];
    let i = (p.id % 7) as usize;
    let initial = p.name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default();
    div()
        .size(px(32.))
        .flex_none()
        .rounded(px(4.))
        .bg(c(BG[i]))
        .flex()
        .items_center()
        .justify_center()
        .text_color(c(FG[i]))
        .font_weight(FontWeight::SEMIBOLD)
        .child(initial)
}
