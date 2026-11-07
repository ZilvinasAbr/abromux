//! The merge requests panel on the window's right (⌥⌘B): the selected
//! workspace's, yours and the ones you opened lately, straight from the
//! cache. A click opens one in a GitLab tab (the `abromux-gitlab` plugin).

use super::model::{Mr, MrKey};
use super::{Store, ago_iso, configured, remote::places, store};
use crate::{AbroMux, TITLEBAR_H, theme::*, titlebar, workspace::Surface};
use gpui::{prelude::*, *};
use std::collections::{HashMap, HashSet};

pub const PANEL_WIDTH: f32 = 300.;

const GREEN: u32 = 0x57ab5a;
const RED: u32 = 0xe5534b;
const BLUE: u32 = 0x539bf5;
const YELLOW: u32 = 0xc69026;

fn pipeline_color(status: &str) -> u32 {
    match status {
        "success" => GREEN,
        "failed" => RED,
        "running" => BLUE,
        "pending" | "created" | "preparing" | "waiting_for_resource" | "scheduled" => YELLOW,
        _ => MUTED,
    }
}

fn dot(color: u32) -> Div {
    div().size(px(8.)).flex_none().rounded_full().bg(rgb(color))
}

/// The merge request a GitLab tab shows, if it's on the cache's GitLab.
pub fn tab_mr(s: &Surface, st: &Store, cx: &App) -> Option<MrKey> {
    let Surface::Plugin(p) = s else { return None };
    let v = p.view().downcast::<abromux_gitlab::view::GitLabView>().ok()?;
    let (host, project, path, iid) = v.read(cx).showing()?;
    if abromux_gitlab::cache::host_key(&host) != st.host {
        return None;
    }
    match path.filter(|p| !p.is_empty()) {
        Some(p) => Some(MrKey::new(p, iid)),
        // Opened by id before it's loaded: known if the cache has it.
        None => st
            .mrs
            .values()
            .find(|m| m.key.iid == iid && m.project_id().is_some() && m.project_id() == project)
            .map(|m| m.key.clone()),
    }
}

impl AbroMux {
    /// `key` in a GitLab tab: the one already showing it in this workspace,
    /// else a new tab in the focused pane. A new one starts from the cache.
    pub(crate) fn open_mr(&mut self, key: MrKey, window: &mut Window, cx: &mut Context<Self>) {
        let st = store(cx).read(cx);
        let url = st.web_url(&key);
        let existing = self.workspaces[self.active]
            .root
            .panes()
            .into_iter()
            .flat_map(|p| p.tabs.iter())
            .find(|s| tab_mr(s, st, cx).as_ref() == Some(&key))
            .map(|s| s.entity_id());
        match existing {
            Some(id) => {
                self.focus_surface(id, cx);
                self.focus_active(window, cx);
            }
            None => self.open_plugin_link("gitlab", &url, window, cx),
        }
        cx.notify();
    }

    /// A merge request's link or `group/project!12`, as `open_mr` opens it;
    /// a link to another GitLab than the cache's opens as it is.
    pub(crate) fn open_mr_ref(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<MrKey> {
        let key = MrKey::parse(text)
            .ok_or_else(|| anyhow::anyhow!("not a merge request: give its URL or group/project!iid"))?;
        let elsewhere = abromux_gitlab::api::parse_mr_url(text)
            .is_some_and(|(host, ..)| abromux_gitlab::cache::host_key(&host) != store(cx).read(cx).host);
        if elsewhere {
            self.open_plugin_link("gitlab", text.trim(), window, cx);
        } else {
            self.open_mr(key.clone(), window, cx);
        }
        Ok(key)
    }

    /// Tell the cache which merge requests GitLab tabs show, and which are
    /// on screen (checked often). Called on every frame and every few
    /// seconds; it only does anything when that changed.
    pub(crate) fn sync_mr_views(&self, window: &Window, cx: &mut App) {
        let st = store(cx).read(cx);
        let mut open = HashSet::new();
        let mut shown = HashSet::new();
        for (wi, ws) in self.workspaces.iter().enumerate() {
            for p in ws.root.panes() {
                for t in &p.tabs {
                    if let Some(k) = tab_mr(t, st, cx) {
                        let active = p.active_surface().is_some_and(|a| a.entity_id() == t.entity_id());
                        if wi == self.active && active && ws.shows(p.id) {
                            shown.insert(k.clone());
                        }
                        open.insert(k);
                    }
                }
            }
        }
        let active = window.is_window_active();
        store(cx).update(cx, |s, cx| s.set_views(open, shown, active, cx));
    }

    /// Where each workspace's checkouts are, for its merge requests.
    pub(crate) fn sync_mr_workspaces(&self, cx: &mut App) {
        let map: HashMap<usize, Vec<super::remote::Place>> = self
            .workspaces
            .iter()
            .map(|ws| {
                let cwds: Vec<String> = ws
                    .root
                    .panes()
                    .iter()
                    .flat_map(|p| p.tabs.iter())
                    .filter_map(|s| s.cwd(cx))
                    .collect();
                (ws.id, places(ws.home.as_deref(), &cwds))
            })
            .collect();
        store(cx).update(cx, |s, cx| s.set_workspaces(map, cx));
    }

    pub(crate) fn render_mr_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let st = store(cx).read(cx);
        let ws_id = self.workspaces[self.active].id;
        let mut sections: Vec<AnyElement> = vec![];
        let mut listed: HashSet<MrKey> = HashSet::new();
        let configured = configured(cx);
        let section = |title: &str, mrs: Vec<&Mr>, empty: &str, sections: &mut Vec<AnyElement>, cx: &Context<Self>| {
            sections.push(section_title(title, mrs.len()).into_any_element());
            if mrs.is_empty() {
                sections.push(
                    div()
                        .px(px(12.))
                        .pb(px(8.))
                        .text_size(px(11.5))
                        .text_color(rgb(MUTED))
                        .child(empty.to_string())
                        .into_any_element(),
                );
            }
            for m in mrs {
                sections.push(mr_row(m, st.refreshing(&m.key), cx).into_any_element());
            }
        };
        if configured {
            let mut here: Vec<&Mr> = st
                .pinned
                .get(&ws_id)
                .map(|ks| ks.iter().filter_map(|k| st.mrs.get(k)).collect())
                .unwrap_or_default();
            here.sort_by(|a, b| a.key.cmp(&b.key));
            listed.extend(here.iter().map(|m| m.key.clone()));
            section(
                "This workspace",
                here,
                "No open merge requests from this workspace's branches.",
                &mut sections,
                cx,
            );
            let mine: Vec<&Mr> = st.mine().into_iter().filter(|m| !listed.contains(&m.key)).collect();
            listed.extend(mine.iter().map(|m| m.key.clone()));
            section(
                "Mine",
                mine,
                if st.lists_ok { "Nothing open." } else { "Loading…" },
                &mut sections,
                cx,
            );
            let recent: Vec<&Mr> = st
                .recent()
                .into_iter()
                .filter(|m| !listed.contains(&m.key))
                .collect();
            section("Recently opened", recent, "None yet.", &mut sections, cx);
        }
        let status: Option<(String, u32)> = if !configured {
            None
        } else if let Some(p) = st.paused() {
            Some((format!("Rate-limited; resuming in {}s", p.as_secs().max(1)), RED_TEXT))
        } else if let Some(e) = &st.error {
            Some((e.clone(), RED_TEXT))
        } else if st.slowdown() > 1.01 {
            Some((format!("Checking {:.1}× less often (rate limit)", st.slowdown()), MUTED))
        } else {
            None
        };
        div()
            .w(px(PANEL_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(rgba(SIDEBAR_BG))
            .border_l_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(TITLEBAR_H))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px(px(12.))
                    .on_mouse_down(MouseButton::Left, |ev, window, _| {
                        titlebar::drag_or_zoom(ev, window)
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Merge Requests"),
                    )
                    .child(
                        icon_button("mrs-refresh", "rotate-cw").on_click(|_, _, cx| {
                            store(cx).update(cx, |s, cx| s.refresh_all(cx));
                        }),
                    )
                    .child(
                        icon_button("mrs-close", "x").on_click(cx.listener(|this, _, _, cx| {
                            this.mr_panel = false;
                            cx.notify();
                        })),
                    ),
            )
            .when_some(status, |d, (text, color)| {
                d.child(
                    div()
                        .px(px(12.))
                        .pb(px(6.))
                        .text_size(px(11.5))
                        .text_color(rgb(color))
                        .child(text),
                )
            })
            .when(!configured, |d| {
                d.child(
                    div()
                        .p(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(
                            "Sign in to GitLab in a GitLab tab: the panel uses the same \
                             host and token. (Settings → GitLab says which host.)",
                        )
                        .child(
                            div()
                                .id("mrs-sign-in")
                                .text_color(rgb(ACCENT_BRIGHT))
                                .cursor_pointer()
                                .child("Open a GitLab Tab…")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_plugin("gitlab", window, cx)
                                })),
                        ),
                )
            })
            .child(
                div()
                    .id("mrs-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(sections),
            )
            .into_any_element()
    }
}

const RED_TEXT: u32 = RED;

fn section_title(title: &str, n: usize) -> Div {
    div()
        .px(px(12.))
        .pt(px(10.))
        .pb(px(4.))
        .flex()
        .gap(px(6.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED))
        .child(title.to_uppercase())
        .when(n > 0, |d| d.child(n.to_string()))
}

fn mr_row(m: &Mr, refreshing: bool, cx: &Context<AbroMux>) -> Stateful<Div> {
    let key = m.key.clone();
    let l = m.light.as_ref();
    let title = l
        .map(|l| l.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Loading…".into());
    let draft = l.is_some_and(|l| l.draft);
    let sub = {
        let mut parts = vec![format!("{}!{}", m.key.project_name(), m.key.iid)];
        if let Some(l) = l {
            if !l.author.is_empty() {
                parts.push(l.author.clone());
            }
            let ago = ago_iso(&l.updated_at);
            if !ago.is_empty() {
                parts.push(ago);
            }
        }
        if refreshing {
            parts.push("loading…".into());
        }
        parts.join(" · ")
    };
    let color = l
        .and_then(|l| l.pipeline.as_deref())
        .map_or(BORDER, pipeline_color);
    div()
        .id(SharedString::from(format!("mr-{}", m.key.reference())))
        .mx(px(6.))
        .px(px(6.))
        .py(px(5.))
        .rounded(px(6.))
        .flex()
        .gap(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(HOVER)))
        .on_click(cx.listener(move |this, _, window, cx| this.open_mr(key.clone(), window, cx)))
        .child(div().pt(px(5.)).child(dot(color)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(12.5))
                        .truncate()
                        .when(draft, |d| d.text_color(rgb(MUTED)))
                        .child(if draft { format!("Draft: {}", title.trim_start_matches("Draft: ")) } else { title }),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .truncate()
                        .child(sub),
                ),
        )
}
