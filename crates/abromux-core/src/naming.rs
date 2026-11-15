//! Workspace auto-naming's state (`automation.workspaceAutoNaming`): the
//! prompts a workspace's agent was given, and how its naming tries went.
//! The app runs the naming itself (its naming.rs).

use crate::text::one_line;

/// A title Claude Code set on its terminal, as a workspace name: None for
/// its placeholder before it has made one from the session.
pub fn agent_title(shown: &str) -> Option<String> {
    let t = one_line(shown.trim());
    (!t.is_empty() && t != "Claude Code").then_some(t)
}

/// Auto-naming's state for one workspace (the app's `AbroMux::auto_name`).
#[derive(Default)]
pub struct Naming {
    /// Its agents' prompts, oldest first, cut to the latest
    /// `PROMPT_CHARS` or so.
    pub prompts: Vec<String>,
    /// An agent CLI is making a title now.
    pub running: bool,
    /// More prompts came while it was.
    pub again: bool,
    /// It has named the workspace (so its name isn't the made-up one).
    pub titled: bool,
    /// Tries in a row that failed, and when the last one did.
    pub failures: u32,
    pub failed_at: Option<std::time::Instant>,
}

/// After this many failed tries in a row, the next one waits `RETRY_AFTER`.
pub const TRIES: u32 = 3;
pub const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(600);

impl Naming {
    /// Its tries keep failing: no more for a while (a CLI that isn't
    /// installed isn't run on every prompt).
    pub fn gave_up(&self) -> bool {
        self.failures >= TRIES && self.failed_at.is_some_and(|t| t.elapsed() < RETRY_AFTER)
    }

    /// Keep `prompt`, dropping the oldest past `PROMPT_CHARS` or so.
    pub fn add(&mut self, prompt: String) {
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

    #[test]
    fn agent_titles() {
        assert_eq!(
            agent_title("Abromux workspace name").as_deref(),
            Some("Abromux workspace name")
        );
        assert_eq!(agent_title("  Claude Code "), None);
        assert_eq!(agent_title(""), None);
    }
}
