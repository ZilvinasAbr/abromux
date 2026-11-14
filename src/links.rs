//! A workspace's tracked links: its Jira tickets, GitLab merge requests and
//! any other page pinned to it. The selected workspace lists them in the
//! sidebar, under its row; ⌥⌘1…9 (or a click) shows one, in the tab already
//! showing it if there is one, else in the workspace's link pane (the one
//! its pages are in). So opening the same ticket again never makes a second
//! tab.
//!
//! Each link has a key that stays the same however it's written: `PAY-1423`
//! for a ticket (`…/browse/PAY-1423`, `…?selectedIssue=PAY-1423`),
//! `group/project!12` for a merge request (`…/-/merge_requests/12/diffs`),
//! the URL without its `#fragment` for anything else. A tab shows a link
//! when its page has the same key.
//!
//! Tickets and merge requests opened in a workspace are tracked on their
//! own; `abromux link add` (or ⌥⌘T on a tab) tracks anything. Its merge
//! requests from its branches (the merge requests panel's "This workspace")
//! are listed too. Untracking one hides it, so it isn't tracked again.

pub use abromux_core::links::*;

// ---- in the app ------------------------------------------------------------

use crate::{AbroMux, Hints, gitlab, hint_pill, settings::Settings, theme::*, workspace::Surface};
use gpui::{prelude::*, *};
use gpui_component::WindowExt as _;
use std::collections::HashSet;

/// The most links ⌥⌘1…9 reach.
pub const DIGITS: usize = 9;

/// The link tab `s` shows: its kind, key and page.
pub fn surface_link(s: &Surface, cx: &App) -> Option<(Kind, String, String)> {
    let url = match s {
        Surface::Plugin(_) => {
            let st = gitlab::store(cx).read(cx);
            let k = gitlab::panel::tab_mr(s, st, cx)?;
            return Some((Kind::Mr, k.reference(), st.web_url(&k)));
        }
        Surface::Browser(b) => b.read(cx).url.clone(),
        Surface::Terminal(t) => t.read(cx).app.as_ref()?.page_url()?.to_string(),
    };
    let (kind, key) = key_of_url(&url)?;
    Some((kind, key, url))
}

impl AbroMux {
    /// The Jira a bare key is on: the config's, else the one a tracked
    /// ticket is on.
    pub(crate) fn jira_base(&self, cx: &App) -> Option<String> {
        cx.global::<Settings>()
            .jira
            .host
            .as_deref()
            .and_then(origin)
            .or_else(|| {
                self.workspaces
                    .iter()
                    .flat_map(|w| &w.links)
                    .find(|l| l.kind == Kind::Jira)
                    .and_then(|l| origin(&l.url))
            })
    }

    pub(crate) fn parse_link(&self, text: &str, cx: &App) -> Result<Link, String> {
        let st = gitlab::store(cx).read(cx);
        let gitlab = !st.host.is_empty();
        parse(text, self.jira_base(cx).as_deref(), &|k| {
            gitlab.then(|| st.web_url(k))
        })
    }

    /// Workspace `wi`'s links as the sidebar lists them: the tracked ones,
    /// then the merge requests from its branches, with the titles their
    /// tabs or the merge request cache have now; a merge request stacked on
    /// another comes right after it.
    pub(crate) fn shown_links(&self, wi: usize, cx: &App) -> Vec<Link> {
        self.stacked_links(wi, cx)
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    }

    /// `shown_links`, each with how deep it's stacked (0: on no other).
    pub(crate) fn stacked_links(&self, wi: usize, cx: &App) -> Vec<(Link, usize)> {
        let ws = &self.workspaces[wi];
        let st = gitlab::store(cx).read(cx);
        let mut out = ws.links.clone();
        let gitlab_on = crate::plugins::enabled("gitlab", cx);
        for k in st
            .pinned
            .get(&ws.id)
            .into_iter()
            .flatten()
            .filter(|_| gitlab_on)
        {
            let key = k.reference();
            if !ws.hidden_links.contains(&key) && !out.iter().any(|l| l.key == key) {
                out.push(Link {
                    kind: Kind::Mr,
                    key,
                    url: st.web_url(k),
                    title: None,
                });
            }
        }
        let tabs: Vec<(String, String)> = ws
            .surfaces()
            .filter_map(|s| {
                let (kind, key, _) = surface_link(s, cx)?;
                Some((key.clone(), clean_title(kind, &key, &s.title(cx))?))
            })
            .collect();
        for l in &mut out {
            let cached = (l.kind == Kind::Mr)
                .then(|| MrKey::parse(&l.key))
                .flatten()
                .and_then(|k| st.mrs.get(&k))
                .and_then(|m| m.light.as_ref())
                .map(|m| m.title.clone())
                .filter(|t| !t.is_empty());
            let live = tabs
                .iter()
                .find(|(k, _)| *k == l.key)
                .map(|(_, t)| t.clone());
            if let Some(t) = cached.or(live) {
                l.title = Some(t);
            }
        }
        let branches: Vec<Option<(String, String, String)>> = out
            .iter()
            .map(|l| {
                let k = (l.kind == Kind::Mr)
                    .then(|| MrKey::parse(&l.key))
                    .flatten()?;
                let m = st.mrs.get(&k)?.light.as_ref()?;
                Some((
                    k.project.clone(),
                    m.source_branch.clone(),
                    m.target_branch.clone(),
                ))
            })
            .collect();
        let refs: Vec<Option<(&str, &str, &str)>> = branches
            .iter()
            .map(|b| {
                b.as_ref()
                    .map(|(p, s, t)| (p.as_str(), s.as_str(), t.as_str()))
            })
            .collect();
        stack_order(&refs)
            .into_iter()
            .map(|(i, depth)| (out[i].clone(), depth))
            .collect()
    }

    /// Tab `s` shows one of the links workspace `wi` lists in the sidebar.
    pub(crate) fn is_listed_link(&self, wi: usize, s: &Surface, cx: &App) -> bool {
        cx.global::<Settings>().sidebar.show_links
            && surface_link(s, cx)
                .is_some_and(|(_, key, _)| self.shown_links(wi, cx).iter().any(|l| l.key == key))
    }

    /// Keys of the links workspace `wi`'s panes show now.
    fn visible_links(&self, wi: usize, cx: &App) -> HashSet<String> {
        let ws = &self.workspaces[wi];
        ws.panes()
            .filter(|p| ws.shows(p.id))
            .filter_map(|p| p.active_surface())
            .filter_map(|s| surface_link(s, cx).map(|(_, k, _)| k))
            .collect()
    }

    /// Track `link` in workspace `wi`; one it has already keeps its place.
    pub(crate) fn track_link(&mut self, wi: usize, link: Link, cx: &mut Context<Self>) -> bool {
        let ws = &mut self.workspaces[wi];
        ws.hidden_links.retain(|k| *k != link.key);
        if ws.links.iter().any(|l| l.key == link.key) {
            return false;
        }
        ws.links.push(link);
        cx.notify();
        true
    }

    /// Untrack the link with `key`; it isn't tracked again on its own.
    pub(crate) fn untrack_link(&mut self, wi: usize, key: &str, cx: &mut Context<Self>) {
        let ws = &mut self.workspaces[wi];
        ws.links.retain(|l| l.key != key);
        if !ws.hidden_links.iter().any(|k| k == key) {
            ws.hidden_links.push(key.to_string());
        }
        cx.notify();
    }

    /// Track the tickets and merge requests the workspaces' tabs show, and
    /// note the tracked ones' titles. Runs every few seconds and after a
    /// link opens; cheap.
    pub(crate) fn sync_links(&mut self, cx: &mut Context<Self>) {
        let jira = self.jira_base(cx);
        let mut changed = false;
        for ws in &mut self.workspaces {
            let seen: Vec<(Kind, String, String, String)> = ws
                .surfaces()
                .filter_map(|s| {
                    let (kind, key, url) = surface_link(s, cx)?;
                    Some((kind, key, url, s.title(cx)))
                })
                .collect();
            for (kind, key, url, title) in seen {
                let title = clean_title(kind, &key, &title);
                if let Some(l) = ws.links.iter_mut().find(|l| l.key == key) {
                    if title.is_some() && l.title != title {
                        l.title = title;
                        changed = true;
                    }
                    continue;
                }
                if kind == Kind::Web || ws.hidden_links.contains(&key) {
                    continue;
                }
                let url = match kind {
                    Kind::Jira => origin(&url)
                        .or(jira.clone())
                        .map_or(url, |o| format!("{o}/browse/{key}")),
                    _ => url,
                };
                ws.links.push(Link {
                    kind,
                    key,
                    url,
                    title,
                });
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// The tab in workspace `wi` showing the link with `key`.
    pub(crate) fn link_tab(&self, wi: usize, key: &str, cx: &App) -> Option<(usize, EntityId)> {
        self.workspaces[wi].panes().find_map(|p| {
            p.tabs
                .iter()
                .find(|s| surface_link(s, cx).is_some_and(|(_, k, _)| k == key))
                .map(|s| (p.id, s.entity_id()))
        })
    }

    /// Bring forward the tab showing `key` in workspace `wi`, if one does.
    pub(crate) fn focus_link_tab(
        &mut self,
        wi: usize,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<EntityId> {
        let (pane, id) = self.link_tab(wi, key, cx)?;
        self.workspaces[wi].link_pane = Some(pane);
        self.focus_surface(id, cx);
        if wi == self.active {
            self.focus_active(window, cx);
        }
        cx.notify();
        Some(id)
    }

    /// A link opened from a terminal or a page: if workspace `wi` has a tab
    /// showing it, bring that forward instead of opening another, and
    /// return true. Either way a ticket or merge request is tracked.
    pub(crate) fn reuse_link_tab(
        &mut self,
        wi: usize,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((_, key)) = key_of_url(url) else {
            return false;
        };
        // Tracked as soon as it opens: its new tab may not know its page yet.
        if let Ok(link) = self.parse_link(url, cx)
            && link.kind != Kind::Web
            && !self.workspaces[wi].hidden_links.contains(&link.key)
        {
            self.track_link(wi, link, cx);
        }
        self.focus_link_tab(wi, &key, window, cx).is_some()
    }

    /// The pane workspace `wi`'s links open in: the one they last did, else
    /// the one with the most pages (not the focused one if another has
    /// some). None: there's none yet.
    fn link_pane(&mut self, wi: usize, cx: &App) -> Option<usize> {
        let ws = &self.workspaces[wi];
        if let Some(p) = ws.link_pane.filter(|id| ws.pane(*id).is_some()) {
            return Some(p);
        }
        let pages = |p: &&crate::workspace::Pane| {
            p.tabs
                .iter()
                .filter(|s| s.is_browser(cx) || s.is_plugin("gitlab"))
                .count()
        };
        let found = ws
            .panes()
            .filter(|p| pages(p) > 0)
            .max_by_key(|p| (p.id != ws.focused_pane, pages(p)))
            .map(|p| p.id);
        self.workspaces[wi].link_pane = found;
        found
    }

    /// Show `link` in workspace `wi`: its tab if one shows it, else a new
    /// tab in the link pane (a new pane right of the focused one if there's
    /// none). Returns the tab.
    pub(crate) fn show_link(
        &mut self,
        wi: usize,
        link: &Link,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<EntityId> {
        if let Some(id) = self.link_tab(wi, &link.key, cx) {
            if focus {
                return self.focus_link_tab(wi, &link.key, window, cx);
            }
            return Some(id.1);
        }
        let surface = match link.kind {
            Kind::Mr if crate::plugins::enabled("gitlab", cx) => self
                .make_plugin(
                    "gitlab",
                    &serde_json::json!({ "url": link.url }),
                    window,
                    cx,
                )
                .unwrap_or_else(|_| self.make_browser(&link.url, window, cx)),
            _ => self.make_browser(&link.url, window, cx),
        };
        let id = surface.entity_id();
        match self.link_pane(wi, cx) {
            Some(p) => self.add_tab_to(wi, p, surface, focus, window, cx),
            None => {
                let from = self.workspaces[wi].focused_pane;
                let new = self.split_pane(
                    wi,
                    from,
                    Axis::Horizontal,
                    false,
                    surface,
                    focus,
                    window,
                    cx,
                );
                self.workspaces[wi].link_pane = new;
            }
        }
        cx.notify();
        Some(id)
    }

    /// ⌥⌘N: the selected workspace's Nth link (from 0).
    pub(crate) fn show_link_at(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let wi = self.active;
        if let Some(link) = self.shown_links(wi, cx).into_iter().nth(n) {
            self.show_link(wi, &link, true, window, cx);
        }
    }

    /// ⌥⌘T: track the focused tab's page; again, untrack it.
    pub(crate) fn toggle_track_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wi = self.active;
        let Some(s) = self.ws().focused_surface().cloned() else {
            return;
        };
        let Some((kind, key, url)) = surface_link(&s, cx) else {
            window.push_notification(
                gpui_component::notification::Notification::info(
                    "This tab shows no web page to track.",
                ),
                cx,
            );
            return;
        };
        if self.workspaces[wi].links.iter().any(|l| l.key == key) {
            self.untrack_link(wi, &key, cx);
        } else {
            let title = clean_title(kind, &key, &s.title(cx));
            self.track_link(
                wi,
                Link {
                    kind,
                    key,
                    url,
                    title,
                },
                cx,
            );
            self.workspaces[wi].link_pane = self.workspaces[wi]
                .panes()
                .find(|p| p.tabs.iter().any(|t| t.entity_id() == s.entity_id()))
                .map(|p| p.id);
        }
    }

    /// Every link of workspace `wi`, each in its tab (made where missing);
    /// the first is brought forward.
    pub(crate) fn show_all_links(
        &mut self,
        wi: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<EntityId> {
        let links = self.shown_links(wi, cx);
        let ids: Vec<EntityId> = links
            .iter()
            .filter_map(|l| self.show_link(wi, l, false, window, cx))
            .collect();
        if let Some(first) = links.first() {
            self.show_link(wi, first, true, window, cx);
        }
        ids
    }

    /// The selected workspace's links, under its row in the sidebar.
    pub(crate) fn render_links(&self, wi: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let links = self.stacked_links(wi, cx);
        if links.is_empty() || !cx.global::<Settings>().sidebar.show_links {
            return None;
        }
        let z = cx.global::<Settings>().sidebar.font_size / 12.5;
        let visible = self.visible_links(wi, cx);
        let hints = self.hints == Some(Hints::Links);
        let grouped = self.workspaces[wi].group.is_some();
        let rows = links.into_iter().enumerate().map(|(i, (l, depth))| {
            let shown = visible.contains(&l.key);
            let key = l.key.clone();
            let open = l.clone();
            let tracked = self.workspaces[wi].links.iter().any(|t| t.key == l.key);
            let group: SharedString = format!("link-{i}").into();
            let fg = if shown {
                rgb(pal().text)
            } else {
                rgb(pal().muted)
            };
            div()
                .id(("link", i))
                .group(group.clone())
                .relative()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(6.))
                .h(px(22. * z))
                .rounded(px(5.))
                .cursor_pointer()
                .when(shown, |d| d.bg(rgba(pal().hover)))
                .hover(|d| d.bg(rgba(pal().hover)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    let wi = this.active;
                    this.show_link(wi, &open, true, window, cx);
                }))
                .tooltip({
                    let full: SharedString = match l.label() {
                        Some(t) => format!("{} · {t}", l.key).into(),
                        None => l.key.clone().into(),
                    };
                    move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                    }
                })
                // Stacked on the one above: in from it, with a turn.
                .when(depth > 0, |d| {
                    d.ml(px(14. * (depth - 1) as f32 * z)).child(
                        icon("corner-down-right")
                            .size(px(11. * z))
                            .text_color(rgb(pal().muted)),
                    )
                })
                .child(
                    icon(l.kind.icon())
                        .size(px(12. * z))
                        .text_color(l.kind.color().map_or(fg, rgb)),
                )
                .map(|d| match (l.kind, l.label()) {
                    // A merge request: just what it's about, without the draft
                    // marker, the ticket or its number (the tooltip has them).
                    (Kind::Mr, Some(title)) => d.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11. * z))
                            .text_color(fg)
                            .child(mr_title(title).0),
                    ),
                    _ => d
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(130. * z))
                                .truncate()
                                .text_size(px(11. * z))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .child(l.short_key()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11. * z))
                                .text_color(rgb(pal().muted))
                                .children(l.label().map(str::to_string)),
                        ),
                })
                .when(hints && i < DIGITS, |d| {
                    d.child(hint_pill(format!("⌥⌘{}", i + 1)))
                })
                .when(!hints, |d| {
                    d.child(
                        div()
                            .id(("link-untrack", i))
                            .invisible()
                            .group_hover(group.clone(), |s| s.visible())
                            .flex_none()
                            .rounded(px(4.))
                            .p(px(2.))
                            .hover(|s| s.bg(tint(0x22)))
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(if tracked {
                                    "Untrack"
                                } else {
                                    "Hide (from this workspace's branches)"
                                })
                                .build(window, cx)
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let wi = this.active;
                                this.untrack_link(wi, &key, cx);
                                cx.stop_propagation();
                            }))
                            .child(icon("x").size(px(10. * z)).text_color(rgb(pal().muted))),
                    )
                })
        });
        Some(
            div()
                .id("links")
                .mx(px(8.))
                .when(grouped, |d| d.ml(px(8. + 18.)))
                .pl(px(10.))
                .pb(px(4.))
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element(),
        )
    }
}
