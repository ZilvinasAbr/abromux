//! Workspace auto-naming (`automation.workspaceAutoNaming`): a workspace
//! still on its made-up name is named from what its agent is asked to do,
//! by that agent's own CLI (`agent::title_for`), and renamed as the work
//! moves on, until you name it yourself.
//!
//! Only the first agent to run in a workspace names it ([`claim_naming`]):
//! later ones, in other terminals, leave the name alone. Claude Code titles
//! its own terminal, so a Claude workspace takes that title as it changes
//! ([`name_from_title`]); other agents are asked for one by their CLI.
//!
//! [`claim_naming`]: AbroMux::claim_naming
//! [`name_from_title`]: AbroMux::name_from_title

use crate::{AbroMux, agent::Kind, one_line, workspace::Surface};
pub use abromux_core::naming::Naming;
use abromux_core::naming::agent_title;
use gpui::{AppContext as _, Context};

impl AbroMux {
    /// Name the workspace of `surface` from what its agent is asked to do,
    /// and keep renaming it as the work moves on, until you name it
    /// yourself. On with `automation.workspaceAutoNaming`, and always for a
    /// ⇧⌘I workspace still on its made-up name.
    pub(crate) fn auto_name(
        &mut self,
        surface: gpui::EntityId,
        kind: Kind,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.iter().find(|w| w.owns(surface)) else {
            return;
        };
        let on = cx
            .global::<crate::settings::Settings>()
            .automation
            .workspace_auto_naming;
        if ws.custom_name.is_some() || !(on || ws.home.is_some()) {
            return;
        }
        // Another agent names this workspace, or Claude Code titles its
        // terminal itself ([`Self::name_from_title`]).
        if !self.names_workspace(surface) || kind == Kind::Claude {
            return;
        }
        // Still on the name its folder was made with: nothing has named it yet.
        let made_up = ws
            .home
            .as_deref()
            .and_then(|h| h.file_name())
            .is_some_and(|f| ws.auto_title.as_deref() == f.to_str());
        let (id, titled) = (ws.id, ws.auto_title.is_some() && !made_up);
        let naming = self.naming.entry(id).or_default();
        // "yes", "go on": nothing new to name it from, once it has a name.
        let short = prompt.split_whitespace().count() < 4;
        if naming.gave_up() || (titled && !naming.prompts.is_empty() && short) {
            return;
        }
        naming.add(prompt);
        if naming.running {
            naming.again = true;
            return;
        }
        self.run_naming(id, kind, cx);
    }

    /// Auto-naming is on for the workspace holding `surface`, and it isn't
    /// named by hand.
    fn naming_on(&self, surface: gpui::EntityId, cx: &Context<Self>) -> Option<usize> {
        let i = self.workspaces.iter().position(|w| w.owns(surface))?;
        let on = cx
            .global::<crate::settings::Settings>()
            .automation
            .workspace_auto_naming;
        let ws = &self.workspaces[i];
        (ws.custom_name.is_none() && (on || ws.home.is_some())).then_some(i)
    }

    /// `surface` is the terminal whose agent names its workspace.
    fn names_workspace(&self, surface: gpui::EntityId) -> bool {
        let Some(ws) = self.workspaces.iter().find(|w| w.owns(surface)) else {
            return false;
        };
        ws.name_surface.is_some() && ws.name_surface == self.surface_refs.get(&surface).copied()
    }

    /// An agent reported in from terminal `surface`: if it's the first in its
    /// workspace, it names the workspace from now on.
    pub(crate) fn claim_naming(&mut self, surface: gpui::EntityId, cx: &mut Context<Self>) {
        let Some(i) = self.naming_on(surface, cx) else {
            return;
        };
        if self.workspaces[i].name_surface.is_some() {
            return;
        }
        let Some(r) = self.surface_refs.get(&surface).copied() else {
            return;
        };
        let has_agent = self.workspaces[i].surfaces().any(|s| match s {
            Surface::Terminal(t) => t.entity_id() == surface && t.read(cx).agent.is_some(),
            _ => false,
        });
        if has_agent {
            self.workspaces[i].name_surface = Some(r);
            self.name_from_title(surface, cx);
        }
    }

    /// Terminal `surface`'s title changed: if Claude Code, the workspace's
    /// naming agent, set it, it's the workspace's name.
    pub(crate) fn name_from_title(&mut self, surface: gpui::EntityId, cx: &mut Context<Self>) {
        if !self.names_workspace(surface) {
            return;
        }
        let Some(i) = self.naming_on(surface, cx) else {
            return;
        };
        let title = self.workspaces[i].surfaces().find_map(|s| match s {
            Surface::Terminal(t) if t.entity_id() == surface => {
                let t = t.read(cx);
                t.has_claude()
                    .then(|| agent_title(t.shown_title()))
                    .flatten()
            }
            _ => None,
        });
        let Some(title) = title else {
            return;
        };
        let ws = &mut self.workspaces[i];
        if ws.auto_title.as_deref() != Some(title.as_str()) {
            ws.auto_title = Some(title);
            cx.notify();
        }
    }

    /// Ask `kind`'s CLI for a title from the prompts so far; once it
    /// answers, again if more came meanwhile.
    fn run_naming(&mut self, id: usize, kind: Kind, cx: &mut Context<Self>) {
        let current = self
            .ws_index(id)
            .and_then(|i| self.workspaces[i].auto_title.clone());
        let Some(naming) = self.naming.get_mut(&id) else {
            return;
        };
        let current = current.filter(|_| naming.titled);
        naming.running = true;
        naming.again = false;
        let prompts = naming.prompts.clone();
        let task = cx.background_spawn(async move {
            crate::agent::title_for(kind, &prompts, current.as_deref())
        });
        cx.spawn(async move |this, cx| {
            let title = task.await;
            this.update(cx, |this, cx| {
                let Some(naming) = this.naming.get_mut(&id) else {
                    return; // Renamed or cleared meanwhile.
                };
                naming.running = false;
                let Some(t) = title else {
                    // A CLI that failed (a timeout, a rate limit, not signed
                    // in) is tried again on a later prompt, a few times,
                    // then only now and then.
                    naming.failures += 1;
                    naming.failed_at = Some(std::time::Instant::now());
                    return;
                };
                naming.titled = true;
                naming.failures = 0;
                let again = naming.again;
                let Some(i) = this.ws_index(id) else {
                    this.naming.remove(&id);
                    return;
                };
                if this.workspaces[i].custom_name.is_some() {
                    this.naming.remove(&id);
                    return;
                }
                this.workspaces[i].auto_title = Some(one_line(&t));
                cx.notify();
                if again {
                    this.run_naming(id, kind, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}
