//! Coding agents running in a terminal, so session restore can resume them.
//!
//! `abromux hooks install` hooks into each agent it finds set up (see
//! `Kind::ALL`): JSON hook settings for Claude Code, Codex, Gemini CLI, Droid
//! and Cursor, and a small plugin file for OpenCode and Pi. Each hook runs
//! `abromux hooks <kind>-event` with the agent's payload on stdin, which
//! turns it into Claude Code's shape (`normalize`) and forwards it to the app
//! as `agent.report` for the terminal it runs in (`$ABROMUX_SURFACE_ID`).
//! Outside abromux the hook is a no-op.
//!
//! The app keeps the session id, so on restore a terminal that had an agent
//! running types its resume command (`claude --resume <id>`, `codex resume
//! <id>`, …) into its new shell. It also shows the agent's state in the
//! sidebar ("Running", "Needs input") and notifies when the agent asks for
//! something or finishes a turn (with the start of its last reply).
//!
//! Codex has no SessionEnd hook, so its hooks also pass the Codex process id
//! (`$PPID` of the hook's shell), and the OpenCode and Pi plugins pass their
//! own: an agent whose process is gone isn't saved. Newer Codex runs hooks in
//! a shared background server instead, with the environment of whichever
//! terminal started it; the zsh integration runs `codex` with `--no-daemon`
//! so they run here (`src/shell/zsh/abromux.zsh`).

use crate::ipc;
use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Agent {
    /// `Kind::as_str`: "claude", "codex", ….
    pub kind: String,
    pub session_id: String,
    /// Where the agent was started; `--resume` only finds sessions from there.
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// The agent's process, when its hooks say (Codex's, OpenCode's, Pi's).
    #[serde(skip)]
    pub pid: Option<u32>,
}

/// The coding agents abromux has hooks for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Claude,
    Codex,
    Gemini,
    Droid,
    Cursor,
    OpenCode,
    Pi,
}

/// How an agent takes hooks.
#[derive(PartialEq)]
enum Setup {
    /// Claude Code's `{"hooks": {Event: [{"hooks": [{command}]}]}}`, which
    /// Codex, Gemini and Droid share. Droid's own `hooks.json` drops the
    /// outer `"hooks"`.
    Groups,
    /// Cursor's `{"version": 1, "hooks": {event: [{command}]}}`.
    Flat,
    /// A JavaScript file the agent loads at startup.
    Plugin,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Claude,
        Kind::Codex,
        Kind::Gemini,
        Kind::Droid,
        Kind::Cursor,
        Kind::OpenCode,
        Kind::Pi,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
            Kind::Gemini => "gemini",
            Kind::Droid => "droid",
            Kind::Cursor => "cursor",
            Kind::OpenCode => "opencode",
            Kind::Pi => "pi",
        }
    }

    /// Its sidebar pill.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Claude => "Claude",
            Kind::Codex => "Codex",
            Kind::Gemini => "Gemini",
            Kind::Droid => "Droid",
            Kind::Cursor => "Cursor",
            Kind::OpenCode => "OpenCode",
            Kind::Pi => "Pi",
        }
    }

    /// Its notifications' title.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Claude => "Claude Code",
            Kind::Gemini => "Gemini CLI",
            k => k.label(),
        }
    }

    fn setup(self) -> Setup {
        match self {
            Kind::Cursor => Setup::Flat,
            Kind::OpenCode | Kind::Pi => Setup::Plugin,
            _ => Setup::Groups,
        }
    }

    /// Hook events, in the agent's own names: all (re)assert the session but
    /// the session end, which ends it. A tool finishing marks the agent
    /// running again after a permission prompt (Claude's Notification,
    /// Codex's PermissionRequest).
    fn events(self) -> &'static [&'static str] {
        match self {
            Kind::Claude | Kind::Droid => &[
                "SessionStart",
                "UserPromptSubmit",
                "PostToolUse",
                "Notification",
                "Stop",
                "SessionEnd",
            ],
            Kind::Codex => &[
                "SessionStart",
                "UserPromptSubmit",
                "PostToolUse",
                "PermissionRequest",
                "Stop",
            ],
            Kind::Gemini => &[
                "SessionStart",
                "BeforeAgent",
                "AfterTool",
                "Notification",
                "AfterAgent",
                "SessionEnd",
            ],
            // Its permission hooks must answer allow or deny, so none here.
            Kind::Cursor => &[
                "sessionStart",
                "beforeSubmitPrompt",
                "postToolUse",
                "stop",
                "sessionEnd",
            ],
            Kind::OpenCode | Kind::Pi => &[],
        }
    }

    fn subcommand(self) -> String {
        format!("{}-event", self.as_str())
    }

    /// The agent's config folder, if it's set up here.
    fn dir(self) -> anyhow::Result<Option<PathBuf>> {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .context("HOME isn't set")?;
        let env = |var: &str| std::env::var_os(var).map(PathBuf::from);
        let dir = match self {
            // Created if missing: Claude Code is the main one.
            Kind::Claude => {
                return Ok(Some(
                    env("CLAUDE_CONFIG_DIR").unwrap_or(home.join(".claude")),
                ));
            }
            Kind::Codex => env("CODEX_HOME").unwrap_or(home.join(".codex")),
            Kind::Gemini => env("GEMINI_CLI_HOME").unwrap_or(home).join(".gemini"),
            Kind::Droid => home.join(".factory"),
            Kind::Cursor => home.join(".cursor"),
            Kind::OpenCode => env("OPENCODE_CONFIG_DIR").unwrap_or_else(|| {
                env("XDG_CONFIG_HOME")
                    .unwrap_or(home.join(".config"))
                    .join("opencode")
            }),
            Kind::Pi => env("PI_CODING_AGENT_DIR").unwrap_or(home.join(".pi/agent")),
        };
        Ok(dir.is_dir().then_some(dir))
    }

    /// The file our hooks go in, and whether its hooks sit at the top level
    /// rather than under `"hooks"`.
    fn settings_path(self, dir: &Path) -> (PathBuf, bool) {
        match self {
            Kind::Claude | Kind::Gemini => (dir.join("settings.json"), false),
            Kind::Codex | Kind::Cursor => (dir.join("hooks.json"), false),
            // Droid reads settings.json's hooks only without a hooks.json, so
            // creating one would hide hooks already there.
            Kind::Droid => match dir.join("hooks.json") {
                p if p.exists() => (p, true),
                _ => (dir.join("settings.json"), false),
            },
            Kind::OpenCode => (dir.join("plugins/abromux.js"), false),
            Kind::Pi => (dir.join("extensions/abromux.js"), false),
        }
    }

    /// The hook's shell command. Codex and Gemini read a hook's stdout as
    /// JSON, so theirs always prints `{}` (and decides nothing); Claude and
    /// Droid show what it prints.
    fn command(self, exe: &str) -> String {
        let exe = quote(exe);
        let sub = self.subcommand();
        let guard = format!("[ -n \"$ABROMUX_SURFACE_ID\" ] && [ -x {exe} ]");
        match self {
            Kind::Codex => format!(
                "{guard} && ABROMUX_AGENT_PID=$PPID {exe} hooks {sub} >/dev/null 2>&1; echo '{{}}'"
            ),
            Kind::Gemini => format!("{guard} && {exe} hooks {sub} >/dev/null 2>&1; echo '{{}}'"),
            _ => format!("{guard} && {exe} hooks {sub} || true"),
        }
    }

    /// One hook's entry. Gemini counts its timeout in milliseconds.
    fn hook(self, exe: &str) -> Value {
        let command = self.command(exe);
        match self {
            Kind::Cursor => json!({ "command": command, "timeout": 5 }),
            Kind::Gemini => json!({ "type": "command", "command": command, "timeout": 5000 }),
            _ => json!({ "type": "command", "command": command, "timeout": 5 }),
        }
    }

    fn plugin(self, exe: &str) -> String {
        let src = match self {
            Kind::OpenCode => OPENCODE_PLUGIN,
            _ => PI_EXTENSION,
        };
        let exe = serde_json::to_string(exe).expect("a string");
        format!("{PLUGIN_MARK}\n{SEND_JS}{src}").replace("EXE", &exe)
    }

    /// What to type to resume session `id`.
    fn resume(self, id: &str) -> String {
        match self {
            Kind::Claude => format!("claude --resume {id}"),
            Kind::Codex => format!("codex resume {id}"),
            Kind::Gemini => format!("gemini --resume {id}"),
            Kind::Droid => format!("droid --resume {id}"),
            Kind::Cursor => format!("cursor-agent --resume {id}"),
            Kind::OpenCode => format!("opencode --session {id}"),
            Kind::Pi => format!("pi --session {id}"),
        }
    }
}

/// The first line of our plugin files, so uninstall knows them.
const PLUGIN_MARK: &str =
    "// Installed by `abromux hooks install`; `abromux hooks uninstall` removes it.";

/// Both plugins send Claude Code-shaped events (see `normalize`) to `EXE`.
const SEND_JS: &str = r#"import { spawn } from "node:child_process";

const exe = EXE;
const send = (kind, event) => {
  try {
    const p = spawn(exe, ["hooks", `${kind}-event`], { stdio: ["pipe", "ignore", "ignore"] });
    p.on("error", () => {});
    p.stdin.on("error", () => {});
    p.stdin.end(JSON.stringify({ ...event, pid: process.pid }));
  } catch {}
};
const replyText = (content) =>
  typeof content === "string"
    ? content
    : (content ?? []).filter((c) => c?.type === "text").map((c) => c.text).join(" ");
"#;

const OPENCODE_PLUGIN: &str = r#"
// Subagent sessions have a parent; their events would take over the terminal.
export const AbromuxPlugin = async ({ directory }) => {
  if (!process.env.ABROMUX_SURFACE_ID) return {};
  const children = new Set();
  const busy = new Set();
  const assistant = new Set();
  const replies = new Map();
  const report = (event, sessionID, extra = {}) => {
    if (sessionID && !children.has(sessionID))
      send("opencode", { hook_event_name: event, session_id: sessionID, cwd: directory, ...extra });
  };
  const idle = (id) => {
    if (busy.delete(id)) report("Stop", id, { last_assistant_message: replies.get(id) });
  };
  return {
    event: async ({ event }) => {
      const p = event.properties ?? {};
      switch (event.type) {
        case "session.created":
        case "session.updated":
          if (p.info?.parentID) children.add(p.info.id);
          else if (event.type === "session.created") report("SessionStart", p.info?.id);
          break;
        case "session.status":
          if (p.status?.type === "busy" && !busy.has(p.sessionID)) {
            busy.add(p.sessionID);
            report("UserPromptSubmit", p.sessionID);
          } else if (p.status?.type === "idle") idle(p.sessionID);
          break;
        case "session.idle":
          idle(p.sessionID);
          break;
        case "permission.asked":
        case "permission.updated":
          report("PermissionRequest", p.sessionID, { message: p.title ?? p.permission });
          break;
        case "session.error":
          if (p.sessionID) replies.set(p.sessionID, p.error?.data?.message ?? "Stopped with an error");
          break;
        case "permission.replied":
          report("PostToolUse", p.sessionID);
          break;
        case "message.updated":
          if (p.info?.role === "assistant") assistant.add(p.info.id);
          break;
        case "message.part.updated":
          if (p.part?.type === "text" && assistant.has(p.part.messageID))
            replies.set(p.part.sessionID, replyText([p.part]));
          break;
      }
    },
  };
};
"#;

const PI_EXTENSION: &str = r#"
// Only interactive sessions: a `pi -p` run inside one mustn't take it over.
export default function (pi) {
  if (!process.env.ABROMUX_SURFACE_ID) return;
  let reply;
  const report = (ctx, event, extra = {}) => {
    if (!ctx.hasUI || !ctx.sessionManager.getSessionFile()) return;
    send("pi", {
      hook_event_name: event,
      session_id: ctx.sessionManager.getSessionId(),
      cwd: ctx.cwd,
      ...extra,
    });
  };
  pi.on("session_start", (_e, ctx) => report(ctx, "SessionStart"));
  pi.on("agent_start", (_e, ctx) => report(ctx, "UserPromptSubmit"));
  pi.on("agent_end", (e) => {
    const last = [...(e.messages ?? [])].reverse().find((m) => m?.role === "assistant");
    reply = last && (last.stopReason === "error" ? last.errorMessage : replyText(last.content));
  });
  pi.on("agent_settled", (e, ctx) =>
    report(ctx, "Stop", { last_assistant_message: e.aborted ? "Stopped" : reply }),
  );
  pi.on("ui_prompt_start", (e, ctx) =>
    report(ctx, "PermissionRequest", { message: e.title ?? "Needs your input" }),
  );
  pi.on("ui_prompt_end", (_e, ctx) => report(ctx, "PostToolUse"));
  pi.on("session_shutdown", (_e, ctx) => report(ctx, "SessionEnd"));
}
"#;

/// Longest Stop summary sent to the app.
const SUMMARY_CHARS: usize = 160;

/// Session ids and modes go into a shell command line, so keep them to safe
/// characters.
fn safe(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
            // Other agents' modes don't map to a resume flag.
            permission_mode: s("permission_mode")
                .filter(|m| kind == Kind::Claude && safe(m) && *m != "default")
                .map(String::from),
            pid: p
                .get("pid")
                .and_then(Value::as_u64)
                .and_then(|p| u32::try_from(p).ok()),
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
        cmd += &kind.resume(&self.session_id);
        if kind == Kind::Claude
            && let Some(mode) = self.permission_mode.as_deref().filter(|m| safe(m))
        {
            cmd += &format!(" --permission-mode {mode}");
        }
        Some(cmd)
    }
}

/// `abromux hooks <kind>-event`: the agent's hook payload arrives on stdin.
/// Never fails and prints nothing (Claude's SessionStart output would land
/// in the conversation), so a closed app can't get in the agent's way.
pub fn hook_event(kind: Kind) -> i32 {
    let Ok(surface) = std::env::var("ABROMUX_SURFACE_ID") else {
        return 0;
    };
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return 0;
    };
    let pid = std::env::var("ABROMUX_AGENT_PID")
        .ok()
        .and_then(|p| p.parse().ok())
        .or(payload["pid"].as_u64().and_then(|p| u32::try_from(p).ok()));
    let Some(params) = report(kind, &payload, &surface, pid) else {
        return 0;
    };
    let _ = ipc::call_timeout("agent.report", params, Duration::from_secs(2));
    0
}

/// An agent's hook payload in Claude Code's shape: its event names, and
/// `session_id`, `cwd`, `message` and `last_assistant_message`. None for an
/// event that means nothing here.
fn normalize(kind: Kind, payload: &Value) -> Option<Value> {
    let mut p = payload.clone();
    let event = payload["hook_event_name"].as_str().unwrap_or_default();
    let renamed = match (kind, event) {
        (Kind::Gemini, "BeforeAgent") | (Kind::Cursor, "beforeSubmitPrompt") => "UserPromptSubmit",
        (Kind::Gemini, "AfterTool") | (Kind::Cursor, "postToolUse") => "PostToolUse",
        (Kind::Gemini, "AfterAgent") => {
            p["last_assistant_message"] = payload["prompt_response"].clone();
            "Stop"
        }
        (Kind::Cursor, "sessionStart") => "SessionStart",
        (Kind::Cursor, "sessionEnd") => "SessionEnd",
        (Kind::Cursor, "stop") => {
            match payload["status"].as_str() {
                Some("aborted") => p["last_assistant_message"] = json!("Stopped"),
                Some("error") => p["last_assistant_message"] = json!("Stopped with an error"),
                _ => {}
            }
            "Stop"
        }
        (Kind::Cursor, _) => return None,
        (_, e) => e,
    };
    p["hook_event_name"] = json!(renamed);
    if kind == Kind::Cursor {
        // The chat id, which `--resume` takes; sessionStart has it too.
        p["session_id"] = payload["conversation_id"].clone();
        if p["cwd"].is_null() {
            p["cwd"] = payload["workspace_roots"][0].clone();
        }
    }
    Some(p)
}

/// `agent.report`'s params for a hook payload.
fn report(kind: Kind, payload: &Value, surface: &str, pid: Option<u32>) -> Option<Value> {
    let payload = normalize(kind, payload)?;
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
    // For `automation.workspaceAutoNaming`.
    if event == "UserPromptSubmit"
        && let Some(prompt) = payload["prompt"].as_str()
    {
        params["prompt"] = json!(prompt.chars().take(PROMPT_CHARS).collect::<String>());
    }
    if event == "Stop" {
        let summary = match payload["last_assistant_message"].as_str() {
            Some(text) => shorten(text),
            None => payload["transcript_path"].as_str().and_then(last_reply),
        };
        params["summary"] = json!(summary);
    }
    // Codex asks before running something: say what.
    if event == "PermissionRequest" && kind == Kind::Codex {
        let input = &payload["tool_input"];
        // The notification's title already names the agent.
        let what = input["command"]
            .as_str()
            .or(input["cmd"].as_str())
            .map(|c| format!("Asks to run {c}"));
        let what = what.or_else(|| {
            payload["tool_name"]
                .as_str()
                .map(|t| format!("Asks to use {t}"))
        });
        params["message"] = json!(shorten(
            &what.unwrap_or_else(|| "Needs your approval".into())
        ));
    } else if let Some(m) = payload["message"].as_str() {
        params["message"] = json!(shorten(m));
    }
    Some(params)
}

/// The start of the agent's last reply in a transcript (JSON lines), on one
/// line.
fn last_reply(path: &str) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    // Only the tail: transcripts grow to megabytes.
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(256 * 1024)))
        .ok()?;
    let mut bytes = vec![];
    f.read_to_end(&mut bytes).ok()?;
    summary(&String::from_utf8_lossy(&bytes))
}

/// The last assistant line with text. Claude marks it `"type": "assistant"`;
/// Droid and Cursor put the role in the message.
fn summary(jsonl: &str) -> Option<String> {
    jsonl
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| {
            let msg = &v["message"];
            if v["type"] != "assistant" && msg["role"] != "assistant" && v["role"] != "assistant" {
                return None;
            }
            let content = msg.get("content").unwrap_or(&v["content"]);
            let text = match content {
                Value::String(s) => s.clone(),
                c => c
                    .as_array()?
                    .iter()
                    .filter(|c| c["type"] == "text")
                    .filter_map(|c| c["text"].as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            shorten(&text)
        })
}

/// How much of a prompt goes to the app, to name its workspace from.
const PROMPT_CHARS: usize = 2000;

/// A short title for a session that starts with `prompt`, from the agent's
/// own CLI (`claude -p` on a small model, or `codex exec`), run through a
/// login shell so it's found on the user's PATH. Blocks for up to a minute;
/// None for other agents or if it fails.
pub fn title_for(kind: Kind, prompt: &str) -> Option<String> {
    use std::process::{Command, Stdio};
    let ask = format!(
        "Reply with only a title of 2 to 5 words for a coding session that starts \
         with the request below. No quotes, no trailing punctuation.\n\n<request>\n{prompt}\n</request>"
    );
    let dir = std::env::temp_dir();
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out = dir.join(format!("abromux-title-{}-{run}.txt", std::process::id()));
    let script = match kind {
        // On stdin: `--tools` takes a list, which would swallow an argument.
        Kind::Claude => {
            r#"printf '%s' "$1" | claude -p --model haiku --no-session-persistence --tools """#
                .to_string()
        }
        Kind::Codex => format!(
            r#"codex exec --skip-git-repo-check --ephemeral -s read-only -o '{}' "$1" >/dev/null 2>&1 && cat '{}'"#,
            out.display(),
            out.display()
        ),
        _ => return None,
    };
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut cmd = Command::new(shell);
    cmd.args(["-lc", &script, "abromux", &ask])
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Not as a hook of some abromux terminal.
    for (k, _) in std::env::vars() {
        if k.starts_with("ABROMUX_") {
            cmd.env_remove(k);
        }
    }
    let mut child = cmd.spawn().ok()?;
    // Read as it comes: a child that fills the pipe would otherwise block
    // until the timeout.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut text).ok();
        text
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(s) => break s,
            None if start.elapsed() > std::time::Duration::from_secs(60) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&out);
                return None;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    };
    let text = reader.join().ok()?;
    let _ = std::fs::remove_file(&out);
    status.success().then(|| clean_title(&text)).flatten()
}

/// The first line of a model's answer, as a name: no quotes or markdown,
/// at most 60 characters.
fn clean_title(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line
        .trim_start_matches(['#', '*', ' '])
        .trim_matches(['"', '\'', '`', '*', '“', '”'])
        .trim_end_matches(['.', '!', ' '])
        .trim();
    let title: String = line.chars().take(60).collect();
    (!title.is_empty()).then_some(title)
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
    hook["command"].as_str().is_some_and(|c| {
        Kind::ALL
            .iter()
            .any(|k| c.contains(&format!("hooks {}", k.subcommand())))
    })
}

/// The event map: `settings["hooks"]`, or `settings` itself.
fn events_mut(settings: &mut Map<String, Value>, top: bool) -> Option<&mut Map<String, Value>> {
    if top {
        return Some(settings);
    }
    match settings.get_mut("hooks") {
        Some(Value::Object(hooks)) => Some(hooks),
        _ => None,
    }
}

/// Drop our hooks from `settings`, and any event list left empty.
fn strip(settings: &mut Map<String, Value>, top: bool) {
    let Some(hooks) = events_mut(settings, top) else {
        return;
    };
    for list in hooks.values_mut() {
        let Value::Array(list) = list else {
            continue;
        };
        list.retain_mut(|g| match g.get_mut("hooks") {
            Some(Value::Array(inner)) => {
                inner.retain(|h| !is_ours(h));
                !inner.is_empty()
            }
            // Cursor's lists hold the hooks themselves.
            _ => !is_ours(g),
        });
    }
    hooks.retain(|_, g| g.as_array().is_none_or(|a| !a.is_empty()));
    if !top && hooks.is_empty() {
        settings.remove("hooks");
    }
}

/// Append our hooks: Codex trusts each hook by its place in the list, so
/// hooks already there keep theirs.
fn add(settings: &mut Map<String, Value>, kind: Kind, exe: &str, top: bool) -> anyhow::Result<()> {
    let hook = kind.hook(exe);
    if kind.setup() == Setup::Flat {
        settings.entry("version").or_insert(json!(1));
    }
    let hooks = if top {
        settings
    } else {
        match settings.entry("hooks").or_insert_with(|| json!({})) {
            Value::Object(hooks) => hooks,
            _ => bail!("\"hooks\" in the {} settings isn't an object", kind.title()),
        }
    };
    for event in kind.events() {
        let Value::Array(list) = hooks.entry(*event).or_insert_with(|| json!([])) else {
            bail!(
                "hooks.{event} in the {} settings isn't a list",
                kind.title()
            )
        };
        list.push(match kind.setup() {
            Setup::Flat => hook.clone(),
            _ => json!({ "hooks": [hook] }),
        });
    }
    Ok(())
}

/// `abromux hooks install|uninstall [agent…]`: edit each agent's settings in
/// place, keeping everything else. Agents that aren't set up are skipped;
/// `only` limits it to the agents named.
pub fn hooks(install: bool, only: &[String]) -> anyhow::Result<String> {
    let kinds: Vec<Kind> = if only.is_empty() {
        Kind::ALL.to_vec()
    } else {
        only.iter()
            .map(|n| {
                Kind::parse(n).with_context(|| {
                    let names: Vec<_> = Kind::ALL.iter().map(|k| k.as_str()).collect();
                    format!("no agent {n} (expected {})", names.join(", "))
                })
            })
            .collect::<anyhow::Result<_>>()?
    };
    let exe = std::env::current_exe()
        .and_then(|e| e.canonicalize())
        .context("can't find the abromux binary")?;
    let exe = exe.to_string_lossy();
    let mut out = vec![];
    for kind in kinds {
        let Some(dir) = kind.dir()? else {
            if !only.is_empty() || install {
                out.push(format!("Skipped {}: not set up here", kind.title()));
            }
            continue;
        };
        let (path, top) = kind.settings_path(&dir);
        out.push(match kind.setup() {
            Setup::Plugin => plugin(kind, &path, install, &exe)?,
            _ => edit(kind, &path, top, install, &exe)?,
        });
    }
    if install && out.iter().any(|l| l.contains("Codex hooks")) {
        out.push(
            "Codex asks you to review new hooks once: open `codex` and trust them (or run /hooks)."
                .into(),
        );
    }
    Ok(out.join("\n"))
}

fn edit(kind: Kind, path: &Path, top: bool, install: bool, exe: &str) -> anyhow::Result<String> {
    let mut settings = match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text)
            .with_context(|| format!("can't parse {}", path.display()))?
        {
            Value::Object(m) => m,
            _ => bail!("{} isn't a JSON object", path.display()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e).with_context(|| format!("can't read {}", path.display())),
    };
    strip(&mut settings, top);
    if install {
        add(&mut settings, kind, exe, top)?;
    }
    write(
        path,
        &(serde_json::to_string_pretty(&Value::Object(settings))? + "\n"),
    )?;
    Ok(format!(
        "{} {} hooks in {}",
        if install { "Installed" } else { "Removed" },
        kind.title(),
        path.display()
    ))
}

/// Write or remove our plugin file; one without our mark is someone else's.
fn plugin(kind: Kind, path: &Path, install: bool, exe: &str) -> anyhow::Result<String> {
    let ours = match std::fs::read_to_string(path) {
        Ok(text) => text.starts_with(PLUGIN_MARK),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => return Err(e).with_context(|| format!("can't read {}", path.display())),
    };
    if !ours {
        bail!(
            "{} isn't ours; move it to install {}'s",
            path.display(),
            kind.title()
        );
    }
    if install {
        write(path, &kind.plugin(exe))?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    } else {
        return Ok(format!(
            "No {} plugin to remove in {}",
            kind.title(),
            path.display()
        ));
    }
    Ok(format!(
        "{} {} plugin {}",
        if install { "Installed" } else { "Removed" },
        kind.title(),
        path.display()
    ))
}

/// Replace `path` in one step, making its folder if needed.
fn write(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("abromux-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(
            clean_title("\n  \"Fix login bug.\"\nmore").as_deref(),
            Some("Fix login bug")
        );
        assert_eq!(
            clean_title("**Refactor sidebar**").as_deref(),
            Some("Refactor sidebar")
        );
        assert_eq!(clean_title("   \n"), None);
    }

    #[test]
    fn report_validates_ids_and_drops_default_mode() {
        let a = Agent::from_report(&json!({ "kind": "claude", "session_id": "9b77eba8-c39f", "cwd": "/p", "permission_mode": "default" })).unwrap();
        assert_eq!(a.permission_mode, None);
        assert!(
            Agent::from_report(&json!({ "kind": "claude", "session_id": "x; rm -rf ~" })).is_none()
        );
        assert!(Agent::from_report(&json!({ "kind": "aider", "session_id": "abc" })).is_none());
        let c = Agent::from_report(&json!({ "kind": "codex", "session_id": "abc", "permission_mode": "bypassPermissions", "pid": 42 })).unwrap();
        assert_eq!((c.permission_mode, c.pid), (None, Some(42)));
    }

    #[test]
    fn codex_reports() {
        // Payloads as codex-cli 0.164 sends them.
        let stop = json!({ "hook_event_name": "Stop", "session_id": "01a1", "cwd": "/p", "last_assistant_message": "Done:\n  all  green." });
        let r = report(Kind::Codex, &stop, "surface:3", Some(7)).unwrap();
        assert_eq!(
            (r["kind"].as_str(), r["summary"].as_str(), r["pid"].as_u64()),
            (Some("codex"), Some("Done: all green."), Some(7))
        );
        let ask = json!({ "hook_event_name": "PermissionRequest", "session_id": "01a1", "tool_name": "Bash", "tool_input": { "command": "touch  /etc/x" } });
        assert_eq!(
            report(Kind::Codex, &ask, "surface:3", None).unwrap()["message"],
            "Asks to run touch /etc/x"
        );
        let ask = json!({ "hook_event_name": "PermissionRequest", "tool_name": "apply_patch" });
        assert_eq!(
            report(Kind::Codex, &ask, "surface:3", None).unwrap()["message"],
            "Asks to use apply_patch"
        );
    }

    #[test]
    fn other_agents_report_like_claude() {
        let r = |k, p| report(k, &p, "surface:1", None).unwrap();
        // Gemini CLI's own event names.
        let g = r(
            Kind::Gemini,
            json!({ "hook_event_name": "AfterAgent", "session_id": "a1-b2", "cwd": "/p", "prompt_response": "Fixed it." }),
        );
        assert_eq!(
            (g["event"].as_str(), g["summary"].as_str()),
            (Some("Stop"), Some("Fixed it."))
        );
        let g = r(
            Kind::Gemini,
            json!({ "hook_event_name": "BeforeAgent", "session_id": "a1-b2" }),
        );
        assert_eq!(g["event"], "UserPromptSubmit");
        let g = r(
            Kind::Gemini,
            json!({ "hook_event_name": "Notification", "notification_type": "ToolPermission", "message": "Allow  shell?" }),
        );
        assert_eq!(
            (g["event"].as_str(), g["message"].as_str()),
            (Some("Notification"), Some("Allow shell?"))
        );
        // Cursor: the chat id and the first workspace root.
        let c = r(
            Kind::Cursor,
            json!({ "hook_event_name": "stop", "conversation_id": "c-9", "workspace_roots": ["/w"], "status": "aborted" }),
        );
        assert_eq!(
            (
                c["event"].as_str(),
                c["session_id"].as_str(),
                c["cwd"].as_str(),
                c["summary"].as_str()
            ),
            (Some("Stop"), Some("c-9"), Some("/w"), Some("Stopped"))
        );
        assert!(
            report(
                Kind::Cursor,
                &json!({ "hook_event_name": "afterFileEdit" }),
                "s",
                None
            )
            .is_none()
        );
        // The plugins send Claude's names and their own pid.
        let o = r(
            Kind::OpenCode,
            json!({ "hook_event_name": "PermissionRequest", "session_id": "ses_1", "message": "Run ls" }),
        );
        assert_eq!(o["message"], "Run ls");
    }

    #[test]
    fn alive_checks_the_process() {
        let mut a = Agent {
            kind: "codex".into(),
            session_id: "a".into(),
            cwd: String::new(),
            permission_mode: None,
            pid: None,
        };
        assert!(a.alive(), "no pid: assume so");
        a.pid = Some(std::process::id());
        assert!(a.alive());
        a.pid = Some(999_999);
        assert!(!a.alive());
    }

    #[test]
    fn resume_command_cds_only_when_needed() {
        let a = Agent {
            kind: "claude".into(),
            session_id: "abc-1".into(),
            cwd: "/it's here".into(),
            permission_mode: Some("auto".into()),
            pid: None,
        };
        assert_eq!(
            a.resume_command("/it's here").unwrap(),
            "claude --resume abc-1 --permission-mode auto"
        );
        assert_eq!(
            a.resume_command("/tmp").unwrap(),
            r"cd -- '/it'\''s here' && claude --resume abc-1 --permission-mode auto"
        );
        let c = Agent {
            kind: "codex".into(),
            session_id: "01a1-2".into(),
            cwd: "/p".into(),
            permission_mode: None,
            pid: None,
        };
        assert_eq!(
            c.resume_command("/tmp").unwrap(),
            "cd -- '/p' && codex resume 01a1-2"
        );
        let o = Agent {
            kind: "opencode".into(),
            session_id: "ses_9".into(),
            ..c
        };
        assert_eq!(o.resume_command("/p").unwrap(), "opencode --session ses_9");
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
        assert_eq!(
            summary(&(jsonl + "\n{truncated")).as_deref(),
            Some("All tests pass.")
        );
        let long = json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "é".repeat(300) }] } }).to_string();
        assert_eq!(summary(&long).unwrap().chars().count(), SUMMARY_CHARS + 1);
        assert_eq!(summary(""), None);
        // Droid's transcripts.
        let droid = json!({ "type": "message", "message": { "role": "assistant", "content": [{ "type": "text", "text": "ok" }] } }).to_string();
        assert_eq!(summary(&droid).as_deref(), Some("ok"));
    }

    #[test]
    fn install_is_idempotent_and_uninstall_keeps_other_hooks() {
        let theirs = json!({ "hooks": [{ "type": "command", "command": "notify.sh" }] });
        let mut s: Map<String, Value> =
            serde_json::from_value(json!({ "model": "opus", "hooks": { "Stop": [theirs] } }))
                .unwrap();
        for _ in 0..2 {
            strip(&mut s, false);
            add(
                &mut s,
                Kind::Claude,
                "/Applications/AbroMux.app/Contents/MacOS/abromux",
                false,
            )
            .unwrap();
        }
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(s["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        strip(&mut s, false);
        assert_eq!(
            Value::Object(s),
            json!({ "model": "opus", "hooks": { "Stop": [theirs] } })
        );
    }

    #[test]
    fn codex_hooks_go_after_existing_ones() {
        let theirs = json!({ "matcher": "Edit", "hooks": [{ "type": "command", "command": "fmt.sh", "timeout": 60 }] });
        let mut s: Map<String, Value> =
            serde_json::from_value(json!({ "hooks": { "PostToolUse": [theirs] } })).unwrap();
        add(&mut s, Kind::Codex, "/x/abromux", false).unwrap();
        let post = s["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(
            post[0], theirs,
            "existing hooks keep their place (and Codex's trust)"
        );
        let cmd = post[1]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(
            cmd,
            r#"[ -n "$ABROMUX_SURFACE_ID" ] && [ -x '/x/abromux' ] && ABROMUX_AGENT_PID=$PPID '/x/abromux' hooks codex-event >/dev/null 2>&1; echo '{}'"#
        );
        assert!(
            s["hooks"].get("PermissionRequest").is_some() && s["hooks"].get("SessionEnd").is_none()
        );
        strip(&mut s, false);
        assert_eq!(
            Value::Object(s),
            json!({ "hooks": { "PostToolUse": [theirs] } })
        );
    }

    #[test]
    fn cursor_and_droid_layouts() {
        let theirs = json!({ "command": "./cursor-hook.sh", "timeout": 10 });
        let mut s: Map<String, Value> =
            serde_json::from_value(json!({ "version": 1, "hooks": { "stop": [theirs] } })).unwrap();
        add(&mut s, Kind::Cursor, "/x/abromux", false).unwrap();
        assert_eq!(s["hooks"]["stop"][1]["timeout"], 5);
        assert!(
            s["hooks"]["stop"][1]["command"]
                .as_str()
                .unwrap()
                .contains("hooks cursor-event")
        );
        strip(&mut s, false);
        assert_eq!(
            Value::Object(s),
            json!({ "version": 1, "hooks": { "stop": [theirs] } })
        );
        // Droid's hooks.json: events at the top.
        let mut d = Map::new();
        add(&mut d, Kind::Droid, "/x/abromux", true).unwrap();
        assert!(
            d["Stop"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("hooks droid-event")
        );
        strip(&mut d, true);
        assert!(d.is_empty());
        // Gemini counts milliseconds.
        let mut g = Map::new();
        add(&mut g, Kind::Gemini, "/x/abromux", false).unwrap();
        assert_eq!(g["hooks"]["AfterAgent"][0]["hooks"][0]["timeout"], 5000);
    }

    #[test]
    fn plugins_embed_the_binary() {
        let js = Kind::OpenCode.plugin("/A \"b\"/abromux");
        assert!(js.starts_with(PLUGIN_MARK));
        assert!(js.contains(r#"const exe = "/A \"b\"/abromux";"#));
        assert!(
            Kind::Pi
                .plugin("/x")
                .contains("export default function (pi)")
        );
    }
}
