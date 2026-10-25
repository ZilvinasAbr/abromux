//! What scripts and agents report about a workspace, shown under it in the
//! sidebar like cmux: status pills (`set-status`), a progress bar
//! (`set-progress`) and the newest `log` entry. Agent hooks set a pill for
//! the agent in each terminal ("Running", "Needs input").

use crate::{
    AbroMux,
    theme::*,
    workspace::{LogEntry, LogLevel, Progress, Status, Workspace},
};
use gpui::{prelude::*, *};
use std::time::SystemTime;

/// Oldest log entries are dropped past this.
const KEEP_LOG: usize = 200;

/// Icons a status may name: the ones bundled with the app.
pub const ICONS: &[&str] = &[
    "terminal", "globe", "plus", "x", "split-right", "split-down", "chevron-left", "chevron-right", "chevron-down", "chevron-up",
    "rotate-cw", "panel-left", "bell", "git-branch", "folder", "search", "circle-x", "pencil",
];

pub const RUNNING: u32 = 0x6a9fd8;
pub const SUCCESS: u32 = 0x7fb069;
pub const WARNING: u32 = 0xe0a84f;
pub const ERROR: u32 = 0xe5534b;

pub fn level_color(level: LogLevel) -> u32 {
    match level {
        LogLevel::Info => MUTED,
        LogLevel::Progress => RUNNING,
        LogLevel::Success => SUCCESS,
        LogLevel::Warning => WARNING,
        LogLevel::Error => ERROR,
    }
}

impl AbroMux {
    /// Set (or replace) pill `key` on workspace `wi`.
    pub(crate) fn set_status(&mut self, wi: usize, status: Status, cx: &mut Context<Self>) {
        let list = &mut self.workspaces[wi].status;
        match list.iter_mut().find(|s| s.key == status.key) {
            Some(s) => *s = status,
            None => list.push(status),
        }
        cx.notify();
    }

    /// Remove pill `key` from workspace `wi`; false if it wasn't there.
    pub(crate) fn clear_status(&mut self, wi: usize, key: &str, cx: &mut Context<Self>) -> bool {
        let list = &mut self.workspaces[wi].status;
        let before = list.len();
        list.retain(|s| s.key != key);
        cx.notify();
        list.len() != before
    }

    /// A closed terminal takes its agent's pill with it.
    pub(crate) fn forget_status(&mut self, surface: EntityId) {
        for ws in &mut self.workspaces {
            ws.status.retain(|s| s.owner != Some(surface));
        }
    }

    pub(crate) fn set_progress(&mut self, wi: usize, progress: Option<Progress>, cx: &mut Context<Self>) {
        self.workspaces[wi].progress = progress;
        cx.notify();
    }

    pub(crate) fn add_log(&mut self, wi: usize, level: LogLevel, source: Option<String>, message: String, cx: &mut Context<Self>) {
        let log = &mut self.workspaces[wi].log;
        log.push(LogEntry { level, source, message, at: SystemTime::now() });
        let excess = log.len().saturating_sub(KEEP_LOG);
        log.drain(..excess);
        cx.notify();
    }

    /// The agent in terminal `surface`: `state` is its pill text, or None
    /// to remove it.
    pub(crate) fn set_agent_status(&mut self, surface: EntityId, label: &str, state: Option<(&str, u32)>, cx: &mut Context<Self>) {
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(surface)) else { return };
        // "Claude 3": the surface number keeps two agents in one workspace apart.
        let key = format!("{label} {}", self.surface_refs.get(&surface).copied().unwrap_or(0));
        // Keep the pill where it is when only its state changes.
        self.workspaces[wi].status.retain(|s| s.owner != Some(surface) || state.is_some() && s.key == key);
        match state {
            Some((value, color)) => self.set_status(wi, Status { key, value: value.into(), icon: None, color: Some(color), owner: Some(surface) }, cx),
            None => cx.notify(),
        }
    }

    /// Pills, progress and the newest log line for the sidebar row.
    pub(crate) fn render_ws_status(&self, ws: &Workspace, active: bool) -> Option<AnyElement> {
        if ws.status.is_empty() && ws.progress.is_none() && ws.log.is_empty() {
            return None;
        }
        let sub = if active { rgba(0xffffffc0) } else { rgb(MUTED) };
        let fg = if active { rgb(0xffffff) } else { rgb(TEXT) };
        let agents = ws.status.iter().filter(|s| s.owner.is_some()).count();
        let pills = ws.status.iter().map(|s| {
            // An agent's key carries its surface number to keep it apart from
            // others; show it only when there's more than one.
            let key = match s.owner {
                Some(_) if agents == 1 => s.key.split(' ').next().unwrap_or_default().to_string(),
                _ => s.key.clone(),
            };
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .px(px(6.))
                .py(px(1.))
                .rounded(px(4.))
                .bg(if active { rgba(0x0000002e) } else { rgba(0xffffff0d) })
                .text_size(px(10.5))
                .map(|d| match s.icon.as_deref().filter(|i| ICONS.contains(i)) {
                    Some(i) => d.child(icon(i).size(px(10.)).text_color(s.color.map_or(sub, rgb))),
                    None => d.child(div().size(px(6.)).flex_none().rounded_full().bg(s.color.map_or(sub, rgb))),
                })
                .child(div().flex_none().whitespace_nowrap().text_color(sub).child(key))
                .child(div().flex_none().max_w(px(150.)).truncate().text_color(fg).child(s.value.clone()))
        });
        let progress = ws.progress.as_ref().map(|p| {
            let pct = (p.value.clamp(0., 1.) * 100.).round();
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(6.))
                        .text_size(px(10.5))
                        .text_color(sub)
                        .child(div().min_w_0().truncate().child(p.label.clone().unwrap_or_default()))
                        .child(div().flex_none().child(format!("{pct}%"))),
                )
                .child(
                    div()
                        .h(px(3.))
                        .w_full()
                        .rounded_full()
                        .bg(if active { rgba(0x00000040) } else { rgba(0xffffff1a) })
                        .child(div().h_full().w(relative(p.value.clamp(0., 1.))).rounded_full().bg(if active { rgb(0xffffff) } else { rgb(RUNNING) })),
                )
        });
        let log = ws.log.last().map(|l| {
            let color = if active { sub } else { rgb(level_color(l.level)) };
            let text = match &l.source {
                Some(s) => format!("[{s}] {}", l.message),
                None => l.message.clone(),
            };
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(px(10.5))
                .child(div().size(px(6.)).flex_none().rounded_full().bg(color))
                .child(div().min_w_0().truncate().text_color(if l.level == LogLevel::Info { sub } else { color }).child(text))
        });
        Some(
            div()
                .mt(px(4.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .when(!ws.status.is_empty(), |d| d.child(div().flex().flex_col().items_start().gap(px(3.)).children(pills)))
                .children(progress)
                .children(log)
                .into_any_element(),
        )
    }
}

/// JSON for `list-status` and the workspace description.
pub fn status_json(s: &Status) -> serde_json::Value {
    serde_json::json!({
        "key": s.key,
        "value": s.value,
        "icon": s.icon,
        "color": s.color.map(|c| format!("#{c:06x}")),
    })
}

pub fn log_json(l: &LogEntry, now: SystemTime) -> serde_json::Value {
    serde_json::json!({
        "level": l.level.as_str(),
        "source": l.source,
        "message": l.message,
        "age_seconds": now.duration_since(l.at).unwrap_or_default().as_secs(),
    })
}
