//! What scripts and agents report about a workspace, shown under it in the
//! sidebar like cmux: status pills (`set-status`), a progress bar
//! (`set-progress`) and the newest `log` entry. Agent hooks set a pill for
//! the agent in each terminal ("Running", "Needs input").

use crate::workspace::Surface;
use crate::{
    AbroMux,
    settings::SidebarSettings,
    theme::*,
    workspace::{LogEntry, LogLevel, Progress, Status, Workspace},
};
use abromux_core::program_status::{self, Kind, Shown, State};
use abromux_core::status::KEEP_LOG;
pub use abromux_core::status::{ERROR, ICONS, RUNNING, SUCCESS, WARNING};
use gpui::{prelude::*, *};
use std::time::SystemTime;

pub fn level_color(level: LogLevel) -> u32 {
    match level {
        LogLevel::Info => pal().muted,
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

    pub(crate) fn set_progress(
        &mut self,
        wi: usize,
        progress: Option<Progress>,
        cx: &mut Context<Self>,
    ) {
        self.workspaces[wi].progress = progress;
        cx.notify();
    }

    pub(crate) fn add_log(
        &mut self,
        wi: usize,
        level: LogLevel,
        source: Option<String>,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let log = &mut self.workspaces[wi].log;
        log.push(LogEntry {
            level,
            source,
            message,
            at: SystemTime::now(),
        });
        let excess = log.len().saturating_sub(KEEP_LOG);
        log.drain(..excess);
        cx.notify();
    }

    /// The agent in terminal `surface`: `state` is its pill text, or None
    /// to remove it.
    pub(crate) fn set_agent_status(
        &mut self,
        surface: EntityId,
        label: &str,
        state: Option<(&str, u32)>,
        cx: &mut Context<Self>,
    ) {
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(surface)) else {
            return;
        };
        // "Claude 3": the surface number keeps two agents in one workspace apart.
        let key = format!(
            "{label} {}",
            self.surface_refs.get(&surface).copied().unwrap_or(0)
        );
        // Keep the pill where it is when only its state changes.
        self.workspaces[wi]
            .status
            .retain(|s| s.owner != Some(surface) || s.program || state.is_some() && s.key == key);
        match state {
            Some((value, color)) => self.set_status(
                wi,
                Status {
                    key,
                    value: value.into(),
                    icon: None,
                    color: Some(color),
                    owner: Some(surface),
                    program: false,
                },
                cx,
            ),
            None => cx.notify(),
        }
    }

    /// A program in terminal `surface` changed what it reports about itself
    /// (OSC 7501, OSC 9;4): its pill follows what it shows now, and becoming
    /// blocked, done or failed notifies, as an agent's hooks do.
    pub(crate) fn program_status_changed(
        &mut self,
        surface: EntityId,
        was: Option<Shown>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Surface::Terminal(term)) = self.find_tab(surface) else {
            return;
        };
        let (now, title) = {
            let t = term.read(cx);
            (t.program.shown(), t.shown_title().to_string())
        };
        if now != was {
            self.emit_for(
                "program.status",
                surface,
                serde_json::json!({ "status": crate::events::program_status_json(now.as_ref()) }),
            );
        }
        let Some(wi) = self.workspaces.iter().position(|w| w.contains(surface)) else {
            return;
        };
        let n = self.surface_refs.get(&surface).copied().unwrap_or(0);
        self.workspaces[wi]
            .status
            .retain(|s| !(s.owner == Some(surface) && s.program));
        let Some(now) = now else {
            cx.notify();
            return;
        };
        let r = &now.record;
        let app = r.app.as_deref().map(program_status::display);
        let (value, color) = pill_text(&now);
        self.set_status(
            wi,
            Status {
                key: format!("{} {n}", app.as_deref().unwrap_or("Program")),
                value,
                icon: None,
                color: Some(color),
                owner: Some(surface),
                program: true,
            },
            cx,
        );
        let changed = was
            .as_ref()
            .is_none_or(|w| w.id != now.id || w.record.state != r.state);
        if changed && matches!(r.state, State::Blocked | State::Done | State::Error) {
            let who = app.unwrap_or(title);
            let what = r.title.as_deref().map(program_status::display);
            let body = r
                .msg
                .as_deref()
                .map(program_status::display)
                .unwrap_or_else(|| match r.state {
                    State::Blocked => blocked_text(r.kind).into(),
                    State::Error => "Failed".into(),
                    _ => "Finished".into(),
                });
            let body = match what {
                Some(w) => format!("{w}: {body}"),
                None => body,
            };
            self.add_notification(surface, who, body, false, window, cx);
        }
    }

    /// Pills, progress and the newest log line for the sidebar row.
    pub(crate) fn render_ws_status(
        &self,
        ws: &Workspace,
        active: bool,
        sb: &SidebarSettings,
    ) -> Option<AnyElement> {
        let z = sb.font_size / 12.5;
        let status = if sb.show_custom_metadata {
            &ws.status[..]
        } else {
            &[]
        };
        let progress = ws.progress.as_ref().filter(|_| sb.show_progress);
        let log = ws.log.last().filter(|_| sb.show_log);
        if status.is_empty() && progress.is_none() && log.is_none() {
            return None;
        }
        let sub = if active {
            rgba(0xffffffc0)
        } else {
            rgb(pal().muted)
        };
        let fg = if active {
            rgb(0xffffff)
        } else {
            rgb(pal().text)
        };
        let agents = status.iter().filter(|s| s.owner.is_some()).count();
        let pills = status.iter().map(|s| {
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
                .bg(if active { rgba(0x0000002e) } else { tint(0x0d) })
                .text_size(px(10.5 * z))
                .map(|d| match s.icon.as_deref().filter(|i| ICONS.contains(i)) {
                    Some(i) => d.child(
                        icon(i)
                            .size(px(10. * z))
                            .text_color(s.color.map_or(sub, rgb)),
                    ),
                    None => d.child(
                        div()
                            .size(px(6.))
                            .flex_none()
                            .rounded_full()
                            .bg(s.color.map_or(sub, rgb)),
                    ),
                })
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_color(sub)
                        .child(key),
                )
                .child(
                    div()
                        .flex_none()
                        .max_w(px(150.))
                        .truncate()
                        .text_color(fg)
                        .child(s.value.clone()),
                )
        });
        let progress = progress.map(|p| {
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
                        .text_size(px(10.5 * z))
                        .text_color(sub)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(p.label.clone().unwrap_or_default()),
                        )
                        .child(div().flex_none().child(format!("{pct}%"))),
                )
                .child(
                    div()
                        .h(px(3.))
                        .w_full()
                        .rounded_full()
                        .bg(if active { rgba(0x00000040) } else { tint(0x1a) })
                        .child(
                            div()
                                .h_full()
                                .w(relative(p.value.clamp(0., 1.)))
                                .rounded_full()
                                .bg(if active { rgb(0xffffff) } else { rgb(RUNNING) }),
                        ),
                )
        });
        let log = log.map(|l| {
            let color = if active {
                sub
            } else {
                rgb(level_color(l.level))
            };
            let text = match &l.source {
                Some(s) => format!("[{s}] {}", l.message),
                None => l.message.clone(),
            };
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(px(10.5 * z))
                .child(div().size(px(6.)).flex_none().rounded_full().bg(color))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(if l.level == LogLevel::Info {
                            sub
                        } else {
                            color
                        })
                        .child(text),
                )
        });
        Some(
            div()
                .mt(px(4.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .when(!status.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(3.))
                            .children(pills),
                    )
                })
                .children(progress)
                .children(log)
                .into_any_element(),
        )
    }
}

pub use abromux_core::status::{log_json, status_json};

fn blocked_text(kind: Option<Kind>) -> &'static str {
    match kind {
        Some(Kind::Permission) => "Needs approval",
        Some(Kind::Question) => "Needs an answer",
        Some(Kind::Auth) => "Needs a sign-in",
        None => "Needs input",
    }
}

/// The pill for a program's status: "Working 40%", "Needs approval",
/// "Done", "Failed", after the record's title when it has one.
fn pill_text(shown: &Shown) -> (String, u32) {
    let r = &shown.record;
    let (text, color) = match r.state {
        State::Working => (
            match r.progress {
                Some(p) => format!("Working {p}%"),
                None => "Working".into(),
            },
            RUNNING,
        ),
        State::Blocked => (blocked_text(r.kind).into(), WARNING),
        State::Done => ("Done".into(), SUCCESS),
        State::Error => ("Failed".into(), ERROR),
        State::Idle => ("Idle".into(), pal().muted),
    };
    let text = match r.title.as_deref() {
        Some(t) if !shown.id.is_empty() => format!("{}: {text}", program_status::display(t)),
        _ => text,
    };
    (text, color)
}
