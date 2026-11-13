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

use crate::gitlab::model::MrKey;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Jira,
    Mr,
    Web,
}

impl Kind {
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Jira => "jira",
            Kind::Mr => "gitlab",
            Kind::Web => "globe",
        }
    }

    /// Its icon's color: the brand's (Jira blue, GitLab orange).
    pub fn color(self) -> Option<u32> {
        match self {
            Kind::Jira => Some(0x2684ff),
            Kind::Mr => Some(0xfc6d26),
            Kind::Web => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Jira => "jira",
            Kind::Mr => "mr",
            Kind::Web => "web",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Link {
    pub kind: Kind,
    pub key: String,
    pub url: String,
    /// The page's title, as last seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Link {
    /// What the sidebar shows after the key.
    pub fn label(&self) -> Option<&str> {
        self.title
            .as_deref()
            .filter(|t| !t.is_empty() && *t != self.key)
    }

    /// The key as the sidebar shows it: a merge request by its project's
    /// last part (`api!812`), a page by its host and path.
    pub fn short_key(&self) -> String {
        match self.kind {
            Kind::Jira => self.key.clone(),
            Kind::Mr => MrKey::parse(&self.key)
                .map(|k| format!("{}!{}", k.project_name(), k.iid))
                .unwrap_or_else(|| self.key.clone()),
            Kind::Web => {
                let s = self.key.split_once("://").map_or(&*self.key, |(_, r)| r);
                s.trim_end_matches('/').to_string()
            }
        }
    }
}

pub use abromux_plugin::is_jira_key;

fn is_web(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// The ticket a Jira URL shows: `/browse/KEY`, or `selectedIssue=KEY` on a
/// board.
fn jira_key_in_url(url: &str) -> Option<String> {
    let end = |s: &str| s.find(['/', '?', '#', '&']).unwrap_or(s.len());
    if let Some((_, rest)) = url.split_once("/browse/") {
        let k = &rest[..end(rest)];
        if is_jira_key(k) {
            return Some(k.into());
        }
    }
    if let Some((_, rest)) = url.split_once("selectedIssue=") {
        let k = &rest[..end(rest)];
        if is_jira_key(k) {
            return Some(k.into());
        }
    }
    None
}

/// `https://acme.atlassian.net` from `acme.atlassian.net` or a URL on it.
pub fn origin(s: &str) -> Option<String> {
    let s = s.trim();
    let (scheme, rest) = match s.split_once("://") {
        Some((sc, r)) => (sc, r),
        None => ("https", s),
    };
    let host = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

/// The kind and key of the page at `url`, if it's a web page.
pub fn key_of_url(url: &str) -> Option<(Kind, String)> {
    let url = url.trim();
    if !is_web(url) {
        return None;
    }
    if let Some(k) = jira_key_in_url(url) {
        return Some((Kind::Jira, k));
    }
    if let Some(k) = MrKey::parse(url) {
        return Some((Kind::Mr, k.reference()));
    }
    let page = url.split('#').next().unwrap_or(url).trim_end_matches('/');
    Some((Kind::Web, page.to_string()))
}

/// What `abromux link add` takes: a URL, `PAY-1423` (with `jira` the Jira
/// it's on), or `group/project!12` (with `mr_url` making its link).
pub fn parse(
    text: &str,
    jira: Option<&str>,
    mr_url: &dyn Fn(&MrKey) -> Option<String>,
) -> Result<Link, String> {
    let text = text.trim();
    if is_web(text) {
        let (kind, key) = key_of_url(text).ok_or_else(|| format!("not a link: {text:?}"))?;
        let url = match kind {
            // The merge request itself, not one of its pages, on the
            // GitLab the URL is on (which may not be the one set up).
            Kind::Mr => abromux_gitlab::api::parse_mr_url(text)
                .map(|(host, project, iid)| format!("{host}/{project}/-/merge_requests/{iid}"))
                .unwrap_or_else(|| text.to_string()),
            Kind::Jira => match origin(text) {
                Some(o) => format!("{o}/browse/{key}"),
                None => text.to_string(),
            },
            Kind::Web => text.to_string(),
        };
        return Ok(Link {
            kind,
            key,
            url,
            title: None,
        });
    }
    if is_jira_key(text) {
        let base = jira.ok_or_else(|| {
            format!(
                "which Jira is {text} on? Give its URL, or set \"jira\": {{\"host\": \
                 \"acme.atlassian.net\"}} in the config file"
            )
        })?;
        return Ok(Link {
            kind: Kind::Jira,
            key: text.into(),
            url: format!("{base}/browse/{text}"),
            title: None,
        });
    }
    if let Some(k) = MrKey::parse(text) {
        let url = mr_url(&k)
            .ok_or_else(|| format!("{text}: GitLab isn't set up; give the merge request's URL"))?;
        return Ok(Link {
            kind: Kind::Mr,
            key: k.reference(),
            url,
            title: None,
        });
    }
    Err(format!(
        "not a link: {text:?} (give a URL, a Jira key like PAY-1423, or group/project!12)"
    ))
}

/// A page's title without what its site adds: `[PAY-1423] Refund fails -
/// Jira` → `Refund fails`.
pub fn clean_title(kind: Kind, key: &str, title: &str) -> Option<String> {
    let mut t = title.trim();
    if kind == Kind::Jira {
        if let Some(rest) = t.strip_prefix(&format!("[{key}]")) {
            t = rest.trim_start();
        }
        if let Some((head, tail)) = t.rsplit_once(" - ")
            && tail.contains("Jira")
        {
            t = head.trim_end();
        }
    }
    if kind == Kind::Mr
        && let Some((head, tail)) = t.split_once(" (!")
        && tail.contains(')')
    {
        t = head.trim_end();
    }
    (!t.is_empty() && t != key && !is_web(t)).then(|| t.to_string())
}

/// A merge request's title as the sidebar shows it: without GitLab's draft
/// marker (`Draft:`, `[Draft]`, `WIP:`) and the ticket it starts with
/// (`PAY-1423: `, `[PAY-1423] `), which get their own places. Gives the
/// title, whether it's a draft, and the ticket.
pub fn mr_title(title: &str) -> (String, bool, Option<String>) {
    let mut t = title.trim();
    let mut draft = false;
    loop {
        let lower = t.to_ascii_lowercase();
        let marker = ["draft:", "[draft]", "(draft)", "draft ", "wip:", "[wip]"]
            .iter()
            .find(|m| lower.starts_with(**m));
        match marker {
            Some(m) => {
                draft = true;
                t = t[m.len()..].trim_start();
            }
            None => break,
        }
    }
    // The ticket: bare or in brackets, then `:`, `-`, `|` or a space.
    let (inner, bracketed) = match t.strip_prefix('[').or_else(|| t.strip_prefix('(')) {
        Some(rest) => (rest, true),
        None => (t, false),
    };
    let end = inner
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(inner.len());
    let key = &inner[..end];
    let mut ticket = None;
    if is_jira_key(key) {
        let mut rest = &inner[end..];
        let closed = !bracketed || rest.starts_with([']', ')']);
        if bracketed && closed {
            rest = &rest[1..];
        }
        let rest = rest.trim_start_matches([':', '-', '|', '–', '—', ' ']);
        // A title that is only the ticket stays as it is.
        if closed && !rest.is_empty() {
            ticket = Some(key.to_string());
            t = rest;
        }
    }
    (t.to_string(), draft, ticket)
}

/// Merge requests stacked on one another (one's target branch is
/// another's source, in the same project): `branches[i]` is link i's
/// project, source and target branch if it's a merge request. Returns the
/// links in order, each after the one it's stacked on, with its depth.
pub fn stack_order(branches: &[Option<(&str, &str, &str)>]) -> Vec<(usize, usize)> {
    let n = branches.len();
    let parent: Vec<Option<usize>> = (0..n)
        .map(|i| {
            let (project, _, target) = branches[i]?;
            (0..n).find(|&j| {
                j != i && matches!(branches[j], Some((p, source, _)) if p == project && source == target)
            })
        })
        .collect();
    fn visit(
        i: usize,
        depth: usize,
        parent: &[Option<usize>],
        seen: &mut [bool],
        out: &mut Vec<(usize, usize)>,
    ) {
        if seen[i] {
            return;
        }
        seen[i] = true;
        out.push((i, depth));
        for j in 0..parent.len() {
            if parent[j] == Some(i) {
                visit(j, depth + 1, parent, seen, out);
            }
        }
    }
    let mut seen = vec![false; n];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        if parent[i].is_none() {
            visit(i, 0, &parent, &mut seen, &mut out);
        }
    }
    // A loop (a → b → a) has no first one: start it where the list does.
    for i in 0..n {
        visit(i, 0, &parent, &mut seen, &mut out);
    }
    out
}

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
            let fg = if shown { rgb(TEXT) } else { rgb(MUTED) };
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
                .when(shown, |d| d.bg(rgba(HOVER)))
                .hover(|d| d.bg(rgba(HOVER)))
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
                            .text_color(rgb(MUTED)),
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
                                .text_color(rgb(MUTED))
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
                            .hover(|s| s.bg(rgba(0xffffff22)))
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
                            .child(icon("x").size(px(10. * z)).text_color(rgb(MUTED))),
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

#[cfg(test)]
mod tests {
    use super::{Kind, MrKey, clean_title, is_jira_key, key_of_url, origin, parse, stack_order};

    #[test]
    fn stacks_merge_requests() {
        let mr = |s: &'static str, t: &'static str| Some(("a/b", s, t));
        // 0 → main, 1 → feat-2 (on 2), 2 → feat-1 (on 0), 3 a ticket.
        let order = stack_order(&[
            mr("feat-1", "main"),
            mr("feat-3", "feat-2"),
            mr("feat-2", "feat-1"),
            None,
        ]);
        assert_eq!(order, [(0, 0), (2, 1), (1, 2), (3, 0)]);
        // Another project's branch of the same name isn't the base.
        let order = stack_order(&[Some(("x/y", "feat-1", "main")), mr("feat-2", "feat-1")]);
        assert_eq!(order, [(0, 0), (1, 0)]);
        // A loop still lists each once.
        let order = stack_order(&[mr("a", "b"), mr("b", "a")]);
        assert_eq!(order.len(), 2);
    }

    fn mr_url(k: &MrKey) -> Option<String> {
        Some(format!(
            "https://gitlab.acme.com/{}/-/merge_requests/{}",
            k.project, k.iid
        ))
    }

    #[test]
    fn jira_keys() {
        assert!(is_jira_key("PAY-1423"));
        assert!(is_jira_key("AB2_C-1"));
        assert!(!is_jira_key("P-1"));
        assert!(!is_jira_key("pay-1"));
        assert!(!is_jira_key("PAY-"));
        assert!(!is_jira_key("PAY-12a"));
    }

    #[test]
    fn keys_of_urls() {
        let k = |u| key_of_url(u).unwrap();
        assert_eq!(
            k("https://acme.atlassian.net/browse/PAY-1423"),
            (Kind::Jira, "PAY-1423".into())
        );
        assert_eq!(
            k(
                "https://acme.atlassian.net/jira/software/projects/PAY/boards/3?selectedIssue=PAY-7&x=1"
            ),
            (Kind::Jira, "PAY-7".into())
        );
        assert_eq!(
            k("https://gitlab.acme.com/acme/api/-/merge_requests/812/diffs#note_1"),
            (Kind::Mr, "acme/api!812".into())
        );
        assert_eq!(
            k("https://docs.rs/serde/#derive"),
            (Kind::Web, "https://docs.rs/serde".into())
        );
        assert_eq!(key_of_url("about:blank"), None);
    }

    #[test]
    fn parses_what_the_cli_takes() {
        let jira = Some("https://acme.atlassian.net");
        let l = parse("PAY-1423", jira, &mr_url).unwrap();
        assert_eq!(l.url, "https://acme.atlassian.net/browse/PAY-1423");
        assert!(parse("PAY-1423", None, &mr_url).is_err());
        let l = parse(
            "https://acme.atlassian.net/browse/PAY-9?focusedCommentId=1",
            None,
            &mr_url,
        )
        .unwrap();
        assert_eq!(
            (l.kind, &*l.key, &*l.url),
            (
                Kind::Jira,
                "PAY-9",
                "https://acme.atlassian.net/browse/PAY-9"
            )
        );
        let l = parse("acme/api!812", None, &mr_url).unwrap();
        assert_eq!(
            (l.kind, &*l.url),
            (
                Kind::Mr,
                "https://gitlab.acme.com/acme/api/-/merge_requests/812"
            )
        );
        let l = parse(
            "https://gitlab.acme.com/acme/api/-/merge_requests/812/diffs",
            None,
            &mr_url,
        )
        .unwrap();
        assert_eq!(
            (&*l.key, &*l.url),
            (
                "acme/api!812",
                "https://gitlab.acme.com/acme/api/-/merge_requests/812"
            )
        );
        // A merge request on another GitLab keeps its own host.
        let l = parse(
            "https://git.other.org/acme/api/-/merge_requests/5#note_1",
            None,
            &mr_url,
        )
        .unwrap();
        assert_eq!(
            (l.kind, &*l.url),
            (
                Kind::Mr,
                "https://git.other.org/acme/api/-/merge_requests/5"
            )
        );
        assert!(parse("hello", jira, &mr_url).is_err());
    }

    #[test]
    fn titles() {
        assert_eq!(
            clean_title(Kind::Jira, "PAY-1", "[PAY-1] Refund fails - Jira").as_deref(),
            Some("Refund fails")
        );
        assert_eq!(clean_title(Kind::Jira, "PAY-1", "PAY-1"), None);
        assert_eq!(
            clean_title(
                Kind::Mr,
                "a/b!3",
                "Fix rounding (!3) · Merge requests · a / b · GitLab"
            )
            .as_deref(),
            Some("Fix rounding")
        );
        assert_eq!(clean_title(Kind::Web, "x", "https://x.com"), None);
    }

    #[test]
    fn merge_request_titles() {
        let t = |s: &str| super::mr_title(s);
        assert_eq!(
            t("Draft: PAY-1423 Refund fails"),
            ("Refund fails".into(), true, Some("PAY-1423".into()))
        );
        assert_eq!(
            t("[PAY-1423] Refund fails"),
            ("Refund fails".into(), false, Some("PAY-1423".into()))
        );
        assert_eq!(
            t("PAY-1423: Refund fails"),
            ("Refund fails".into(), false, Some("PAY-1423".into()))
        );
        assert_eq!(
            t("Draft: Resolve \"PAY-1 x\""),
            ("Resolve \"PAY-1 x\"".into(), true, None)
        );
        assert_eq!(
            t("[Draft] WIP: Fix rounding"),
            ("Fix rounding".into(), true, None)
        );
        assert_eq!(
            t("PAY-1423 - Refund – fails"),
            ("Refund – fails".into(), false, Some("PAY-1423".into()))
        );
        // Not tickets, and a title that's only one.
        assert_eq!(
            t("UTF-8 everywhere"),
            ("everywhere".into(), false, Some("UTF-8".into()))
        );
        assert_eq!(t("PAY-1423"), ("PAY-1423".into(), false, None));
        assert_eq!(
            t("Drafting the plan"),
            ("Drafting the plan".into(), false, None)
        );
        assert_eq!(
            t("[PAY-1423 Refund"),
            ("[PAY-1423 Refund".into(), false, None)
        );
    }

    #[test]
    fn origins() {
        assert_eq!(
            origin("acme.atlassian.net").as_deref(),
            Some("https://acme.atlassian.net")
        );
        assert_eq!(
            origin("https://acme.atlassian.net/browse/X-1").as_deref(),
            Some("https://acme.atlassian.net")
        );
    }
}
