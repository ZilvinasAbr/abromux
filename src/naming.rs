//! Workspace auto-naming (`automation.workspaceAutoNaming`): a workspace
//! still on its made-up name is named from what its agent is asked to do,
//! by that agent's own CLI (`agent::title_for`), and renamed as the work
//! moves on, until you name it yourself.

use crate::{AbroMux, agent::Kind, one_line};
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
        let Some(ws) = self.workspaces.iter().find(|w| w.contains(surface)) else {
            return;
        };
        let on = cx
            .global::<crate::settings::Settings>()
            .automation
            .workspace_auto_naming;
        if ws.custom_name.is_some() || !(on || ws.home.is_some()) {
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

/// Auto-naming's state for one workspace ([`AbroMux::auto_name`]).
#[derive(Default)]
pub struct Naming {
    /// Its agents' prompts, oldest first, cut to the latest
    /// `PROMPT_CHARS` or so.
    prompts: Vec<String>,
    /// An agent CLI is making a title now.
    running: bool,
    /// More prompts came while it was.
    again: bool,
    /// It has named the workspace (so its name isn't the made-up one).
    titled: bool,
    /// Tries in a row that failed, and when the last one did.
    failures: u32,
    failed_at: Option<std::time::Instant>,
}

/// After this many failed tries in a row, the next one waits `RETRY_AFTER`.
const TRIES: u32 = 3;
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(600);

impl Naming {
    /// Its tries keep failing: no more for a while (a CLI that isn't
    /// installed isn't run on every prompt).
    fn gave_up(&self) -> bool {
        self.failures >= TRIES && self.failed_at.is_some_and(|t| t.elapsed() < RETRY_AFTER)
    }

    fn add(&mut self, prompt: String) {
        self.prompts.push(prompt);
        let mut total = 0;
        let keep = self
            .prompts
            .iter()
            .rev()
            .take_while(|p| {
                total += p.chars().count();
                total <= crate::agent::PROMPT_CHARS
            })
            .count()
            .max(1);
        let drop = self.prompts.len() - keep;
        self.prompts.drain(..drop);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_keeps_the_latest_prompts() {
        let mut n = Naming::default();
        let big = "x".repeat(crate::agent::PROMPT_CHARS - 10);
        n.add("first".into());
        n.add(big.clone());
        n.add("latest".into());
        assert_eq!(n.prompts, vec![big.clone(), "latest".to_string()]);
        // One prompt is kept even when it's too long alone.
        n.add("y".repeat(crate::agent::PROMPT_CHARS + 5));
        assert_eq!(n.prompts.len(), 1);
    }

    #[test]
    fn a_failed_try_is_tried_again_a_few_times() {
        let mut n = Naming::default();
        assert!(!n.gave_up());
        n.failures = TRIES - 1;
        n.failed_at = Some(std::time::Instant::now());
        assert!(!n.gave_up(), "one failure doesn't stop the naming");
        n.failures = TRIES;
        assert!(n.gave_up());
        // Later, it gets another go.
        n.failed_at = std::time::Instant::now().checked_sub(RETRY_AFTER);
        assert!(!n.gave_up());
    }

}
