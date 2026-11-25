//! A workspace's links: the Jira tickets, GitLab merge requests and pages
//! its pinned tabs (`crate::pins`) show, and the suggested ones. ⌥⌘1…9,
//! ⌘P or `abromux link open` shows one, in the tab already showing it if there is one
//! (hidden or not), else in a new pinned tab in the workspace's link pane
//! (the one its pages are in). So opening the same ticket again never makes
//! a second tab.
//!
//! Each link has a key that stays the same however it's written: `PAY-1423`
//! for a ticket (`…/browse/PAY-1423`, `…?selectedIssue=PAY-1423`),
//! `group/project!12` for a merge request (`…/-/merge_requests/12/diffs`),
//! the URL without its `#fragment` for anything else. A tab shows a link
//! when its page has the same key.
//!
//! Tabs showing tickets and merge requests are pinned on their own;
//! `abromux link add` opens anything in a hidden pinned tab, ⌥⌘T pins any
//! tab. Its merge requests from its branches (the merge requests
//! cache's "This workspace") and links tracked before tabs were pinned (`links`)
//! are suggested under them. Unpinning one, or dismissing a suggestion,
//! keeps it from coming back on its own.

pub use abromux_core::links::*;

// ---- in the app ------------------------------------------------------------

use crate::{AbroMux, gitlab, settings::Settings, workspace::Surface};
use gpui::{prelude::*, *};
use std::collections::HashSet;

/// The most links ⌥⌘1…9 reach.
pub const DIGITS: usize = 9;

/// The link tab `s` shows: its kind, key and page.
pub fn surface_link(s: &Surface, cx: &App) -> Option<(Kind, String, String)> {
    let url = match s {
        Surface::Plugin(_) => {
            let st = gitlab::store(cx).read(cx);
            let k = gitlab::views::tab_mr(s, st, cx)?;
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
            // Not the tabs themselves: a GitLab tab asks this while it draws,
            // and reading it then would re-enter it.
            .or_else(|| self.jira_seen.clone())
    }

    pub(crate) fn parse_link(&self, text: &str, cx: &App) -> Result<Link, String> {
        let st = gitlab::store(cx).read(cx);
        let gitlab = !st.host.is_empty();
        parse(text, self.jira_base(cx).as_deref(), &|k| {
            gitlab.then(|| st.web_url(k))
        })
    }

    /// What tab `s` of workspace `wi` is about: the ticket or merge request
    /// its page shows, else the one it was pinned for, else its page.
    pub(crate) fn tab_link(&self, wi: usize, s: &Surface, cx: &App) -> Option<Link> {
        let live = surface_link(s, cx);
        if let Some((kind, key, url)) = live.clone().filter(|(k, ..)| *k != Kind::Web) {
            return Some(Link {
                title: clean_title(kind, &key, &s.title(cx)),
                kind,
                key,
                url,
            });
        }
        if let Some(l) = self.workspaces[wi].pinned_for.get(&s.entity_id()) {
            return Some(l.clone());
        }
        live.map(|(kind, key, url)| Link {
            title: clean_title(kind, &key, &s.title(cx)),
            kind,
            key,
            url,
        })
    }

    /// The tab of workspace `wi` showing the link with `key`, in the layout
    /// or hidden: a pinned one first.
    pub(crate) fn link_tab(&self, wi: usize, key: &str, cx: &App) -> Option<EntityId> {
        let ws = &self.workspaces[wi];
        let shows = |s: &&Surface| {
            ws.pinned_for
                .get(&s.entity_id())
                .is_some_and(|l| l.key == key)
                || surface_link(s, cx).is_some_and(|(_, k, _)| k == key)
        };
        let mut tabs = ws.all_surfaces().filter(shows).map(|s| s.entity_id());
        let first = tabs.next()?;
        Some(
            std::iter::once(first)
                .chain(tabs)
                .find(|id| ws.is_pinned(*id))
                .unwrap_or(first),
        )
    }

    /// Workspace `wi`'s links list (`abromux link`, ⌥⌘1…9, ⌘P): its pinned
    /// tabs, then the suggested links (the merge requests from its branches,
    /// and links tracked before tabs were pinned) that no tab shows. A merge
    /// request stacked on another comes right after it, within each part.
    pub(crate) fn items(&self, wi: usize, cx: &App) -> Vec<Item> {
        let ws = &self.workspaces[wi];
        let st = gitlab::store(cx).read(cx);
        let mr_title = |key: &str| {
            MrKey::parse(key)
                .and_then(|k| st.mrs.get(&k))
                .and_then(|m| m.light.as_ref())
                .map(|m| m.title.clone())
                .filter(|t| !t.is_empty())
        };
        let pinned: Vec<Item> = ws
            .pinned_surfaces()
            .into_iter()
            .map(|(s, hidden)| {
                let link = self.tab_link(wi, &s, cx).map(|mut l| {
                    if let Some(t) = (l.kind == Kind::Mr).then(|| mr_title(&l.key)).flatten() {
                        l.title = Some(t);
                    }
                    l
                });
                Item {
                    tab: Some((s, hidden)),
                    link,
                }
            })
            .collect();
        let shown: HashSet<String> = ws
            .all_surfaces()
            .filter_map(|s| surface_link(s, cx).map(|(_, k, _)| k))
            .chain(ws.pinned_for.values().map(|l| l.key.clone()))
            .collect();
        let mut suggested: Vec<Link> = ws
            .links
            .iter()
            .filter(|l| !shown.contains(&l.key))
            .cloned()
            .collect();
        if crate::plugins::enabled("gitlab", cx) {
            for k in st.pinned.get(&ws.id).into_iter().flatten() {
                let key = k.reference();
                if !ws.hidden_links.contains(&key)
                    && !shown.contains(&key)
                    && !suggested.iter().any(|l| l.key == key)
                {
                    suggested.push(Link {
                        kind: Kind::Mr,
                        key,
                        url: st.web_url(k),
                        title: None,
                    });
                }
            }
        }
        for l in &mut suggested {
            if let Some(t) = (l.kind == Kind::Mr).then(|| mr_title(&l.key)).flatten() {
                l.title = Some(t);
            }
        }
        let suggested: Vec<Item> = suggested
            .into_iter()
            .map(|l| Item {
                tab: None,
                link: Some(l),
            })
            .collect();
        let stack = |items: Vec<Item>| -> Vec<Item> {
            let branches: Vec<Option<(String, String, String)>> = items
                .iter()
                .map(|it| {
                    let l = it.link.as_ref().filter(|l| l.kind == Kind::Mr)?;
                    let k = MrKey::parse(&l.key)?;
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
                .map(|i| items[i].clone())
                .collect()
        };
        let mut out = stack(pinned);
        out.extend(stack(suggested));
        out
    }

    /// Stop suggesting the link with `key` in workspace `wi`.
    pub(crate) fn dismiss_link(&mut self, wi: usize, key: &str, cx: &mut Context<Self>) {
        let ws = &mut self.workspaces[wi];
        ws.links.retain(|l| l.key != key);
        if !ws.hidden_links.iter().any(|k| k == key) {
            ws.hidden_links.push(key.to_string());
        }
        cx.notify();
    }

    /// Pin the tabs showing tickets and merge requests (one tab per key,
    /// and not a key unpinned before), and note legacy links' titles. Runs
    /// every few seconds and after a link opens; cheap.
    pub(crate) fn sync_links(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut jira_seen = None;
        for ws in &mut self.workspaces {
            let seen: Vec<(EntityId, Kind, String, String, String)> = ws
                .all_surfaces()
                .filter_map(|s| {
                    let (kind, key, url) = surface_link(s, cx)?;
                    Some((s.entity_id(), kind, key, s.title(cx), url))
                })
                .collect();
            let mut pinned_keys: HashSet<String> = seen
                .iter()
                .filter(|(id, ..)| ws.is_pinned(*id))
                .map(|(_, _, k, ..)| k.clone())
                .chain(ws.pinned_for.values().map(|l| l.key.clone()))
                .collect();
            if let Some(url) = ws.all_surfaces().find_map(|s| match surface_link(s, cx)? {
                (Kind::Jira, _, url) => origin(&url),
                _ => None,
            }) {
                jira_seen = Some(url);
            }
            for (id, kind, key, title, url) in seen {
                let title = clean_title(kind, &key, &title);
                if let Some(l) = ws.links.iter_mut().find(|l| l.key == key) {
                    if title.is_some() && l.title != title {
                        l.title = title.clone();
                        changed = true;
                    }
                }
                // A pinned tab showing its ticket: note its title for when
                // it shows something else.
                if let Some(l) = ws.pinned_for.get_mut(&id)
                    && l.key == key
                    && title.is_some()
                    && l.title != title
                {
                    l.title = title.clone();
                    changed = true;
                }
                if kind == Kind::Web
                    || ws.hidden_links.contains(&key)
                    || ws.is_pinned(id)
                    || pinned_keys.contains(&key)
                {
                    continue;
                }
                ws.pinned.push(id);
                ws.pinned_for.insert(
                    id,
                    Link {
                        kind,
                        key: key.clone(),
                        url,
                        title,
                    },
                );
                pinned_keys.insert(key);
                changed = true;
            }
        }
        if jira_seen.is_some() {
            self.jira_seen = jira_seen;
        }
        if changed {
            cx.notify();
        }
    }

    /// A link opened from a terminal or a page: if workspace `wi` has a tab
    /// showing it (hidden or not), bring that forward instead of opening
    /// another, and return true.
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
        let Some(id) = self.link_tab(wi, &key, cx) else {
            return false;
        };
        self.remember_link_pane(wi, id);
        self.reveal_tab(id, window, cx);
        true
    }

    fn remember_link_pane(&mut self, wi: usize, id: EntityId) {
        let ws = &mut self.workspaces[wi];
        if let Some((pane, _, _)) = ws.tab_position(id) {
            ws.link_pane = Some(pane);
        }
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

    /// Show `link` in workspace `wi`, pinned: the tab showing it if there
    /// is one (hidden or not), else a new tab in the link pane (a new pane
    /// right of the focused one if there's none). With `hidden`, a new tab
    /// starts hidden and an existing one stays where it is. Returns the tab,
    /// None if the page went to the system browser.
    pub(crate) fn show_link(
        &mut self,
        wi: usize,
        link: &Link,
        focus: bool,
        hidden: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<EntityId> {
        self.workspaces[wi].hidden_links.retain(|k| *k != link.key);
        if let Some(id) = self.link_tab(wi, &link.key, cx) {
            self.pin_tab(id, cx);
            if link.kind != Kind::Web {
                self.workspaces[wi]
                    .pinned_for
                    .entry(id)
                    .or_insert_with(|| link.clone());
            }
            if hidden {
                return Some(id);
            }
            if focus {
                self.remember_link_pane(wi, id);
                self.reveal_tab(id, window, cx);
            } else {
                self.show_hidden(id, false, window, cx);
            }
            return Some(id);
        }
        if !matches!(link.kind, Kind::Mr if crate::plugins::enabled("gitlab", cx))
            && self.open_in_system_browser(&link.url, window, cx)
        {
            return None;
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
        let ws = &mut self.workspaces[wi];
        ws.links.retain(|l| l.key != link.key);
        ws.pin(id);
        if link.kind != Kind::Web {
            ws.pinned_for.insert(id, link.clone());
        }
        if hidden {
            if let Surface::Browser(b) = &surface {
                b.update(cx, |b, cx| b.set_shown(false, cx));
            }
            let pane = self.link_pane(wi, cx);
            let ws = &mut self.workspaces[wi];
            let pane = pane.unwrap_or(ws.focused_pane);
            ws.hidden.push(crate::workspace::Hidden {
                surface,
                pane,
                index: usize::MAX,
            });
            cx.notify();
            return Some(id);
        }
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

    /// Show item `item` of workspace `wi`: its tab, or the
    /// suggested link in a new pinned one.
    pub(crate) fn open_item(
        &mut self,
        wi: usize,
        item: &Item,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<EntityId> {
        match (&item.tab, &item.link) {
            (Some((s, _)), _) => {
                let id = s.entity_id();
                if focus {
                    self.remember_link_pane(wi, id);
                    self.reveal_tab(id, window, cx);
                } else {
                    self.show_hidden(id, false, window, cx);
                }
                Some(id)
            }
            (None, Some(l)) => self.show_link(wi, l, focus, false, window, cx),
            (None, None) => None,
        }
    }

    /// ⌥⌘N: the selected workspace's Nth item (from 0).
    pub(crate) fn show_link_at(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let wi = self.active;
        if let Some(item) = self.items(wi, cx).into_iter().nth(n) {
            self.open_item(wi, &item, true, window, cx);
        }
    }

    /// Every item of workspace `wi` in the layout (suggestions opened as
    /// pinned tabs, hidden tabs put back); the first is brought forward.
    pub(crate) fn show_all_links(
        &mut self,
        wi: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<EntityId> {
        let items = self.items(wi, cx);
        let ids: Vec<EntityId> = items
            .iter()
            .filter_map(|it| self.open_item(wi, it, false, window, cx))
            .collect();
        if let Some(&first) = ids.first() {
            self.reveal_tab(first, window, cx);
        }
        ids
    }
}

/// An entry of a workspace's links list: a pinned tab, or a suggested link.
#[derive(Clone)]
pub(crate) struct Item {
    /// The pinned tab, and whether it's hidden. None for a suggestion.
    pub tab: Option<(Surface, bool)>,
    /// The ticket, merge request or page it shows.
    pub link: Option<Link>,
}
