//! Notification hooks, as in cmux: `notifications.hooks` in config.json
//! lists shell commands that each notification passes through, in order.
//!
//! Each one gets the notification as JSON on stdin and prints it back,
//! changed or not:
//!
//! ```json
//! {
//!   "version": 1,
//!   "notification": { "id": 7, "workspaceId": "workspace:2", "surfaceId": "surface:5",
//!                     "title": "Claude Code", "subtitle": "", "body": "Needs your input" },
//!   "context": { "cwd": "/path", "hookId": "filter", "appFocused": false, "focusedPanel": false },
//!   "effects": { "record": true, "markUnread": true, "desktop": true, "sound": true }
//! }
//! ```
//!
//! `record: false` drops the notification, `markUnread: false` keeps it
//! read, `desktop: false` skips the macOS banner and `sound: false` its
//! sound. A changed title or body is what's shown. A hook that prints
//! nothing changes nothing, so a plain command (`say done`) works as a
//! hook too. If a hook fails, times out or prints something that isn't
//! JSON, the notification goes ahead as it would have without hooks.
//!
//! Hooks run off the main thread; the banner waits for them. Bells don't
//! go through hooks. Unlike cmux, only the global config file's hooks run,
//! not a project's.

use crate::settings::NotificationHook;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// cmux's default.
const TIMEOUT: f64 = 20.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effects {
    pub record: bool,
    pub mark_unread: bool,
    pub desktop: bool,
    pub sound: bool,
}

impl Default for Effects {
    fn default() -> Self {
        Effects {
            record: true,
            mark_unread: true,
            desktop: true,
            sound: true,
        }
    }
}

/// A notification as the hooks see it.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub id: usize,
    pub workspace: Option<String>,
    pub surface: Option<String>,
    pub title: String,
    pub body: String,
    pub cwd: Option<String>,
    pub app_focused: bool,
    pub focused_panel: bool,
    /// For the hook's environment, so it can call `abromux` back.
    pub socket: Option<String>,
}

/// What the hooks made of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub title: String,
    pub body: String,
    pub effects: Effects,
    /// The first hook that failed, and why. The rest are then skipped and
    /// the notification goes ahead unchanged.
    pub error: Option<String>,
}

pub fn payload(input: &Input) -> Value {
    json!({
        "version": 1,
        "notification": {
            "id": input.id,
            "workspaceId": input.workspace,
            "surfaceId": input.surface,
            "title": input.title,
            "subtitle": "",
            "body": input.body,
        },
        "context": {
            "cwd": input.cwd,
            "hookId": Value::Null,
            "appFocused": input.app_focused,
            "focusedPanel": input.focused_panel,
        },
        "effects": {
            "record": true,
            "markUnread": true,
            "desktop": true,
            "sound": true,
        },
    })
}

/// Run every enabled hook in turn. Blocks: call it off the main thread.
pub fn run(hooks: &[NotificationHook], input: &Input) -> Outcome {
    let unchanged = |error| Outcome {
        title: input.title.clone(),
        body: input.body.clone(),
        effects: Effects::default(),
        error,
    };
    let mut doc = payload(input);
    for hook in hooks.iter().filter(|h| h.enabled) {
        doc["context"]["hookId"] = json!(hook.id);
        let timeout = Duration::from_secs_f64(hook.timeout_seconds.unwrap_or(TIMEOUT));
        let out = run_one(&hook.command, &doc.to_string(), input, timeout)
            .and_then(|out| apply(doc.clone(), &out));
        match out {
            Ok(d) => doc = d,
            Err(e) => return unchanged(Some(format!("{}: {e}", hook.id))),
        }
    }
    outcome(&doc, input)
}

/// The document after a hook printed `out`: unchanged if it printed
/// nothing, else what it printed, which has to be a JSON object.
fn apply(doc: Value, out: &str) -> Result<Value, String> {
    if out.trim().is_empty() {
        return Ok(doc);
    }
    match serde_json::from_str::<Value>(out) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(_) => Err("printed JSON that isn't an object".into()),
        Err(e) => Err(format!("printed invalid JSON: {e}")),
    }
}

fn outcome(doc: &Value, input: &Input) -> Outcome {
    let n = &doc["notification"];
    let text = |key: &str, default: &str| n[key].as_str().unwrap_or(default).to_string();
    let mut title = text("title", &input.title);
    // abromux has no subtitle line; cmux's `notify` joins them the same way.
    let subtitle = text("subtitle", "");
    if !subtitle.is_empty() {
        title = if title.is_empty() {
            subtitle
        } else {
            format!("{title} — {subtitle}")
        };
    }
    let e = &doc["effects"];
    let flag = |key: &str| e[key].as_bool().unwrap_or(true);
    Outcome {
        title,
        body: text("body", &input.body),
        effects: Effects {
            record: flag("record"),
            mark_unread: flag("markUnread"),
            desktop: flag("desktop"),
            sound: flag("sound"),
        },
        error: None,
    }
}

/// `sh -c command` with `stdin` on its stdin; its stdout.
fn run_one(command: &str, stdin: &str, input: &Input, timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = input
        .cwd
        .as_deref()
        .filter(|d| std::path::Path::new(d).is_dir())
    {
        cmd.current_dir(dir);
    }
    for (key, value) in [
        ("ABROMUX_WORKSPACE_ID", &input.workspace),
        ("ABROMUX_SURFACE_ID", &input.surface),
        ("ABROMUX_SOCKET_PATH", &input.socket),
    ] {
        if let Some(v) = value {
            cmd.env(key, v);
        }
    }
    let mut child = cmd.spawn().map_err(|e| format!("can't run it: {e}"))?;
    // Feed and drain on their own threads, so a hook that writes a lot
    // before reading (or never reads) can't block us.
    let mut pipe_in = child.stdin.take().expect("piped");
    let data = stdin.to_string();
    std::thread::spawn(move || {
        let _ = pipe_in.write_all(data.as_bytes());
    });
    let drain = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = r.read_to_string(&mut s);
            s
        })
    };
    let stdout = drain(Box::new(child.stdout.take().expect("piped")));
    let stderr = drain(Box::new(child.stderr.take().expect("piped")));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", timeout.as_secs_f64()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = stdout.join().unwrap_or_default();
    if !status.success() {
        let err = stderr.join().unwrap_or_default();
        let why = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        return Err(match (status.code(), why.is_empty()) {
            (Some(c), true) => format!("exited with {c}"),
            (Some(c), false) => format!("exited with {c}: {why}"),
            (None, _) => "killed by a signal".into(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(id: &str, command: &str) -> NotificationHook {
        NotificationHook {
            id: id.into(),
            command: command.into(),
            timeout_seconds: None,
            enabled: true,
        }
    }

    fn input() -> Input {
        Input {
            id: 3,
            workspace: Some("workspace:1".into()),
            surface: Some("surface:2".into()),
            title: "Codex".into(),
            body: "Agent needs input".into(),
            ..Default::default()
        }
    }

    #[test]
    fn no_hooks_and_silent_hooks_change_nothing() {
        let plain = Outcome {
            title: "Codex".into(),
            body: "Agent needs input".into(),
            effects: Effects::default(),
            error: None,
        };
        assert_eq!(run(&[], &input()), plain);
        assert_eq!(run(&[hook("quiet", "cat >/dev/null")], &input()), plain);
        assert_eq!(run(&[hook("echo", "cat")], &input()), plain);
    }

    #[test]
    fn hooks_change_effects_and_text_in_order() {
        let out = run(
            &[
                hook("no-banner", r#"sed 's/"desktop":true/"desktop":false/'"#),
                hook("retitle", r#"sed 's/"title":"Codex"/"title":"Agent"/'"#),
                hook("sees-id", r#"grep -q '"hookId":"sees-id"' && cat"#),
            ],
            &input(),
        );
        assert_eq!(out.error, None);
        assert_eq!(out.title, "Agent");
        assert!(!out.effects.desktop);
        assert!(out.effects.record && out.effects.mark_unread && out.effects.sound);
    }

    #[test]
    fn a_failing_hook_falls_back_to_the_defaults() {
        let out = run(
            &[
                hook("mute", r#"sed 's/"sound":true/"sound":false/'"#),
                hook("broken", "echo nope; exit 3"),
            ],
            &input(),
        );
        assert_eq!(out.effects, Effects::default());
        assert_eq!(out.error.as_deref(), Some("broken: exited with 3"));
        let out = run(&[hook("junk", "echo '{not json'")], &input());
        assert!(out.error.unwrap().starts_with("junk: printed invalid JSON"));
        let mut slow = hook("slow", "sleep 5");
        slow.timeout_seconds = Some(0.2);
        assert_eq!(
            run(&[slow], &input()).error.as_deref(),
            Some("slow: timed out after 0.2s")
        );
    }

    #[test]
    fn disabled_hooks_are_skipped_and_env_is_set() {
        let mut off = hook("off", "exit 1");
        off.enabled = false;
        let env = hook(
            "env",
            r#"[ "$ABROMUX_SURFACE_ID" = surface:2 ] && sed 's/"record":true/"record":false/'"#,
        );
        let out = run(&[off, env], &input());
        assert_eq!(out.error, None);
        assert!(!out.effects.record);
    }

    #[test]
    fn a_subtitle_joins_the_title() {
        let out = run(
            &[hook(
                "sub",
                r#"sed 's/"subtitle":""/"subtitle":"Waiting"/'"#,
            )],
            &input(),
        );
        assert_eq!(out.title, "Codex — Waiting");
    }
}
