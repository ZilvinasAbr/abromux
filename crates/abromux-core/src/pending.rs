//! Work an agent left running past the end of its turn: background shells,
//! monitors and subagents, a scheduled wakeup, cron jobs. While any is
//! pending, the agent will be woken again by itself, so a `Stop` isn't the
//! end of the work: the tab stays "Running" and nothing notifies ("no new
//! PRs, waiting for more…"). Once it all drains, the next `Stop` notifies.
//!
//! The hook ([`background`], [`finished`]) reads it off Claude Code's
//! payloads; the app keeps a [`Pending`] per terminal and asks it what a
//! `Stop` means ([`Pending::on_stop`]), as `notifications.agentTurnComplete`
//! ([`TurnComplete`]) says.

use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Seen in a `PostToolUse`: what the tool left running, if anything, as
/// `agent.report`'s `background` param.
pub fn background(payload: &Value) -> Option<Value> {
    let tool = payload["tool_name"].as_str()?;
    let input = &payload["tool_input"];
    let response = &payload["tool_response"];
    let s = |v: &Value| v.as_str().map(String::from);
    match tool {
        "ScheduleWakeup" => Some(json!({
            "wakeup": !input["stop"].as_bool().unwrap_or(false),
            // `noop: false`: the tick found something worth telling.
            "noop": input["noop"],
        })),
        "CronCreate" => s(&response["id"])
            .or_else(|| s(&response["jobId"]))
            .map(|id| json!({ "cron": id })),
        "CronDelete" => s(&input["id"])
            .or_else(|| s(&input["jobId"]))
            .map(|id| json!({ "cron_deleted": id })),
        "TaskStop" | "KillShell" => s(&input["task_id"])
            .or_else(|| s(&input["shell_id"]))
            .map(|id| json!({ "stopped": id })),
        _ => {
            // Bash with run_in_background, Monitor, a background subagent.
            let id = ["backgroundTaskId", "taskId", "task_id"]
                .iter()
                .find_map(|k| s(&response[k]))
                .or_else(|| {
                    input["run_in_background"]
                        .as_bool()
                        .unwrap_or(false)
                        .then(|| s(&response["agentId"]))
                        .flatten()
                })?;
            Some(json!({ "started": id }))
        }
    }
}

/// The ids of background tasks a prompt reports done: Claude Code wakes the
/// agent with `<task-notification>` blocks when they finish.
pub fn finished(prompt: &str) -> Vec<String> {
    prompt
        .split("<task-notification>")
        .skip(1)
        .filter_map(|block| {
            let block = block.split("</task-notification>").next()?;
            let tag = |t: &str| {
                let (_, rest) = block.split_once(&format!("<{t}>"))?;
                rest.split_once(&format!("</{t}>")).map(|(v, _)| v.trim())
            };
            // A monitor's events come as notifications too, while it runs.
            match tag("status") {
                Some("running") => None,
                _ => tag("task-id").map(String::from),
            }
        })
        .collect()
}

/// A prompt that is only Claude Code waking the agent, not something typed.
pub fn is_wakeup_prompt(prompt: &str) -> bool {
    prompt.trim_start().starts_with("<task-notification>")
}

/// `notifications.agentTurnComplete`, as in cmux: when an agent finishing
/// a turn notifies.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TurnComplete {
    /// Once the work it left running has drained.
    #[default]
    WhenIdle,
    /// Every turn.
    Always,
    /// Never; asking for permission still does.
    Never,
}

impl TurnComplete {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnComplete::WhenIdle => "whenIdle",
            TurnComplete::Always => "always",
            TurnComplete::Never => "never",
        }
    }
}

/// What a `Stop` does to the agent's tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopOutcome {
    /// Its pill says "Done" rather than "Running": nothing will wake it.
    pub done: bool,
    pub notify: bool,
}

/// What one agent's terminal still has running.
#[derive(Default, Debug)]
pub struct Pending {
    tasks: BTreeSet<String>,
    crons: BTreeSet<String>,
    wakeup: bool,
    /// This turn scheduled its next tick saying it found something.
    found: bool,
}

impl Pending {
    /// Take in an `agent.report`.
    pub fn note(&mut self, p: &Value) {
        match p["event"].as_str() {
            Some("UserPromptSubmit") => {
                // A wakeup fires as a prompt; a typed one can't be told
                // apart, and the next tick schedules another anyway.
                self.wakeup = false;
                self.found = false;
                for id in p["finished"].as_array().into_iter().flatten() {
                    if let Some(id) = id.as_str() {
                        self.tasks.remove(id);
                    }
                }
            }
            Some("PostToolUse") => {
                let b = &p["background"];
                let s = |k: &str| b[k].as_str().map(String::from);
                if let Some(id) = s("started") {
                    self.tasks.insert(id);
                }
                if let Some(id) = s("stopped") {
                    self.tasks.remove(&id);
                }
                if let Some(id) = s("cron") {
                    self.crons.insert(id);
                }
                if let Some(id) = s("cron_deleted") {
                    self.crons.remove(&id);
                }
                if let Some(on) = b["wakeup"].as_bool() {
                    self.wakeup = on;
                    self.found |= on && b["noop"] == json!(false);
                }
            }
            Some("SessionEnd") => *self = Self::default(),
            _ => {}
        }
    }

    /// Something will wake the agent again.
    pub fn busy(&self) -> bool {
        self.wakeup || !self.tasks.is_empty() || !self.crons.is_empty()
    }

    /// What the turn that just ended means. The pill tells the truth
    /// whatever the setting; the setting decides only the notification. A
    /// tick that said it found something (`ScheduleWakeup`'s `noop: false`)
    /// notifies under `whenIdle` and `never` too: the agent asked for you.
    pub fn on_stop(&self, mode: TurnComplete) -> StopOutcome {
        let busy = self.busy();
        let found = busy && self.found;
        StopOutcome {
            done: !busy,
            notify: match mode {
                TurnComplete::Always => true,
                TurnComplete::WhenIdle => !busy || found,
                TurnComplete::Never => found,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(tool: &str, input: Value, response: Value) -> Value {
        let payload = json!({ "tool_name": tool, "tool_input": input, "tool_response": response });
        json!({ "event": "PostToolUse", "background": background(&payload) })
    }

    fn prompt(text: &str) -> Value {
        json!({ "event": "UserPromptSubmit", "finished": finished(text) })
    }

    // The payloads as Claude Code sends them (captured from a real session).
    const DONE: &str = "<task-notification>\n<task-id>bvwsx1qxe</task-id>\n\
        <tool-use-id>toolu_01</tool-use-id>\n<output-file>/tmp/x.output</output-file>\n\
        <status>completed</status>\n<summary>Background command \"x\" completed (exit code 0)</summary>\n\
        </task-notification>";

    #[test]
    fn a_background_shell_keeps_it_busy_until_its_notification() {
        let mut p = Pending::default();
        p.note(&post(
            "Bash",
            json!({ "command": "sleep 20", "run_in_background": true }),
            json!({ "stdout": "", "backgroundTaskId": "bvwsx1qxe" }),
        ));
        assert!(p.busy());
        p.note(&prompt("something typed"));
        assert!(p.busy());
        p.note(&prompt(DONE));
        assert!(!p.busy());
        assert!(is_wakeup_prompt(DONE));
        assert!(!is_wakeup_prompt("poll my PRs"));
    }

    #[test]
    fn a_plain_tool_leaves_nothing() {
        let mut p = Pending::default();
        p.note(&post(
            "Bash",
            json!({ "command": "ls" }),
            json!({ "stdout": "a" }),
        ));
        p.note(&post(
            "Read",
            json!({ "file_path": "/x" }),
            json!({ "type": "text" }),
        ));
        assert!(!p.busy());
    }

    #[test]
    fn a_running_monitor_event_isnt_its_end() {
        let running = DONE.replace("completed", "running");
        assert!(finished(&running).is_empty());
        assert_eq!(finished(&format!("{DONE}\n{DONE}")).len(), 2);
    }

    #[test]
    fn wakeups_and_what_they_found() {
        let mut p = Pending::default();
        p.note(&post(
            "ScheduleWakeup",
            json!({ "delaySeconds": 600, "noop": true }),
            json!({}),
        ));
        assert!(p.busy() && !p.found);
        p.note(&prompt("/loop check PRs"));
        assert!(!p.busy());
        p.note(&post(
            "ScheduleWakeup",
            json!({ "delaySeconds": 600, "noop": false }),
            json!({}),
        ));
        assert!(p.busy() && p.found);
        p.note(&prompt("/loop check PRs"));
        p.note(&post("ScheduleWakeup", json!({ "stop": true }), json!({})));
        assert!(!p.busy());
    }

    #[test]
    fn what_a_stop_does_under_each_setting() {
        use TurnComplete::*;
        let out = |p: &Pending, m| {
            let o = p.on_stop(m);
            (o.done, o.notify)
        };
        let idle = Pending::default();
        assert_eq!(out(&idle, WhenIdle), (true, true));
        assert_eq!(out(&idle, Always), (true, true));
        assert_eq!(out(&idle, Never), (true, false));

        let mut polling = Pending::default();
        polling.note(&post(
            "Bash",
            json!({ "run_in_background": true }),
            json!({ "backgroundTaskId": "b1" }),
        ));
        assert_eq!(out(&polling, WhenIdle), (false, false));
        assert_eq!(out(&polling, Always), (false, true));
        assert_eq!(out(&polling, Never), (false, false));

        let mut found = Pending::default();
        found.note(&post(
            "ScheduleWakeup",
            json!({ "delaySeconds": 60, "noop": false }),
            json!({}),
        ));
        assert_eq!(out(&found, WhenIdle), (false, true));
        assert_eq!(out(&found, Never), (false, true));
    }

    #[test]
    fn stopping_a_task_and_ending_the_session() {
        let mut p = Pending::default();
        p.note(&post("Monitor", json!({}), json!({ "taskId": "m1" })));
        p.note(&post("TaskStop", json!({ "task_id": "m1" }), json!({})));
        assert!(!p.busy());
        p.note(&post("Monitor", json!({}), json!({ "taskId": "m2" })));
        p.note(&json!({ "event": "SessionEnd" }));
        assert!(!p.busy());
    }
}
