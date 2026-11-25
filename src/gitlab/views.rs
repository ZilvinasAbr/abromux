//! Which merge requests GitLab tabs show, and opening one in a tab.

use super::model::MrKey;
use super::{Store, places, store};
use crate::{AbroMux, workspace::Surface};
use gpui::{prelude::*, *};
use std::collections::{HashMap, HashSet};

/// The merge request a GitLab tab shows, if it's on the cache's GitLab.
pub fn tab_mr(s: &Surface, st: &Store, cx: &App) -> Option<MrKey> {
    let Surface::Plugin(p) = s else { return None };
    let v = p
        .view()
        .downcast::<abromux_gitlab::view::GitLabView>()
        .ok()?;
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
            .surfaces()
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
        let key = MrKey::parse(text).ok_or_else(|| {
            anyhow::anyhow!("not a merge request: give its URL or group/project!iid")
        })?;
        let elsewhere = abromux_gitlab::api::parse_mr_url(text).is_some_and(|(host, ..)| {
            abromux_gitlab::cache::host_key(&host) != store(cx).read(cx).host
        });
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
            for p in ws.panes() {
                for t in &p.tabs {
                    if let Some(k) = tab_mr(t, st, cx) {
                        let active = p
                            .active_surface()
                            .is_some_and(|a| a.entity_id() == t.entity_id());
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
                let cwds: Vec<String> = ws.surfaces().filter_map(|s| s.cwd(cx)).collect();
                (ws.id, places(ws.home.as_deref(), &cwds))
            })
            .collect();
        store(cx).update(cx, |s, cx| s.set_workspaces(map, cx));
    }
}
