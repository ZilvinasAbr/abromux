//! Coding agents running in a terminal, so session restore can resume them.
//!
//! `abromux hooks install` adds hooks (see `Kind::events`) for Claude Code
//! to `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR/settings.json`) and
//! for Codex to `~/.codex/hooks.json` (or `$CODEX_HOME/hooks.json`). Each
//! runs `abromux hooks claude-event` or `codex-event`, which forwards the
//! event to the app as `agent.report` for the terminal it runs in
//! (`$ABROMUX_SURFACE_ID`). Outside abromux the hook is a no-op.
//!
//! The app keeps the session id, so on restore a terminal that had an agent
//! running types `claude --resume <id>` or `codex resume <id>` into its new
//! shell. It also shows the agent's state in the sidebar ("Running", "Needs
//! input") and notifies when the agent asks for something or finishes a turn
//! (with the start of its last reply).
//!
//! Codex has no SessionEnd hook, so its hooks also pass the Codex process id
//! (`$PPID` of the hook's shell): an agent whose process is gone isn't saved.

use crate::ipc;
use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{io::Read, path::PathBuf, time::Duration};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Agent {
    /// "claude" or "codex".
    pub kind: String,
    pub session_id: String,
    /// Where the agent was started; `--resume` only finds sessions from there.
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// The agent's process, when its hooks say (Codex's do).
    #[serde(skip)]
    pub pid: Option<u32>,
}

/// The coding agents abromux has hooks for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Claude,
    Codex,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Claude, Kind::Codex];

    pub fn parse(s: &str) -> Option<Self> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
        }
    }

    /// Its sidebar pill.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Claude => "Claude",
            Kind::Codex => "Codex",
        }
    }

    /// Its notifications' title.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Claude => "Claude Code",
            Kind::Codex => "Codex",
        }
    }

    /// Hook events: all (re)assert the session but SessionEnd, which ends
    /// it. PostToolUse marks the agent running again after a permission
    /// prompt (Claude's Notification, Codex's PermissionRequest).
    fn events(self) -> &'static [&'static str] {
        match self {
            Kind::Claude => &["SessionStart", "UserPromptSubmit", "PostToolUse", "Notification", "Stop", "SessionEnd"],
            Kind::Codex => &["SessionStart", "UserPromptSubmit", "PostToolUse", "PermissionRequest", "Stop"],
        }
    }

    fn subcommand(self) -> &'static str {
        match self {
            Kind::Claude => "claude-event",
            Kind::Codex => "codex-event",
        }
    }

    /// The file our hooks go in, or None if the agent isn't set up here.
    fn settings_path(self) -> anyhow::Result<Option<PathBuf>> {
        let home = || std::env::var("HOME").map(PathBuf::from).context("HOME isn't set");
        Ok(match self {
            Kind::Claude => Some(match std::env::var("CLAUDE_CONFIG_DIR") {
                Ok(dir) => PathBuf::from(dir).join("settings.json"),
                Err(_) => home()?.join(".claude/settings.json"),
            }),
            Kind::Codex => {
                let dir = match std::env::var("CODEX_HOME") {
                    Ok(dir) => PathBuf::from(dir),
                    Err(_) => home()?.join(".codex"),
                };
                dir.is_dir().then(|| dir.join("hooks.json"))
            }
        })
    }

    /// The hook's shell command. Codex reads a hook's stdout as JSON, so its
    /// hook always prints `{}` (and decides nothing).
    fn command(self, exe: &str) -> String {
        let exe = quote(exe);
        let sub = self.subcommand();
        match self {
            Kind::Claude => format!("[ -n \"$ABROMUX_SURFACE_ID\" ] && [ -x {exe} ] && {exe} hooks {sub} || true"),
            Kind::Codex => format!("[ -n \"$ABROMUX_SURFACE_ID\" ] && [ -x {exe} ] && ABROMUX_AGENT_PID=$PPID {exe} hooks {sub} >/dev/null 2>&1; echo '{{}}'"),
        }
    }
}

/// Longest Stop summary sent to the app.
const SUMMARY_CHARS: usize = 160;

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
        let kind = Kind::parse(s("kind")?)?;
        let session_id = s("session_id")?;
        if !safe(session_id) {
            return None;
        }
        Some(Agent {
            kind: kind.as_str().into(),
            session_id: session_id.into(),
            cwd: s("cwd").unwrap_or_default().into(),
            // Codex's modes don't map to a resume flag.
            permission_mode: s("permission_mode").filter(|m| kind == Kind::Claude && safe(m) && *m != "default").map(String::from),
            pid: p.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()),
        })
    }

    /// False once the agent's process is known to have exited.
    pub fn alive(&self) -> bool {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // Signal 0 only checks that the process exists.
        self.pid.is_none_or(|p| unsafe { kill(p as i32, 0) } == 0)
    }

    /// What to type into a fresh shell in `shell_cwd` to get this session back.
    pub fn resume_command(&self, shell_cwd: &str) -> Option<String> {
        let kind = Kind::parse(&self.kind)?;
        if !safe(&self.session_id) {
            return None;
        }
        let mut cmd = String::new();
        if !self.cwd.is_empty() && self.cwd != shell_cwd {
            cmd += &format!("cd -- {} && ", quote(&self.cwd));
        }
        match kind {
            Kind::Claude => {
                cmd += &format!("claude --resume {}", self.session_id);
                if let Some(mode) = self.permission_mode.as_deref().filter(|m| safe(m)) {
                    cmd += &format!(" --permission-mode {mode}");
                }
            }
            Kind::Codex => cmd += &format!("codex resume {}", self.session_id),
        }
        Some(cmd)
    }
}

/// `abromux hooks claude-event|codex-event`: the agent's hook payload
/// arrives on stdin. Never fails and prints nothing (Claude's SessionStart
/// output would land in the conversation), so a closed app can't get in the
/// agent's way.
pub fn hook_event(kind: Kind) -> i32 {
    let Ok(surface) = std::env::var("ABROMUX_SURFACE_ID") else { return 0 };
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&input) else { return 0 };
    let params = report(kind, &payload, &surface, std::env::var("ABROMUX_AGENT_PID").ok().and_then(|p| p.parse().ok()));
    let _ = ipc::call_timeout("agent.report", params, Duration::from_secs(2));
    0
}

/// `agent.report`'s params for a hook payload.
fn report(kind: Kind, payload: &Value, surface: &str, pid: Option<u32>) -> Value {
    let event = payload["hook_event_name"].as_str().unwrap_or_default();
    let mut params = json!({
        "surface": surface,
        "kind": kind.as_str(),
        "event": event,
        "session_id": payload["session_id"],
        "cwd": payload["cwd"],
        "permission_mode": payload["permission_mode"],
        "message": payload["message"],
        "notification_type": payload["notification_type"],
        "pid": pid,
    });
    if event == "Stop" {
        let summary = match payload["last_assistant_message"].as_str() {
            Some(text) => shorten(text),
            None => payload["transcript_path"].as_str().and_then(last_reply),
        };
        params["summary"] = json!(summary);
    }
    // Codex asks before running something: say what.
    if event == "PermissionRequest" {
        let input = &payload["tool_input"];
        // The notification's title already names the agent.
        let what = input["command"].as_str().or(input["cmd"].as_str()).map(|c| format!("Asks to run {c}"));
        let what = what.or_else(|| payload["tool_name"].as_str().map(|t| format!("Asks to use {t}")));
        params["message"] = json!(shorten(&what.unwrap_or_else(|| "Needs your approval".into())));
    }
    params
}

/// The start of Claude's last reply in a transcript (JSON lines), on one line.
fn last_reply(path: &str) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    // Only the tail: transcripts grow to megabytes.
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(256 * 1024))).ok()?;
    let mut bytes = vec![];
    f.read_to_end(&mut bytes).ok()?;
    summary(&String::from_utf8_lossy(&bytes))
}

fn summary(jsonl: &str) -> Option<String> {
    jsonl.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find_map(|v| {
        if v["type"] != "assistant" {
            return None;
        }
        let text = v["message"]["content"]
            .as_array()?
            .iter()
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join(" ");
        shorten(&text)
    })
}

/// Text on one line, cut to `SUMMARY_CHARS`. None if it's blank.
fn shorten(text: &str) -> Option<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    Some(match text.char_indices().nth(SUMMARY_CHARS) {
        Some((i, _)) => format!("{}…", text[..i].trim_end()),
        None => text,
    })
}

/// Ours, from any abromux binary (a moved or rebuilt one is replaced too).
fn is_ours(hook: &Value) -> bool {
    hook["command"].as_str().is_some_and(|c| Kind::ALL.iter().any(|k| c.contains(&format!("hooks {}", k.subcommand()))))
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

/// Append our hooks: Codex trusts each hook by its place in the list, so
/// hooks already there keep theirs.
fn add(settings: &mut Map<String, Value>, kind: Kind, exe: &str) -> anyhow::Result<()> {
    let command = kind.command(exe);
    let hooks = settings.entry("hooks").or_insert_with(|| json!({}));
    let Value::Object(hooks) = hooks else { bail!("\"hooks\" in the {} settings isn't an object", kind.label()) };
    for event in kind.events() {
        let groups = hooks.entry(*event).or_insert_with(|| json!([]));
        let Value::Array(groups) = groups else { bail!("hooks.{event} in the {} settings isn't a list", kind.label()) };
        groups.push(json!({ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }));
    }
    Ok(())
}

/// `abromux hooks install|uninstall`: edit each agent's settings in place,
/// keeping everything else. Codex is skipped when it isn't set up.
pub fn hooks(install: bool) -> anyhow::Result<String> {
    let mut out = vec![];
    for kind in Kind::ALL {
        match kind.settings_path()? {
            Some(path) => out.push(edit(kind, &path, install)?),
            None => out.push(format!("Skipped {}: no ~/.codex (or $CODEX_HOME) folder", kind.title())),
        }
    }
    if install && out.iter().any(|l| l.contains("Codex hooks")) {
        out.push("Codex asks you to review new hooks once: open `codex` and trust them (or run /hooks).".into());
    }
    Ok(out.join("\n"))
}

fn edit(kind: Kind, path: &std::path::Path, install: bool) -> anyhow::Result<String> {
    let mut settings = match std::fs::read_to_string(path) {
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
        add(&mut settings, kind, &exe.to_string_lossy())?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.abromux-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(settings))? + "\n")?;
    std::fs::rename(&tmp, path)?;
    Ok(format!("{} {} hooks in {}", if install { "Installed" } else { "Removed" }, kind.title(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_validates_ids_and_drops_default_mode() {
        let a = Agent::from_report(&json!({ "kind": "claude", "session_id": "9b77eba8-c39f", "cwd": "/p", "permission_mode": "default" })).unwrap();
        assert_eq!(a.permission_mode, None);
        assert!(Agent::from_report(&json!({ "kind": "claude", "session_id": "x; rm -rf ~" })).is_none());
        assert!(Agent::from_report(&json!({ "kind": "gemini", "session_id": "abc" })).is_none());
        let c = Agent::from_report(&json!({ "kind": "codex", "session_id": "abc", "permission_mode": "bypassPermissions", "pid": 42 })).unwrap();
        assert_eq!((c.permission_mode, c.pid), (None, Some(42)));
    }

    #[test]
    fn codex_reports() {
        // Payloads as codex-cli 0.164 sends them.
        let stop = json!({ "hook_event_name": "Stop", "session_id": "01a1", "cwd": "/p", "last_assistant_message": "Done:\n  all  green." });
        let r = report(Kind::Codex, &stop, "surface:3", Some(7));
        assert_eq!((r["kind"].as_str(), r["summary"].as_str(), r["pid"].as_u64()), (Some("codex"), Some("Done: all green."), Some(7)));
        let ask = json!({ "hook_event_name": "PermissionRequest", "session_id": "01a1", "tool_name": "Bash", "tool_input": { "command": "touch  /etc/x" } });
        assert_eq!(report(Kind::Codex, &ask, "surface:3", None)["message"], "Asks to run touch /etc/x");
        let ask = json!({ "hook_event_name": "PermissionRequest", "tool_name": "apply_patch" });
        assert_eq!(report(Kind::Codex, &ask, "surface:3", None)["message"], "Asks to use apply_patch");
    }

    #[test]
    fn alive_checks_the_process() {
        let mut a = Agent { kind: "codex".into(), session_id: "a".into(), cwd: String::new(), permission_mode: None, pid: None };
        assert!(a.alive(), "no pid: assume so");
        a.pid = Some(std::process::id());
        assert!(a.alive());
        a.pid = Some(999_999);
        assert!(!a.alive());
    }

    #[test]
    fn resume_command_cds_only_when_needed() {
        let a = Agent { kind: "claude".into(), session_id: "abc-1".into(), cwd: "/it's here".into(), permission_mode: Some("auto".into()), pid: None };
        assert_eq!(a.resume_command("/it's here").unwrap(), "claude --resume abc-1 --permission-mode auto");
        assert_eq!(a.resume_command("/tmp").unwrap(), r"cd -- '/it'\''s here' && claude --resume abc-1 --permission-mode auto");
        let c = Agent { kind: "codex".into(), session_id: "01a1-2".into(), cwd: "/p".into(), permission_mode: None, pid: None };
        assert_eq!(c.resume_command("/tmp").unwrap(), "cd -- '/p' && codex resume 01a1-2");
    }

    #[test]
    fn summary_is_the_last_reply_with_text() {
        let jsonl = [
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "old" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "All\n  tests pass." }, { "type": "tool_use" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use" }] } }),
            json!({ "type": "user", "message": { "content": "thanks" } }),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        assert_eq!(summary(&(jsonl + "\n{truncated")).as_deref(), Some("All tests pass."));
        let long = json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "é".repeat(300) }] } }).to_string();
        assert_eq!(summary(&long).unwrap().chars().count(), SUMMARY_CHARS + 1);
        assert_eq!(summary(""), None);
    }

    #[test]
    fn install_is_idempotent_and_uninstall_keeps_other_hooks() {
        let theirs = json!({ "hooks": [{ "type": "command", "command": "notify.sh" }] });
        let mut s: Map<String, Value> = serde_json::from_value(json!({ "model": "opus", "hooks": { "Stop": [theirs] } })).unwrap();
        for _ in 0..2 {
            strip(&mut s);
            add(&mut s, Kind::Claude, "/Applications/AbroMux.app/Contents/MacOS/abromux").unwrap();
        }
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(s["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        strip(&mut s);
        assert_eq!(Value::Object(s), json!({ "model": "opus", "hooks": { "Stop": [theirs] } }));
    }

    #[test]
    fn codex_hooks_go_after_existing_ones() {
        let theirs = json!({ "matcher": "Edit", "hooks": [{ "type": "command", "command": "fmt.sh", "timeout": 60 }] });
        let mut s: Map<String, Value> = serde_json::from_value(json!({ "hooks": { "PostToolUse": [theirs] } })).unwrap();
        add(&mut s, Kind::Codex, "/x/abromux").unwrap();
        let post = s["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post[0], theirs, "existing hooks keep their place (and Codex's trust)");
        let cmd = post[1]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, r#"[ -n "$ABROMUX_SURFACE_ID" ] && [ -x '/x/abromux' ] && ABROMUX_AGENT_PID=$PPID '/x/abromux' hooks codex-event >/dev/null 2>&1; echo '{}'"#);
        assert!(s["hooks"].get("PermissionRequest").is_some() && s["hooks"].get("SessionEnd").is_none());
        strip(&mut s);
        assert_eq!(Value::Object(s), json!({ "hooks": { "PostToolUse": [theirs] } }));
    }
}
