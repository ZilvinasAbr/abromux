//! GitLab's "Search or go to…": `/` (or `s`) anywhere in the tab opens a
//! search over the page for merge requests. What's typed is searched on
//! GitLab across every project you can see (title and description), `!12`
//! finds merge requests numbered 12, and a pasted merge request link opens
//! as it is. With nothing typed it shows the list the tab has loaded. ↑ ↓
//! pick, Enter opens, Esc closes.

use crate::{
    api,
    style::*,
    time,
    types::MergeRequest,
    view::{GitLabView, Load},
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};

/// The search over the tab, while it's open.
pub struct Search {
    pub input: Entity<InputState>,
    /// GitLab's answer for `query` (None: nothing typed, the loaded list).
    pub found: Option<(String, Load<Vec<MergeRequest>>)>,
    pub selected: usize,
    _task: Option<Task<()>>,
}

impl GitLabView {
    pub(crate) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search merge requests, or paste a link"));
        cx.subscribe_in(&input, window, |v, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => v.search_changed(window, cx),
            InputEvent::PressEnter { .. } => v.search_open_selected(window, cx),
            _ => {}
        })
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        self.search = Some(Search { input, found: None, selected: 0, _task: None });
        cx.notify();
    }

    pub(crate) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.take().is_some() {
            window.focus(&self.focus);
            cx.notify();
        }
    }

    /// What's typed changed: a link opens, else GitLab is asked.
    fn search_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.search else { return };
        let query = s.input.read(cx).value().trim().to_string();
        if api::parse_mr_url(&query).is_some() || api::parse_project_mrs_url(&query).is_some() {
            self.close_search(window, cx);
            self.open_link(&query, window, cx);
            return;
        }
        let Some(c) = self.client.clone() else { return };
        let s = self.search.as_mut().unwrap();
        s.selected = 0;
        if query.is_empty() {
            s.found = None;
            s._task = None;
            return cx.notify();
        }
        // The last answer stays up while the next is asked.
        let shown = s.found.take().and_then(|(_, l)| l.ready().cloned()).map_or(Load::Loading, Load::Ready);
        s.found = Some((query.clone(), shown));
        let q = query.clone();
        // `!12` looks in the projects at hand: the open merge request's, the
        // one being browsed, and those in the list.
        let mut projects: Vec<u64> = self.loc.project.into_iter().chain(self.loc.browse.as_ref().map(|b| b.id)).collect();
        projects.extend(self.list.ready().into_iter().flatten().map(|m| m.project_id));
        let mut seen = std::collections::HashSet::new();
        projects.retain(|p| *p != 0 && seen.insert(*p));
        let s = self.search.as_mut().unwrap();
        s._task = Some(cx.spawn(async move |this, cx| {
            // Typing fast: only the query that stood still a moment is asked.
            cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
            let got = cx.background_spawn(async move { c.search_merge_requests(&q, &projects) }).await;
            this.update(cx, |v, cx| {
                let Some(s) = v.search.as_mut() else { return };
                if s.found.as_ref().is_some_and(|(asked, _)| *asked == query) {
                    let got = got.map_err(|e| format!("{e:#}"));
                    if let Ok(mrs) = &got {
                        v.avatars.want(mrs.iter().map(|m| &m.author), cx, |v| &mut v.avatars);
                    }
                    let s = v.search.as_mut().unwrap();
                    s.found = Some((query, got.map_or_else(Load::Failed, Load::Ready)));
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// The rows the search shows now.
    fn search_rows(&self) -> Vec<MergeRequest> {
        let Some(s) = &self.search else { return vec![] };
        match &s.found {
            Some((_, found)) => found.ready().cloned().unwrap_or_default(),
            None => self.list.ready().map(|l| l.iter().take(20).cloned().collect()).unwrap_or_default(),
        }
    }

    pub(crate) fn search_step(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.search_rows().len();
        if let Some(s) = self.search.as_mut().filter(|_| n > 0) {
            s.selected = if down { (s.selected + 1) % n } else { (s.selected + n - 1) % n };
            cx.notify();
        }
    }

    fn search_open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.search.as_ref().map_or(0, |s| s.selected);
        if let Some(m) = self.search_rows().get(at).cloned() {
            self.search_open(&m, window, cx);
        }
    }

    fn search_open(&mut self, m: &MergeRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.close_search(window, cx);
        self.open_mr(m.project_id, m.iid, cx);
    }

    pub(crate) fn search_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let s = self.search.as_ref()?;
        let rows = self.search_rows();
        let now = time::now();
        let (heading, status): (&str, Option<String>) = match &s.found {
            None if rows.is_empty() => ("", Some("Type to search merge requests.".into())),
            None => ("Merge requests in this list", None),
            Some((_, Load::Failed(e))) => ("", Some(e.clone())),
            Some((_, Load::Ready(m))) if m.is_empty() => ("", Some("No merge requests found.".into())),
            Some((_, Load::Ready(_))) => ("Merge requests", None),
            Some(_) => ("", Some("Searching…".into())),
        };
        let items: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let on = i == s.selected;
                let m2 = m.clone();
                let (icon_color, state) = match m.state.as_str() {
                    "merged" => (0x64a7e9, "Merged"),
                    "closed" => (0xfc7988, "Closed"),
                    _ => (GREEN, "Open"),
                };
                div()
                    .id(SharedString::from(format!("search-{}", m.id)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .mx_1()
                    .px_3()
                    .py(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(c(RAISED)))
                    .hover(|st| st.bg(c(RAISED)))
                    .child(icon("merge", 16., c(icon_color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_color(c(TEXT)).child(m.title.clone()))
                            .child(div().truncate().text_size(px(12.)).line_height(px(18.)).text_color(c(MUTED)).child(format!(
                                "{} · {} · {} · updated {}",
                                m.references.full,
                                state,
                                m.author.name,
                                time::ago(&m.updated_at, now)
                            ))),
                    )
                    .child(self.avatars.of(&m.author, 20.))
                    .on_click(cx.listener(move |v, _, window, cx| v.search_open(&m2, window, cx)))
                    .into_any_element()
            })
            .collect();
        Some(
            deferred(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .justify_center()
                    .bg(ca(0x00000066))
                    .child(
                        div()
                            .id("gl-search")
                            .mt(px(56.))
                            .w(px(640.))
                            .max_w(relative(0.9))
                            .max_h(px(520.))
                            .flex()
                            .flex_col()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(c(BORDER))
                            .bg(c(CARD))
                            .shadow_lg()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|v, _, window, cx| v.close_search(window, cx)))
                            // ↑ ↓ and Esc are the list's, before the field moves its cursor.
                            .capture_key_down(cx.listener(|v, ev: &KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
                                "down" => {
                                    v.search_step(true, cx);
                                    cx.stop_propagation();
                                }
                                "up" => {
                                    v.search_step(false, cx);
                                    cx.stop_propagation();
                                }
                                "escape" => {
                                    v.close_search(window, cx);
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }))
                            .child(
                                div().p_2().border_b_1().border_color(c(RAISED)).child(
                                    Input::new(&s.input).prefix(app_icon("search", 14., c(MUTED))).appearance(false),
                                ),
                            )
                            .when(!heading.is_empty(), |d| {
                                d.child(
                                    div()
                                        .px_4()
                                        .pt_2()
                                        .pb_1()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(c(TEXT))
                                        .child(heading),
                                )
                            })
                            .children(status.map(|t| div().px_4().py_3().text_color(c(MUTED)).child(t)))
                            .child(div().id("gl-search-rows").flex_1().min_h(px(0.)).overflow_y_scroll().pb_1().children(items))
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .px_4()
                                    .py_2()
                                    .border_t_1()
                                    .border_color(c(RAISED))
                                    .text_size(px(12.))
                                    .text_color(c(MUTED))
                                    .child("↑ ↓ to pick")
                                    .child("Enter to open")
                                    .child("!12 or group/project!12 for a number")
                                    .child("Esc to close"),
                            ),
                    ),
            )
            .with_priority(2),
        )
    }
}
