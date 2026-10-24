//! Coding agents running in a terminal, so session restore can resume them.
//!
//! `abromux hooks install` adds Claude Code hooks (SessionStart,
//! UserPromptSubmit, Stop, SessionEnd) to `~/.claude/settings.json` (or
//! `$CLAUDE_CONFIG_DIR/settings.json`). Each runs `abromux hooks claude-event`,
//! which forwards the session id to the app as `agent.report` for the terminal
//! it runs in (`$ABROMUX_SURFACE_ID`). Outside abromux the hook is a no-op.
//! On restore, a terminal that had Claude running types `claude --resume <id>`
//! into its new shell.

use crate::ipc;
use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{io::Read, path::PathBuf, time::Duration};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Agent {
    /// Only "claude" for now.
    pub kind: String,
    pub session_id: String,
    /// Where the agent was started; `--resume` only finds sessions from there.
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
}

/// Hook events that (re)assert the session, and the one that ends it.
const EVENTS: &[&str] = &["SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"];

/// Session ids and modes go into a shell command line, so keep them to safe
/// characters.
fn safe(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Single-quote for sh.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl Agent {
    /// The agent described by a hook report, or None if it isn't usable.
    pub fn from_report(p: &Value) -> Option<Self> {
        let s = |k: &str| p.get(k).and_then(Value::as_str);
        let kind = s("kind")?;
        let session_id = s("session_id")?;
        if kind != "claude" || !safe(session_id) {
            return None;
        }
        Some(Agent {
            kind: kind.into(),
            session_id: session_id.into(),
            cwd: s("cwd").unwrap_or_default().into(),
            permission_mode: s("permission_mode").filter(|m| safe(m) && *m != "default").map(String::from),
        })
    }

    /// What to type into a fresh shell in `shell_cwd` to get this session back.
    pub fn resume_command(&self, shell_cwd: &str) -> Option<String> {
        if self.kind != "claude" || !safe(&self.session_id) {
            return None;
        }
        let mut cmd = String::new();
        if !self.cwd.is_empty() && self.cwd != shell_cwd {
            cmd += &format!("cd -- {} && ", quote(&self.cwd));
        }
        cmd += &format!("claude --resume {}", self.session_id);
        if let Some(mode) = self.permission_mode.as_deref().filter(|m| safe(m)) {
            cmd += &format!(" --permission-mode {mode}");
        }
        Some(cmd)
    }
}

/// `abromux hooks claude-event`: Claude Code's hook payload arrives on stdin.
/// Never fails and prints nothing (SessionStart output would land in the
/// conversation), so a closed app can't get in Claude's way.
pub fn claude_event() -> i32 {
    let Ok(surface) = std::env::var("ABROMUX_SURFACE_ID") else { return 0 };
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&input) else { return 0 };
    let params = json!({
        "surface": surface,
        "kind": "claude",
        "event": payload["hook_event_name"],
        "session_id": payload["session_id"],
        "cwd": payload["cwd"],
        "permission_mode": payload["permission_mode"],
    });
    let _ = ipc::call_timeout("agent.report", params, Duration::from_secs(2));
    0
}

fn claude_settings_path() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return Ok(PathBuf::from(dir).join("settings.json"));
    }
    let home = std::env::var("HOME").context("HOME isn't set")?;
    Ok(PathBuf::from(home).join(".claude/settings.json"))
}

/// Ours, from any abromux binary (a moved or rebuilt one is replaced too).
fn is_ours(hook: &Value) -> bool {
    hook["command"].as_str().is_some_and(|c| c.contains("hooks claude-event"))
}

/// Drop our hooks from `settings`, and any event list left empty.
fn strip(settings: &mut Map<String, Value>) {
    let Some(Value::Object(hooks)) = settings.get_mut("hooks") else { return };
    for groups in hooks.values_mut() {
        let Value::Array(groups) = groups else { continue };
        groups.retain_mut(|g| match g.get_mut("hooks") {
            Some(Value::Array(list)) => {
                list.retain(|h| !is_ours(h));
                !list.is_empty()
            }
            _ => true,
        });
    }
    hooks.retain(|_, g| g.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        settings.remove("hooks");
    }
}

fn add(settings: &mut Map<String, Value>, exe: &str) -> anyhow::Result<()> {
    let exe = quote(exe);
    let command = format!("[ -n \"$ABROMUX_SURFACE_ID\" ] && [ -x {exe} ] && {exe} hooks claude-event || true");
    let hooks = settings.entry("hooks").or_insert_with(|| json!({}));
    let Value::Object(hooks) = hooks else { bail!("\"hooks\" in the Claude settings isn't an object") };
    for event in EVENTS {
        let groups = hooks.entry(*event).or_insert_with(|| json!([]));
        let Value::Array(groups) = groups else { bail!("hooks.{event} in the Claude settings isn't a list") };
        groups.push(json!({ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }));
    }
    Ok(())
}

/// `abromux hooks install|uninstall`: edit the Claude Code settings in place,
/// keeping everything else.
pub fn hooks(install: bool) -> anyhow::Result<String> {
    let path = claude_settings_path()?;
    let mut settings = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text).with_context(|| format!("can't parse {}", path.display()))? {
            Value::Object(m) => m,
            _ => bail!("{} isn't a JSON object", path.display()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e).with_context(|| format!("can't read {}", path.display())),
    };
    strip(&mut settings);
    if install {
        let exe = std::env::current_exe().and_then(|e| e.canonicalize()).context("can't find the abromux binary")?;
        add(&mut settings, &exe.to_string_lossy())?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.abromux-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(settings))? + "\n")?;
    std::fs::rename(&tmp, &path)?;
    Ok(format!("{} Claude Code hooks in {}", if install { "Installed" } else { "Removed" }, path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_validates_ids_and_drops_default_mode() {
        let a = Agent::from_report(&json!({ "kind": "claude", "session_id": "9b77eba8-c39f", "cwd": "/p", "permission_mode": "default" })).unwrap();
        assert_eq!(a.permission_mode, None);
        assert!(Agent::from_report(&json!({ "kind": "claude", "session_id": "x; rm -rf ~" })).is_none());
        assert!(Agent::from_report(&json!({ "kind": "codex", "session_id": "abc" })).is_none());
    }

    #[test]
    fn resume_command_cds_only_when_needed() {
        let a = Agent { kind: "claude".into(), session_id: "abc-1".into(), cwd: "/it's here".into(), permission_mode: Some("auto".into()) };
        assert_eq!(a.resume_command("/it's here").unwrap(), "claude --resume abc-1 --permission-mode auto");
        assert_eq!(a.resume_command("/tmp").unwrap(), r"cd -- '/it'\''s here' && claude --resume abc-1 --permission-mode auto");
    }

    #[test]
    fn install_is_idempotent_and_uninstall_keeps_other_hooks() {
        let theirs = json!({ "hooks": [{ "type": "command", "command": "notify.sh" }] });
        let mut s: Map<String, Value> = serde_json::from_value(json!({ "model": "opus", "hooks": { "Stop": [theirs] } })).unwrap();
        for _ in 0..2 {
            strip(&mut s);
            add(&mut s, "/Applications/AbroMux.app/Contents/MacOS/abromux").unwrap();
        }
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(s["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        strip(&mut s);
        assert_eq!(Value::Object(s), json!({ "model": "opus", "hooks": { "Stop": [theirs] } }));
    }
}
