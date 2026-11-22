//! Claude tabs: Claude Code as a chat page instead of a terminal.
//!
//! A Claude tab is a browser tab on `abromux-claude://localhost/<folder>`,
//! a scheme only these tabs register (like Markdown tabs' `abromux-md://`).
//! The scheme serves the chat page (`claude_chat/`), and the tab runs
//! `claude -p` with stream-json in and out in that folder, through the
//! user's login shell so it sees the same PATH and environment a terminal
//! would. What `claude` prints becomes the page's events ([`Translator`]);
//! what the page asks (send, stop, answer a permission prompt) comes back
//! over `window.ipc.postMessage` ([`Chat::page_message`]).
//!
//! The session id rides in the URL (`?session=<id>`), so a restored tab
//! shows the conversation from Claude Code's transcript file and resumes
//! it with `--resume` when you write again.

use futures::channel::mpsc;
use http::{Request, Response};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt::Write as _,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
};

pub const SCHEME: &str = "abromux-claude";
const PREFIX: &str = "abromux-claude://localhost";

pub const PAGE_HTML: &str = include_str!("claude_chat/page.html");
pub const PAGE_CSS: &str = include_str!("claude_chat/page.css");
pub const PAGE_JS: &str = include_str!("claude_chat/page.js");
pub const LOGO_SVG: &str = include_str!("../assets/icons/claude.svg");
/// highlight.js, for code in replies and diffs (`claude_chat/vendor/`).
pub const HIGHLIGHT_JS: &str = include_str!("claude_chat/vendor/highlight.min.js");

/// The page's own script and style only: Claude's replies are Markdown
/// rendered without raw HTML, and nothing inline runs.
const CSP: &str = "default-src 'none'; script-src abromux-claude:; \
    style-src 'unsafe-inline' abromux-claude:; img-src abromux-claude: data: https:; \
    font-src abromux-claude: data:; connect-src 'none'; object-src 'none'; \
    form-action 'none'; base-uri 'none'; frame-src 'none'";

/// Longest tool output sent to the page, per result.
const MAX_RESULT: usize = 40_000;

pub fn is_chat_url(url: &str) -> bool {
    url.starts_with(SCHEME) && url[SCHEME.len()..].starts_with(':')
}

/// How a Claude tab starts: the conversation it resumes and the model,
/// effort and permission mode it asks for. They ride in the tab's URL, so
/// a restored tab keeps them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Start {
    pub session: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
}

/// Claude Code's name for a permission mode, from it or a friendlier one.
pub fn mode_name(s: &str) -> Option<&'static str> {
    Some(
        match s.to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
            "ask" | "default" | "manual" | "ask-permissions" => "default",
            "accept-edits" | "acceptedits" | "edits" => "acceptEdits",
            "plan" => "plan",
            "auto" => "auto",
            "dont-ask" | "dontask" => "dontAsk",
            "bypass" | "bypass-permissions" | "bypasspermissions" | "yolo" => "bypassPermissions",
            _ => return None,
        },
    )
}

/// An effort level `claude --effort` takes.
pub fn effort_name(s: &str) -> Option<&'static str> {
    Some(match s.to_ascii_lowercase().as_str() {
        "low" => "low",
        "medium" | "med" => "medium",
        "high" => "high",
        "xhigh" | "extra-high" | "x-high" => "xhigh",
        "max" => "max",
        _ => return None,
    })
}

/// A model name or alias (`opus`, `sonnet[1m]`, `claude-opus-5-5`): only
/// what such names are made of reaches the command line.
pub fn is_model_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-[]".contains(&b))
}

fn push_encoded(out: &mut String, s: &str, keep: &[u8]) {
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || keep.contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}

/// The tab URL for a chat in `cwd`.
pub fn url_for(cwd: &Path, start: &Start) -> String {
    let mut out = String::from(PREFIX);
    push_encoded(&mut out, &cwd.to_string_lossy(), b"/-_.~");
    let mut sep = '?';
    let mut param = |out: &mut String, k: &str, v: &str| {
        out.push(sep);
        sep = '&';
        out.push_str(k);
        out.push('=');
        push_encoded(out, v, b"-_.");
    };
    if let Some(s) = start.session.as_deref().filter(|s| is_session_id(s)) {
        param(&mut out, "session", s);
    }
    if let Some(m) = start.model.as_deref().filter(|m| is_model_name(m)) {
        param(&mut out, "model", m);
    }
    if let Some(e) = start.effort.as_deref().and_then(effort_name) {
        param(&mut out, "effort", e);
    }
    if let Some(m) = start.mode.as_deref().and_then(mode_name) {
        param(&mut out, "mode", m);
    }
    out
}

/// The folder and start a tab URL names.
pub fn parse(url: &str) -> Option<(PathBuf, Start)> {
    let rest = url.strip_prefix(SCHEME)?.strip_prefix("://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let (path, query) = match rest.find('?') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let path = path.split('#').next().unwrap_or_default();
    let cwd = PathBuf::from(decode(path)?);
    let get = |k: &str| {
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='))
            .and_then(decode)
    };
    let start = Start {
        session: get("session").filter(|s| is_session_id(s)),
        model: get("model").filter(|m| is_model_name(m)),
        effort: get("effort")
            .and_then(|e| effort_name(&e))
            .map(String::from),
        mode: get("mode").and_then(|m| mode_name(&m)).map(String::from),
    };
    cwd.is_absolute().then_some((cwd, start))
}

/// What to type into a shell in the chat's folder to carry on with `start`
/// in Claude Code's terminal UI. Only values that pass the same checks as
/// the tab URL's get in, each single-quoted (zsh would glob `opus[1m]`).
pub fn terminal_command(start: &Start) -> String {
    let mut cmd = String::from("claude");
    let mut arg = |flag: &str, v: &str| {
        let _ = write!(cmd, " {flag} '{v}'");
    };
    if let Some(s) = start.session.as_deref().filter(|s| is_session_id(s)) {
        arg("--resume", s);
    }
    if let Some(m) = start.model.as_deref().filter(|m| is_model_name(m)) {
        arg("--model", m);
    }
    if let Some(e) = start.effort.as_deref().and_then(effort_name) {
        arg("--effort", e);
    }
    if let Some(m) = start.mode.as_deref().and_then(mode_name) {
        arg("--permission-mode", m);
    }
    cmd
}

/// Session ids are UUIDs; anything else isn't passed to `claude`.
fn is_session_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

fn decode(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = path.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(h, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The scheme's answer: the page's script and style under `/__chat/`, the
/// page itself for anything else (the path is the tab's folder).
pub fn respond(req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let (mime, body): (&str, Cow<'static, [u8]>) = match req.uri().path() {
        "/__chat/page.js" => ("text/javascript; charset=utf-8", PAGE_JS.as_bytes().into()),
        "/__chat/page.css" => ("text/css; charset=utf-8", PAGE_CSS.as_bytes().into()),
        "/__chat/logo.svg" => ("image/svg+xml", LOGO_SVG.as_bytes().into()),
        "/__chat/hl.js" => (
            "text/javascript; charset=utf-8",
            HIGHLIGHT_JS.as_bytes().into(),
        ),
        _ => ("text/html; charset=utf-8", PAGE_HTML.as_bytes().into()),
    };
    Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Content-Security-Policy", CSP)
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}

/// Claude's Markdown as HTML, without any raw HTML of its own.
pub fn render(text: &str) -> String {
    let opts = markdown::Options::gfm();
    markdown::to_html_with_options(text, &opts).unwrap_or_else(|_| {
        format!(
            "<p>{}</p>",
            text.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        )
    })
}

// ---- the claude process ------------------------------------------------

/// Where the chat stands, for the sidebar pill.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Nothing has run yet, or the process ended.
    Idle,
    Running,
    /// A permission prompt or question waits for you.
    Waiting,
    /// The last turn finished.
    Done,
}

/// What the tab tells the app (status pill, notifications).
pub enum Notice {
    State(State),
    NeedsInput(String),
    Finished(String),
}

/// One line from the process, or its end with the tail of its stderr;
/// or work done off the main thread for the page.
pub enum Line {
    Out(String),
    Exited(String),
    /// The folder's files, for `@` mentions.
    Files(Vec<String>),
    /// An event for the page alone (the past conversations list).
    Page(Value),
}

/// A control request waiting for its answer.
enum Pending {
    /// A mode change, and the mode to go back to if it's refused.
    Mode(Option<String>),
    /// What rewinding to before message `uuid` would restore.
    RewindPreview(String),
    /// Files restored to before message `uuid`; the conversation follows.
    Rewind(String),
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A chat's process and what the page has been shown.
pub struct Chat {
    pub cwd: PathBuf,
    /// `CLAUDE_CONFIG_DIR` for the process, from abromux's settings.
    config_dir: Option<PathBuf>,
    pub session: Option<String>,
    model: Option<String>,
    mode: Option<String>,
    effort: Option<String>,
    /// Where the next start picks up: `--resume-session-at` this message,
    /// as a fork (after a rewind).
    resume_at: Option<String>,
    fork: bool,
    proc: Option<Proc>,
    /// Bumped per process, so a dead one's late lines are dropped.
    generation: u64,
    tr: Translator,
    /// The page's events so far, newest version of each, for a page that
    /// (re)loads.
    events: Vec<Value>,
    keyed: HashMap<String, usize>,
    /// Waiting to go to the page.
    outbox: Vec<Value>,
    ready: bool,
    pub state: State,
    /// Permission prompts waiting, by request id: the tool's input and the
    /// rules "always allow" would add.
    asks: HashMap<String, (Value, Value)>,
    /// Control requests sent, for their ids.
    request_n: u64,
    pending: HashMap<String, Pending>,
    /// The folder's files, once listed, and the `@` query waiting for them.
    files: Option<Vec<String>>,
    files_query: Option<String>,
    /// The model's context window, from the last turn's result.
    context_window: Option<u64>,
    /// The first thing you asked, for the tab's title.
    pub topic: Option<String>,
}

impl Chat {
    pub fn new(cwd: PathBuf, start: Start) -> Self {
        Self {
            cwd,
            config_dir: None,
            session: start.session,
            model: start.model,
            mode: start.mode,
            effort: start.effort,
            resume_at: None,
            fork: false,
            proc: None,
            generation: 0,
            tr: Translator::default(),
            events: vec![],
            keyed: HashMap::new(),
            outbox: vec![],
            ready: false,
            state: State::Idle,
            asks: HashMap::new(),
            request_n: 0,
            pending: HashMap::new(),
            files: None,
            files_query: None,
            context_window: None,
            topic: None,
        }
    }

    pub fn with_config_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.config_dir = dir;
        self
    }

    /// Its conversation and settings as they are now, bypass mode included,
    /// to carry the session over to a terminal.
    pub fn settings(&self) -> Start {
        Start {
            session: self.session.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            mode: self.mode.clone(),
        }
    }

    /// The tab's URL: its conversation and settings, but never bypass mode
    /// (a restored tab asks again).
    pub fn url(&self) -> String {
        let start = Start {
            session: self.session.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            mode: self.mode.clone().filter(|m| m != "bypassPermissions"),
        };
        url_for(&self.cwd, &start)
    }

    /// The folder's most recent conversation, if it has one.
    pub fn latest_session(cwd: &Path, config: Option<&Path>) -> Option<String> {
        past_sessions(cwd, config, None)
            .first()
            .and_then(|s| s["id"].as_str())
            .map(String::from)
    }

    /// The page's events from a transcript, read off the main thread.
    pub fn load_history(cwd: &Path, config: Option<&Path>, session: &str) -> Vec<Value> {
        let Some(file) = transcript(cwd, config, session) else {
            return vec![];
        };
        let Ok(text) = std::fs::read_to_string(&file) else {
            return vec![];
        };
        let mut tr = Translator {
            history: true,
            ..Default::default()
        };
        let mut out = vec![];
        for line in text.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                tr.line(&v, &mut out);
            }
        }
        out
    }

    pub fn set_history(&mut self, events: Vec<Value>) {
        // Events that arrived while it loaded go after it.
        let later = std::mem::take(&mut self.events);
        self.keyed.clear();
        for ev in events.into_iter().chain(later) {
            self.record(ev);
        }
        if self.topic.is_none() {
            self.topic = self
                .events
                .iter()
                .find(|e| e["t"] == "user")
                .and_then(|e| e["text"].as_str())
                .map(topic_of);
        }
        if self.ready {
            self.outbox.clear();
            self.outbox.push(json!({"t": "reset"}));
            self.outbox.push(self.meta());
            self.outbox.extend(self.events.iter().cloned());
        }
    }

    fn meta(&self) -> Value {
        json!({
            "t": "meta",
            "cwd": crate::workspace::tilde(&self.cwd.to_string_lossy()),
            "cwdFull": self.cwd.to_string_lossy(),
            "dark": crate::theme::is_dark(),
            "session": self.session,
            "model": self.model,
            "mode": self.mode,
            "effort": self.effort,
            "contextWindow": self.context_window,
            "running": self.proc.is_some(),
            "state": self.state_name(),
        })
    }

    /// Keep `ev` for a page that reloads, replacing an older version of it.
    fn record(&mut self, ev: Value) {
        let key = event_key(&ev);
        match key.and_then(|k| self.keyed.get(&k).copied().map(|i| (k, i))) {
            Some((_, i)) => self.events[i] = ev,
            None => {
                if let Some(k) = event_key(&ev) {
                    self.keyed.insert(k, self.events.len());
                }
                self.events.push(ev);
            }
        }
    }

    fn emit(&mut self, ev: Value) {
        if self.ready {
            self.outbox.push(ev.clone());
        }
        // The status line is sent fresh on every load; no need to keep it.
        if ev["t"] != "meta" {
            self.record(ev);
        }
    }

    /// What's waiting for the page, if anything is, as a JSON array.
    pub fn take_batch(&mut self) -> Option<String> {
        if self.outbox.is_empty() {
            return None;
        }
        serde_json::to_string(&std::mem::take(&mut self.outbox)).ok()
    }

    /// The script that hands the page `batch` (from `take_batch`).
    pub fn script(batch: &str) -> String {
        format!("window.__chat && window.__chat.recv({batch})")
    }

    /// The conversation as a page that just loaded gets it, for a copy of
    /// the page elsewhere (a phone); from now on everything the page is
    /// sent goes out, for that copy too, even if this tab's own page
    /// hasn't loaded.
    pub fn mirror_start(&mut self) -> Vec<Value> {
        self.ready = true;
        let mut out = vec![json!({"t": "reset"}), self.meta()];
        out.extend(self.events.iter().cloned());
        out
    }

    pub fn state_name(&self) -> &'static str {
        match self.state {
            State::Idle => "idle",
            State::Running => "running",
            State::Waiting => "waiting",
            State::Done => "done",
        }
    }

    fn set_state(&mut self, state: State, notices: &mut Vec<Notice>) {
        if self.state != state {
            self.state = state;
            notices.push(Notice::State(state));
            let meta = self.meta();
            self.emit(meta);
        }
    }

    /// A message from the page. Lines from a process it starts go to `lines`.
    pub fn page_message(
        &mut self,
        msg: &Value,
        lines: &mpsc::UnboundedSender<(u64, Line)>,
        notices: &mut Vec<Notice>,
    ) -> Option<PageAction> {
        match msg["chat"].as_str()? {
            "ready" => {
                self.ready = true;
                self.outbox.clear();
                self.outbox.push(json!({"t": "reset"}));
                self.outbox.push(self.meta());
                self.outbox.extend(self.events.iter().cloned());
            }
            "warm" => {
                if self.proc.is_none() {
                    self.start(lines);
                }
            }
            "send" => {
                let text = msg["text"].as_str().unwrap_or_default().trim().to_string();
                let images = msg["images"].as_array().cloned().unwrap_or_default();
                if text.is_empty() && images.is_empty() {
                    return None;
                }
                if self.proc.is_none() {
                    self.start(lines);
                }
                // A new message instead of an answer: decline what waits, and
                // the message tells Claude what to do instead.
                let waiting: Vec<String> = self.asks.keys().cloned().collect();
                for id in waiting {
                    self.asks.remove(&id);
                    self.write(&json!({
                        "type": "control_response",
                        "response": {"subtype": "success", "request_id": id, "response": {
                            "behavior": "deny",
                            "message": "The user sent a new message instead of answering.",
                            // Stop the turn, so Claude reads the message
                            // rather than trying again.
                            "interrupt": true,
                        }},
                    }));
                    self.emit(json!({"t": "permission_done", "reqId": id, "allow": false}));
                }
                let mut content = vec![];
                for img in &images {
                    if let (Some(mt), Some(data)) =
                        (img["mediaType"].as_str(), img["data"].as_str())
                    {
                        content.push(json!({
                            "type": "image",
                            "source": {"type": "base64", "media_type": mt, "data": data},
                        }));
                    }
                }
                if !text.is_empty() {
                    content.push(json!({"type": "text", "text": text}));
                }
                self.write(&json!({
                    "type": "user",
                    "message": {"role": "user", "content": content},
                    "parent_tool_use_id": null,
                    "session_id": self.session.clone().unwrap_or_default(),
                }));
                if self.topic.is_none() && !text.is_empty() {
                    self.topic = Some(topic_of(&text));
                }
                let n = self.events.len();
                self.emit(json!({
                    "t": "user",
                    "key": format!("u{n}"),
                    "text": text,
                    "images": images.len(),
                }));
                self.set_state(State::Running, notices);
            }
            "interrupt" => {
                self.control(json!({"subtype": "interrupt"}));
            }
            "permission" => {
                let id = msg["reqId"].as_str()?.to_string();
                let (input, suggestions) = self.asks.remove(&id)?;
                let response = if msg["allow"].as_bool() == Some(true) {
                    let mut r = json!({
                        "behavior": "allow",
                        "updatedInput": msg.get("updatedInput").cloned().unwrap_or(input),
                    });
                    if msg["always"].as_bool() == Some(true)
                        && suggestions.as_array().is_some_and(|s| !s.is_empty())
                    {
                        r["updatedPermissions"] = suggestions;
                    }
                    r
                } else {
                    json!({
                        "behavior": "deny",
                        "message": msg["message"]
                            .as_str()
                            .filter(|m| !m.is_empty())
                            .unwrap_or("The user declined this."),
                    })
                };
                self.write(&json!({
                    "type": "control_response",
                    "response": {"subtype": "success", "request_id": id, "response": response},
                }));
                self.emit(json!({"t": "permission_done", "reqId": id, "allow": msg["allow"]}));
                if self.asks.is_empty() {
                    self.set_state(State::Running, notices);
                }
            }
            "mode" => {
                let mode = msg["mode"].as_str()?.to_string();
                if self.proc.is_some() {
                    let id = self.control(json!({"subtype": "set_permission_mode", "mode": mode}));
                    self.pending.insert(id, Pending::Mode(self.mode.clone()));
                }
                self.mode = Some(mode);
                let meta = self.meta();
                self.emit(meta);
                return Some(PageAction::UrlChanged);
            }
            "model" => {
                let model = msg["model"].as_str()?.to_string();
                if self.proc.is_some() {
                    self.control(json!({"subtype": "set_model", "model": model}));
                }
                self.model = Some(model);
                let meta = self.meta();
                self.emit(meta);
                return Some(PageAction::UrlChanged);
            }
            "effort" => {
                let effort = msg["effort"].as_str()?.to_string();
                if self.proc.is_some() {
                    self.control(json!({
                        "subtype": "apply_flag_settings",
                        "settings": {"effortLevel": effort},
                    }));
                }
                self.effort = Some(effort);
                let meta = self.meta();
                self.emit(meta);
                return Some(PageAction::UrlChanged);
            }
            "sessions" => {
                let (cwd, tx, current) = (self.cwd.clone(), lines.clone(), self.session.clone());
                let config = self.config_dir.clone();
                std::thread::spawn(move || {
                    let items = past_sessions(&cwd, config.as_deref(), current.as_deref());
                    let _ = tx
                        .unbounded_send((0, Line::Page(json!({"t": "sessions", "items": items}))));
                });
            }
            "resume" => {
                let id = msg["session"].as_str()?;
                if !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                    return None;
                }
                self.clear(Some(id.to_string()), notices);
                return Some(PageAction::Resume);
            }
            "files" => {
                let q = msg["q"].as_str().unwrap_or_default().to_string();
                match &self.files {
                    Some(files) => {
                        let items = match_files(files, &q);
                        self.outbox
                            .push(json!({"t": "files", "q": q, "items": items}));
                    }
                    None => {
                        if self.files_query.replace(q).is_none() {
                            let (cwd, tx) = (self.cwd.clone(), lines.clone());
                            std::thread::spawn(move || {
                                let _ = tx.unbounded_send((0, Line::Files(list_files(&cwd))));
                            });
                        }
                    }
                }
            }
            "rewind_preview" => {
                let uuid = msg["uuid"].as_str()?.to_string();
                self.session.as_ref()?;
                if self.proc.is_none() {
                    self.start(lines);
                }
                let id = self.control(json!({
                    "subtype": "rewind_files", "user_message_id": uuid, "dry_run": true,
                }));
                self.pending.insert(id, Pending::RewindPreview(uuid));
            }
            "rewind" => {
                let uuid = msg["uuid"].as_str()?.to_string();
                if msg["code"].as_bool() == Some(true) && self.proc.is_some() {
                    let id = self.control(json!({
                        "subtype": "rewind_files", "user_message_id": uuid,
                    }));
                    self.pending.insert(id, Pending::Rewind(uuid));
                } else {
                    return self.rewind_conversation(&uuid, notices);
                }
            }
            "new" => {
                self.proc = None;
                self.generation += 1;
                self.session = None;
                self.topic = None;
                self.tr = Translator::default();
                self.events.clear();
                self.keyed.clear();
                self.asks.clear();
                self.outbox.clear();
                self.outbox.push(json!({"t": "reset"}));
                self.set_state(State::Idle, notices);
                self.outbox.push(self.meta());
                return Some(PageAction::UrlChanged);
            }
            "open" => {
                let url = msg["url"].as_str()?;
                if url.starts_with("http://") || url.starts_with("https://") {
                    return Some(PageAction::OpenUrl(url.to_string()));
                }
            }
            _ => {}
        }
        None
    }

    /// Files dropped on the tab: images attach to the message you're
    /// writing (as a paste would), anything else goes in as an `@` mention.
    pub fn dropped(&mut self, paths: &[PathBuf]) {
        for path in paths {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let media = match ext.as_str() {
                "png" => Some("image/png"),
                "jpg" | "jpeg" => Some("image/jpeg"),
                "gif" => Some("image/gif"),
                "webp" => Some("image/webp"),
                _ => None,
            };
            // The API takes images up to 5 MB; a bigger one goes as a path.
            let small = std::fs::metadata(path).is_ok_and(|m| m.len() <= 5 * 1024 * 1024);
            if let (Some(media), true) = (media, small)
                && let Ok(bytes) = std::fs::read(path)
            {
                self.outbox.push(json!({
                    "t": "attach", "mediaType": media, "data": base64(&bytes),
                    "name": path.file_name().map(|n| n.to_string_lossy().into_owned()),
                }));
                continue;
            }
            let shown = path
                .strip_prefix(&self.cwd)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            let mention = if shown.contains(char::is_whitespace) {
                format!("@\"{shown}\"")
            } else {
                format!("@{shown}")
            };
            self.outbox.push(json!({"t": "mention", "text": mention}));
        }
    }

    /// Show conversation `session` (or none), with no process yet.
    fn clear(&mut self, session: Option<String>, notices: &mut Vec<Notice>) {
        self.proc = None;
        self.generation += 1;
        self.session = session;
        self.resume_at = None;
        self.fork = false;
        self.topic = None;
        self.tr = Translator::default();
        self.events.clear();
        self.keyed.clear();
        self.asks.clear();
        self.pending.clear();
        self.outbox.clear();
        self.outbox.push(json!({"t": "reset"}));
        self.set_state(State::Idle, notices);
        self.outbox.push(self.meta());
    }

    /// Go back to before message `uuid`: the conversation forks from the
    /// message before it when you next send, and the composer gets the
    /// message back to edit.
    fn rewind_conversation(&mut self, uuid: &str, notices: &mut Vec<Notice>) -> Option<PageAction> {
        let at = self
            .events
            .iter()
            .position(|e| e["t"] == "user" && e["uuid"] == uuid)?;
        let text = self.events[at]["text"].clone();
        let before = self
            .session
            .as_deref()
            .and_then(|s| transcript(&self.cwd, self.config_dir.as_deref(), s))
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| resume_point(&t, uuid));
        let kept: Vec<Value> = self.events[..at].to_vec();
        self.proc = None;
        self.generation += 1;
        self.asks.clear();
        self.pending.clear();
        self.events.clear();
        self.keyed.clear();
        for ev in kept {
            self.record(ev);
        }
        let mut action = None;
        match before {
            Some(at) => {
                self.resume_at = Some(at);
                self.fork = true;
            }
            // Nothing before it: a new conversation.
            None => {
                self.session = None;
                self.topic = None;
                action = Some(PageAction::UrlChanged);
            }
        }
        self.set_state(State::Idle, notices);
        self.outbox.clear();
        self.outbox.push(json!({"t": "reset"}));
        self.outbox.push(self.meta());
        self.outbox.extend(self.events.iter().cloned());
        self.outbox.push(json!({"t": "composer", "text": text}));
        action
    }

    fn start(&mut self, lines: &mpsc::UnboundedSender<(u64, Line)>) {
        self.generation += 1;
        let generation = self.generation;
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            // Your messages come back with their ids, to rewind to.
            "--replay-user-messages",
            // Bypass stays off unless you pick it from the mode menu.
            "--allow-dangerously-skip-permissions",
        ]
        .map(String::from)
        .into();
        if let Some(s) = &self.session {
            args.extend(["--resume".into(), s.clone()]);
            if let Some(at) = self.resume_at.take() {
                args.extend(["--resume-session-at".into(), at]);
            }
            if std::mem::take(&mut self.fork) {
                args.push("--fork-session".into());
            }
        }
        if let Some(e) = &self.effort {
            args.extend(["--effort".into(), e.clone()]);
        }
        if let Some(m) = &self.model {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(m) = &self.mode {
            args.extend(["--permission-mode".into(), m.clone()]);
        }
        // Through the login shell, as a terminal would run it.
        let mut cmd = Command::new(abromux_core::paths::user_shell());
        cmd.arg("-lc")
            .arg(r#"exec claude "$@""#)
            .arg("claude")
            .args(&args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // It isn't one of abromux's terminals (whose hooks report to the
        // sidebar), nor a Claude Code session's child.
        for (k, _) in std::env::vars() {
            if k.starts_with("ABROMUX_") || k == "CLAUDECODE" || k.starts_with("CLAUDE_CODE_") {
                cmd.env_remove(k);
            }
        }
        if let Some(dir) = &self.config_dir {
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        // Checkpoints of the files Claude edits, so a rewind can restore them.
        cmd.env("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING", "1");
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.emit(json!({"t": "error", "text": format!("Couldn't start claude: {e}")}));
                return;
            }
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return;
        };
        let tail = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let tail_w = tail.clone();
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut buf);
            if let Ok(mut t) = tail_w.lock() {
                let start = buf.len().saturating_sub(2000);
                let start = (start..=buf.len())
                    .find(|&i| buf.is_char_boundary(i))
                    .unwrap_or(buf.len());
                *t = buf[start..].to_string();
            }
        });
        let tx = lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.unbounded_send((generation, Line::Out(line))).is_err() {
                    return;
                }
            }
            // Let stderr's reader finish.
            std::thread::sleep(std::time::Duration::from_millis(100));
            let tail = tail.lock().map(|t| t.clone()).unwrap_or_default();
            let _ = tx.unbounded_send((generation, Line::Exited(tail)));
        });
        self.proc = Some(Proc { child, stdin });
        self.control(json!({"subtype": "initialize"}));
    }

    fn write(&mut self, v: &Value) {
        let Some(p) = &mut self.proc else { return };
        let mut line = v.to_string();
        line.push('\n');
        if p.stdin
            .write_all(line.as_bytes())
            .and_then(|_| p.stdin.flush())
            .is_err()
        {
            self.proc = None;
        }
    }

    /// Send a control request; its id.
    fn control(&mut self, request: Value) -> String {
        self.request_n += 1;
        let id = format!("abromux-{}", self.request_n);
        self.write(&json!({"type": "control_request", "request_id": id, "request": request}));
        id
    }

    /// A line from process `generation`. True if the session id (and so
    /// the tab's URL) changed.
    pub fn line(&mut self, generation: u64, line: Line, notices: &mut Vec<Notice>) -> bool {
        let text = match line {
            Line::Files(files) => {
                if let Some(q) = self.files_query.take() {
                    let items = match_files(&files, &q);
                    self.outbox
                        .push(json!({"t": "files", "q": q, "items": items}));
                }
                self.files = Some(files);
                return false;
            }
            Line::Page(ev) => {
                self.outbox.push(ev);
                return false;
            }
            _ if generation != self.generation => return false,
            Line::Out(t) => t,
            Line::Exited(tail) => {
                self.proc = None;
                self.asks.clear();
                if self.state == State::Running || self.state == State::Waiting {
                    let tail = tail.trim();
                    let text = if tail.is_empty() {
                        "Claude Code stopped.".to_string()
                    } else {
                        format!("Claude Code stopped:\n{tail}")
                    };
                    self.emit(json!({"t": "error", "text": text}));
                }
                self.set_state(State::Idle, notices);
                let meta = self.meta();
                self.emit(meta);
                return false;
            }
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            return false;
        };
        let mut changed = false;
        match v["type"].as_str() {
            Some("control_request") => {
                let req = &v["request"];
                if req["subtype"] == "can_use_tool"
                    && let Some(id) = v["request_id"].as_str()
                {
                    let tool = req["tool_name"].as_str().unwrap_or("a tool").to_string();
                    let input = req["input"].clone();
                    let suggestions = req
                        .get("permission_suggestions")
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.asks
                        .insert(id.to_string(), (input.clone(), suggestions.clone()));
                    let plan = (tool == "ExitPlanMode")
                        .then(|| input["plan"].as_str().map(render))
                        .flatten();
                    self.emit(json!({
                        "t": "permission",
                        "reqId": id,
                        "tool": tool,
                        "input": input,
                        "description": req["description"],
                        "canAlways": suggestions.as_array().is_some_and(|s| !s.is_empty()),
                        "planHtml": plan,
                        "toolUseId": req["tool_use_id"],
                    }));
                    let what = match tool.as_str() {
                        "AskUserQuestion" => "Claude has a question".to_string(),
                        "ExitPlanMode" => "Claude has a plan ready".to_string(),
                        "Bash" => match input["command"].as_str() {
                            Some(c) => format!("Asks to run {}", first_line(c, 80)),
                            None => "Asks to run a command".into(),
                        },
                        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                            let file = input["file_path"]
                                .as_str()
                                .or(input["notebook_path"].as_str())
                                .and_then(|p| Path::new(p).file_name())
                                .map(|n| n.to_string_lossy().into_owned());
                            match (tool.as_str(), file) {
                                ("Write", Some(f)) => format!("Asks to write {f}"),
                                (_, Some(f)) => format!("Asks to edit {f}"),
                                _ => format!("Asks to use {tool}"),
                            }
                        }
                        t => format!("Asks to use {t}"),
                    };
                    self.set_state(State::Waiting, notices);
                    notices.push(Notice::NeedsInput(what));
                } else {
                    // Nothing else is ours to answer; say so, so it doesn't wait.
                    if let Some(id) = v["request_id"].as_str() {
                        self.write(&json!({
                            "type": "control_response",
                            "response": {"subtype": "error", "request_id": id,
                                         "error": "not supported by this client"},
                        }));
                    }
                }
            }
            Some("control_response")
                if v["response"]["request_id"]
                    .as_str()
                    .is_some_and(|id| self.pending.contains_key(id)) =>
            {
                let id = v["response"]["request_id"].as_str().unwrap_or_default();
                let error = (v["response"]["subtype"] == "error").then(|| {
                    v["response"]["error"]
                        .as_str()
                        .unwrap_or("refused")
                        .to_string()
                });
                let r = &v["response"]["response"];
                match self.pending.remove(id) {
                    Some(Pending::Mode(previous)) => {
                        if let Some(e) = error {
                            self.mode = previous;
                            self.emit(json!({"t": "error", "text": format!("Couldn't change the mode: {e}")}));
                            let meta = self.meta();
                            self.emit(meta);
                        }
                    }
                    Some(Pending::RewindPreview(uuid)) => {
                        self.outbox.push(json!({
                            "t": "rewind_preview", "uuid": uuid,
                            "canRewind": error.is_none() && r["canRewind"] == true,
                            "files": r["filesChanged"], "insertions": r["insertions"],
                            "deletions": r["deletions"], "error": error.or(r["error"].as_str().map(String::from)),
                        }));
                    }
                    Some(Pending::Rewind(uuid)) => {
                        if let Some(e) = error {
                            self.emit(json!({"t": "error", "text": format!("Couldn't restore the files: {e}")}));
                        }
                        changed |= matches!(
                            self.rewind_conversation(&uuid, notices),
                            Some(PageAction::UrlChanged)
                        );
                    }
                    None => {}
                }
            }
            Some("control_response") => {
                let r = &v["response"]["response"];
                if r.get("commands").is_some() || r.get("models").is_some() {
                    let commands: Vec<Value> = r["commands"]
                        .as_array()
                        .map(|c| {
                            c.iter()
                                .map(|c| {
                                    json!({
                                        "name": c["name"],
                                        "description": c["description"],
                                        "hint": c["argumentHint"],
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let models: Vec<Value> = r["models"]
                        .as_array()
                        .map(|m| {
                            m.iter()
                                .map(|m| {
                                    json!({
                                        "value": m["value"],
                                        "name": m["displayName"],
                                        "description": m["description"],
                                        "resolved": m["resolvedModel"],
                                        "efforts": m["supportedEffortLevels"],
                                        "auto": m["supportsAutoMode"],
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if self.mode.is_none() {
                        self.mode = r["current_permission_mode"].as_str().map(String::from);
                    }
                    self.emit(json!({"t": "init", "commands": commands, "models": models}));
                    let meta = self.meta();
                    self.emit(meta);
                }
            }
            Some("system") if v["subtype"] == "init" => {
                if let Some(id) = v["session_id"].as_str()
                    && self.session.as_deref() != Some(id)
                {
                    self.session = Some(id.to_string());
                    changed = true;
                }
                if self.model.is_none() {
                    self.model = v["model"].as_str().map(String::from);
                }
                if let Some(m) = v["permissionMode"].as_str() {
                    self.mode = Some(m.to_string());
                }
                let meta = self.meta();
                self.emit(meta);
            }
            // Claude changing its own mode (leaving plan mode, say).
            Some("system") if v["subtype"] == "status" && v["permissionMode"].is_string() => {
                self.mode = v["permissionMode"].as_str().map(String::from);
                let meta = self.meta();
                self.emit(meta);
            }
            // Your message back, with the id a rewind needs. Matched by its
            // text: not every message comes back (slash commands don't).
            Some("user") if v["isReplay"] == true => {
                let text = match &v["message"]["content"] {
                    Value::String(t) => t.trim().to_string(),
                    Value::Array(items) => items
                        .iter()
                        .filter_map(|i| i["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                        .trim()
                        .to_string(),
                    _ => String::new(),
                };
                if let Some(uuid) = v["uuid"].as_str()
                    && let Some(i) = self.events.iter().position(|e| {
                        e["t"] == "user" && e.get("uuid").is_none() && e["text"] == text.as_str()
                    })
                {
                    let mut ev = self.events[i].clone();
                    ev["uuid"] = json!(uuid);
                    self.emit(ev);
                }
            }
            Some("result") => {
                if let Some(w) = v["modelUsage"]
                    .as_object()
                    .and_then(|m| m.values().filter_map(|u| u["contextWindow"].as_u64()).max())
                {
                    self.context_window = Some(w);
                }
                let mut out = vec![];
                self.tr.line(&v, &mut out);
                for ev in out {
                    self.emit(ev);
                }
                let error = v["is_error"].as_bool() == Some(true);
                let summary = if error {
                    v["result"]
                        .as_str()
                        .map(|r| first_line(r, 120))
                        .unwrap_or_else(|| "Failed".into())
                } else {
                    self.last_reply()
                        .map(|r| first_line(&r, 120))
                        .unwrap_or_else(|| "Finished".into())
                };
                self.asks.clear();
                self.set_state(State::Done, notices);
                notices.push(Notice::Finished(summary));
            }
            _ => {
                let mut out = vec![];
                self.tr.line(&v, &mut out);
                if !out.is_empty() && self.state != State::Waiting {
                    self.set_state(State::Running, notices);
                }
                for ev in out {
                    self.emit(ev);
                }
            }
        }
        if changed {
            let meta = self.meta();
            self.emit(meta);
        }
        changed
    }

    fn last_reply(&self) -> Option<String> {
        self.events
            .iter()
            .rev()
            .find(|e| e["t"] == "text")
            .and_then(|e| e["raw"].as_str())
            .map(String::from)
    }
}

/// What the page asked of the tab itself.
pub enum PageAction {
    UrlChanged,
    OpenUrl(String),
    /// Show the conversation now in the URL, from its transcript.
    Resume,
}

fn event_key(ev: &Value) -> Option<String> {
    let t = ev["t"].as_str()?;
    match t {
        "text" | "thinking" | "user" => Some(format!("{t}:{}", ev["key"].as_str()?)),
        "tool" => Some(format!("tool:{}", ev["id"].as_str()?)),
        "tool_result" => Some(format!("res:{}", ev["id"].as_str()?)),
        "permission" | "permission_done" => Some(format!("{t}:{}", ev["reqId"].as_str()?)),
        "sub" => Some(format!("sub:{}", ev["id"].as_str()?)),
        "sub_done" => Some(format!("subdone:{}", ev["id"].as_str()?)),
        "init" | "usage" => Some(t.into()),
        _ => None,
    }
}

fn topic_of(text: &str) -> String {
    first_line(text, 48)
}

fn first_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    if line.chars().count() > max {
        let cut: String = line.chars().take(max - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_string()
    }
}

/// The transcript Claude Code keeps for `session`: under the config
/// folder's `projects/<cwd as a name>/`, or in any project folder.
fn transcript(cwd: &Path, config: Option<&Path>, session: &str) -> Option<PathBuf> {
    let file = format!("{session}.jsonl");
    let dirs = project_dirs(cwd, config);
    for dir in &dirs {
        let p = dir.join(&file);
        if p.is_file() {
            return Some(p);
        }
    }
    for root in dirs.iter().filter_map(|d| d.parent()) {
        for dir in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let p = dir.path().join(&file);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Where to resume so the conversation ends just before message `uuid`:
/// the last of Claude's messages before it, or None if there is none.
fn resume_point(transcript: &str, uuid: &str) -> Option<String> {
    let mut by_id: HashMap<String, (Option<String>, bool)> = HashMap::new();
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(id) = v["uuid"].as_str() {
            by_id.insert(
                id.to_string(),
                (
                    v["parentUuid"].as_str().map(String::from),
                    v["type"] == "assistant",
                ),
            );
        }
    }
    let mut at = by_id.get(uuid)?.0.clone();
    while let Some(id) = at {
        let (parent, assistant) = by_id.get(&id)?;
        if *assistant {
            return Some(id);
        }
        at = parent.clone();
    }
    None
}

/// The folders Claude Code keeps transcripts in for `cwd`.
/// A Claude Code session running in a terminal, as Claude Code records it
/// itself in `<config>/sessions/<pid>.json`.
#[derive(Debug, PartialEq)]
pub struct Running {
    pub session: String,
    pub cwd: PathBuf,
    /// In the middle of a turn.
    pub busy: bool,
}

/// The Claude Code session running under `shell` (a terminal's shell pid),
/// without abromux's hooks: its `claude` process is found among the
/// shell's descendants and its session read from the file Claude Code
/// keeps for it, in the same config folders past conversations come from.
pub fn running_under(shell: u32, config: Option<&Path>) -> Option<Running> {
    let roots = config_roots(config);
    let mut level = vec![shell];
    // The shell's children, and theirs (`claude` run by a wrapper script).
    for _ in 0..3 {
        let next: Vec<u32> = level
            .iter()
            .flat_map(|&p| abromux_platform::process::children(p))
            .take(256)
            .collect();
        for pid in &next {
            let file = format!("sessions/{pid}.json");
            if let Some(r) = roots.iter().find_map(|r| session_file(&r.join(&file))) {
                return Some(r);
            }
        }
        level = next;
    }
    None
}

fn session_file(path: &Path) -> Option<Running> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let session = v["sessionId"].as_str().filter(|s| is_session_id(s))?;
    let cwd = PathBuf::from(v["cwd"].as_str()?);
    cwd.is_absolute().then(|| Running {
        session: session.into(),
        cwd,
        busy: v["status"].as_str() == Some("busy"),
    })
}

// ---- a terminal's session, as a chat ---------------------------------------

/// A Claude Code session running in a terminal, for [`follow`].
#[derive(Clone, Debug)]
pub struct Follow {
    /// The terminal's shell, to watch the session from (`running_under`).
    pub shell: Option<u32>,
    pub config: Option<PathBuf>,
    pub session: String,
    pub cwd: PathBuf,
    pub mode: Option<String>,
    pub busy: bool,
    /// `running_under` found it, so it can be watched: its state, a new
    /// conversation (`/clear`), its end. Without that (only abromux's hooks
    /// know of it) the chat follows the one conversation.
    pub watched: bool,
}

impl Follow {
    fn meta(&self) -> Value {
        json!({
            "t": "meta",
            "cwd": crate::workspace::tilde(&self.cwd.to_string_lossy()),
            "cwdFull": self.cwd.to_string_lossy(),
            "session": self.session,
            "mode": self.mode,
            "running": true,
            "state": if self.busy { "running" } else { "idle" },
            "terminal": true,
        })
    }
}

/// A transcript read as it grows: the page's events from each new line.
#[derive(Default)]
struct Tail {
    file: Option<PathBuf>,
    pos: u64,
    partial: Vec<u8>,
    tr: Translator,
}

impl Tail {
    fn new() -> Self {
        Self {
            tr: Translator {
                history: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn read(&mut self, f: &Follow, out: &mut Vec<Value>) {
        use std::io::{Seek, SeekFrom};
        if self.file.is_none() {
            // A new conversation has no file until its first message.
            self.file = transcript(&f.cwd, f.config.as_deref(), &f.session);
        }
        let Some(file) = &self.file else { return };
        let Ok(mut fh) = std::fs::File::open(file) else {
            return;
        };
        let len = fh.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.pos {
            // Rewritten from the start: read it all again.
            *self = Self {
                file: self.file.take(),
                ..Self::new()
            };
            out.push(json!({"t": "reset"}));
        }
        if len == self.pos || fh.seek(SeekFrom::Start(self.pos)).is_err() {
            return;
        }
        let mut buf = vec![];
        let Ok(n) = fh.take(len - self.pos).read_to_end(&mut buf) else {
            return;
        };
        self.pos += n as u64;
        self.partial.extend_from_slice(&buf);
        // Only whole lines: the last one may still be being written.
        let Some(end) = self.partial.iter().rposition(|&b| b == b'\n') else {
            return;
        };
        let rest = self.partial.split_off(end + 1);
        let whole = std::mem::replace(&mut self.partial, rest);
        for line in String::from_utf8_lossy(&whole).lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                self.tr.line(&v, out);
            }
        }
    }
}

/// Follow a terminal's Claude Code session as a Claude tab's page would see
/// it, for a phone that shows it as a chat (`surface.stream` with
/// `claude`): its conversation so far, then each message as Claude Code
/// writes it to its transcript, on stream `key` (`stream::private_key`,
/// opened already). Ends when the phone stops following, or when Claude
/// Code ends in the terminal.
pub fn follow(key: u64, f: Follow) {
    let _ = std::thread::Builder::new()
        .name("claude-follow".into())
        .spawn(move || follow_loop(key, f));
}

fn follow_loop(key: u64, mut f: Follow) {
    const TICK: std::time::Duration = std::time::Duration::from_millis(400);
    if !is_session_id(&f.session) {
        crate::stream::closed(key);
        return;
    }
    let mut tail = Tail::new();
    let mut fresh = true;
    let mut shown: Option<Value> = None;
    let mut quiet = 0u32;
    for tick in 0u64.. {
        if !fresh && !crate::stream::tapped(key) {
            return;
        }
        if f.watched && tick % 3 == 0 {
            match f.shell.and_then(|p| running_under(p, f.config.as_deref())) {
                None => {
                    crate::stream::closed(key);
                    return;
                }
                Some(r) => {
                    if r.session != f.session {
                        f.session = r.session;
                        f.cwd = r.cwd;
                        tail = Tail::new();
                        fresh = true;
                    }
                    f.busy = r.busy;
                }
            }
        }
        let mut out = vec![];
        if fresh {
            out.push(json!({"t": "reset"}));
        }
        tail.read(&f, &mut out);
        let meta = f.meta();
        if fresh || shown.as_ref() != Some(&meta) || out.first().is_some_and(|e| e["t"] == "reset")
        {
            out.insert(
                usize::from(out.first().is_some_and(|e| e["t"] == "reset")),
                meta.clone(),
            );
            shown = Some(meta);
        }
        fresh = false;
        quiet += 1;
        // Now and then an empty batch, so a phone that left is noticed.
        if !out.is_empty() || quiet >= 12 {
            quiet = 0;
            if let Ok(batch) = serde_json::to_string(&out) {
                crate::stream::events(key, &batch);
            }
        }
        std::thread::sleep(TICK);
    }
}

/// Claude Code's config folders: abromux's `claude.configDir`, then
/// `$CLAUDE_CONFIG_DIR`, then `~/.claude`.
fn config_roots(config: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![];
    if let Some(dir) = config {
        roots.push(dir.to_path_buf());
    }
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR")
        && !dir.is_empty()
    {
        roots.push(PathBuf::from(dir));
    }
    roots.push(abromux_core::paths::home().join(".claude"));
    roots
}

fn project_dirs(cwd: &Path, config: Option<&Path>) -> Vec<PathBuf> {
    let name: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    config_roots(config)
        .into_iter()
        .map(|r| r.join("projects").join(&name))
        .collect()
}

/// This folder's past conversations, newest first: id, when, and the
/// first thing you asked.
fn past_sessions(cwd: &Path, config: Option<&Path>, current: Option<&str>) -> Vec<Value> {
    let mut found = vec![];
    for dir in project_dirs(cwd, config) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            if Some(id.as_str()) == current {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            found.push((modified, id, path));
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    // Both folders can hold the same files (one linked to the other).
    let mut seen = HashSet::new();
    found.retain(|(_, id, _)| seen.insert(id.clone()));
    let mut out = vec![];
    for (modified, id, path) in found {
        if out.len() >= 40 {
            break;
        }
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut title = None;
        let mut summary = None;
        for line in BufReader::new(file).lines().take(400).map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v["type"] == "summary" {
                summary = v["summary"].as_str().map(String::from);
            }
            if let Some(t) = prompt_text(&v) {
                title = Some(t);
                break;
            }
        }
        // The last thing you asked, from the file's end, tells apart
        // conversations that began alike (a rewind's fork, say).
        let last = std::fs::File::open(&path).ok().and_then(|mut f| {
            use std::io::{Seek, SeekFrom};
            let len = f.metadata().ok()?.len();
            f.seek(SeekFrom::Start(len.saturating_sub(256 * 1024)))
                .ok()?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).ok()?;
            String::from_utf8_lossy(&buf)
                .lines()
                .rev()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .find_map(|v| prompt_text(&v))
        });
        if let Some(title) = summary.or(title) {
            let last = last.filter(|l| *l != title);
            out.push(json!({"id": id, "title": title, "last": last, "at": modified}));
        }
    }
    out
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A transcript line's text if it's something you typed.
fn prompt_text(v: &Value) -> Option<String> {
    if v["type"] != "user" || v["isMeta"] == true || v["isSidechain"] == true {
        return None;
    }
    let text = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .find(|i| i["type"] == "text")
            .and_then(|i| i["text"].as_str())?
            .to_string(),
        _ => return None,
    };
    let t = text.trim();
    (!t.is_empty() && !t.starts_with('<') && !t.starts_with("[Request interrupted"))
        .then(|| first_line(t, 90))
}

/// The folder's files for `@` mentions: git's list (tracked and new, not
/// ignored), else a shallow walk.
fn list_files(cwd: &Path) -> Vec<String> {
    let git = Command::new("git")
        .args(["ls-files", "-co", "--exclude-standard"])
        .current_dir(cwd)
        .stderr(Stdio::null())
        .output();
    if let Ok(out) = git
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout)
            .lines()
            .take(50_000)
            .map(String::from)
            .collect();
    }
    let mut files = vec![];
    let mut dirs = vec![(cwd.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            if path.is_dir() {
                if depth < 6 {
                    dirs.push((path, depth + 1));
                }
            } else if let Ok(rel) = path.strip_prefix(cwd) {
                files.push(rel.to_string_lossy().into_owned());
                if files.len() >= 20_000 {
                    return files;
                }
            }
        }
    }
    files
}

/// The best few files for `q`: a file name starting with it first, then
/// a name containing it, then a path containing it.
fn match_files(files: &[String], q: &str) -> Vec<String> {
    let q = q.to_lowercase();
    let mut scored: Vec<(u8, usize, &String)> = files
        .iter()
        .filter_map(|f| {
            let lower = f.to_lowercase();
            let name = lower.rsplit('/').next().unwrap_or(&lower);
            let rank = if q.is_empty() || name.starts_with(&q) {
                0
            } else if name.contains(&q) {
                1
            } else if lower.contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, f.len(), f))
        })
        .collect();
    scored.sort();
    scored
        .into_iter()
        .take(12)
        .map(|(_, _, f)| f.clone())
        .collect()
}

// ---- claude's stream to the page's events --------------------------------

/// Turns `claude`'s stream-json (and transcript lines) into the page's
/// events: `user`, `text`, `thinking`, `tool`, `tool_result`, `note`,
/// `result`, `error`. Text and thinking stream in as they're written;
/// each event with a key replaces the one before it.
#[derive(Default)]
pub struct Translator {
    /// Reading a transcript, not a live stream.
    history: bool,
    /// Messages whose blocks came as stream events, so their full copy
    /// isn't shown twice.
    streamed: HashSet<String>,
    msg: String,
    /// The streaming message's blocks by index: key and text so far.
    blocks: HashMap<u64, (String, String, &'static str)>,
    n: usize,
}

impl Translator {
    pub fn line(&mut self, v: &Value, out: &mut Vec<Value>) {
        // A subagent's steps go inside its tool card.
        if let Some(parent) = v["parent_tool_use_id"].as_str() {
            self.subagent(parent, v, out);
            return;
        }
        if v["isSidechain"] == true || v["isReplay"] == true {
            return;
        }
        match v["type"].as_str() {
            Some("stream_event") => self.stream(&v["event"], out),
            Some("assistant") => self.assistant(v, out),
            Some("user") => self.user(v, out),
            Some("system") => match v["subtype"].as_str() {
                Some("compact_boundary") => {
                    out.push(json!({"t": "note", "text": "Conversation compacted"}))
                }
                Some("api_retry") | Some("api_error") => {
                    let what = v["error"]["message"]
                        .as_str()
                        .or(v["message"].as_str())
                        .unwrap_or("Retrying the request…");
                    out.push(json!({"t": "note", "text": what}))
                }
                _ => {}
            },
            Some("result") => out.push(json!({
                "t": "result",
                "error": v["is_error"] == true,
                "text": if v["is_error"] == true { v["result"].clone() } else { Value::Null },
                "ms": v["duration_ms"],
                "cost": v["total_cost_usd"],
                "turns": v["num_turns"],
            })),
            _ => {}
        }
    }

    fn stream(&mut self, e: &Value, out: &mut Vec<Value>) {
        match e["type"].as_str() {
            Some("message_start") => {
                self.msg = e["message"]["id"].as_str().unwrap_or_default().to_string();
                self.streamed.insert(self.msg.clone());
                self.blocks.clear();
            }
            Some("content_block_start") => {
                let idx = e["index"].as_u64().unwrap_or(0);
                let b = &e["content_block"];
                let key = format!("{}:{idx}", self.msg);
                match b["type"].as_str() {
                    Some("text") => {
                        self.blocks.insert(idx, (key, String::new(), "text"));
                    }
                    Some("thinking") => {
                        out.push(json!({"t": "thinking", "key": key, "text": "", "live": true}));
                        self.blocks.insert(idx, (key, String::new(), "thinking"));
                    }
                    Some("tool_use") | Some("server_tool_use") => {
                        out.push(json!({
                            "t": "tool",
                            "id": b["id"],
                            "name": b["name"],
                            "input": null,
                            "live": true,
                        }));
                    }
                    _ => {}
                }
            }
            Some("content_block_delta") => {
                let idx = e["index"].as_u64().unwrap_or(0);
                let d = &e["delta"];
                let Some((key, text, kind)) = self.blocks.get_mut(&idx) else {
                    return;
                };
                match (d["type"].as_str(), *kind) {
                    (Some("text_delta"), "text") => {
                        text.push_str(d["text"].as_str().unwrap_or_default());
                        out.push(json!({
                            "t": "text", "key": key, "html": render(text), "raw": text, "live": true,
                        }));
                    }
                    (Some("thinking_delta"), "thinking") => {
                        text.push_str(d["thinking"].as_str().unwrap_or_default());
                        out.push(json!({"t": "thinking", "key": key, "text": text, "live": true}));
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let idx = e["index"].as_u64().unwrap_or(0);
                if let Some((key, text, kind)) = self.blocks.get(&idx) {
                    out.push(match *kind {
                        "text" => {
                            json!({"t": "text", "key": key, "html": render(text), "raw": text})
                        }
                        _ => json!({"t": "thinking", "key": key, "text": text}),
                    });
                }
            }
            _ => {}
        }
    }

    /// A subagent's tool calls and what it says, for its card.
    fn subagent(&mut self, parent: &str, v: &Value, out: &mut Vec<Value>) {
        let content = &v["message"]["content"];
        for (i, b) in content.as_array().into_iter().flatten().enumerate() {
            match (v["type"].as_str(), b["type"].as_str()) {
                (Some("assistant"), Some("tool_use")) => out.push(json!({
                    "t": "sub", "parent": parent, "id": b["id"], "name": b["name"], "input": b["input"],
                })),
                (Some("assistant"), Some("text")) => {
                    let text = b["text"].as_str().unwrap_or_default();
                    if !text.trim().is_empty() {
                        let id = format!("{}:{i}", v["message"]["id"].as_str().unwrap_or_default());
                        out.push(json!({
                            "t": "sub", "parent": parent, "id": id, "text": first_line(text, 160),
                        }));
                    }
                }
                (Some("user"), Some("tool_result")) => out.push(json!({
                    "t": "sub_done", "parent": parent, "id": b["tool_use_id"],
                    "error": b["is_error"] == true,
                })),
                _ => {}
            }
        }
    }

    fn assistant(&mut self, v: &Value, out: &mut Vec<Value>) {
        let m = &v["message"];
        // How full the context is: what this reply read and wrote.
        let u = &m["usage"];
        if u.is_object() {
            let used: u64 = [
                "input_tokens",
                "cache_creation_input_tokens",
                "cache_read_input_tokens",
                "output_tokens",
            ]
            .iter()
            .filter_map(|k| u[*k].as_u64())
            .sum();
            if used > 0 {
                out.push(json!({"t": "usage", "used": used}));
            }
        }
        let id = m["id"].as_str().unwrap_or_default().to_string();
        let streamed = self.streamed.contains(&id);
        for b in m["content"].as_array().into_iter().flatten() {
            match b["type"].as_str() {
                Some("text") if !streamed => {
                    let text = b["text"].as_str().unwrap_or_default();
                    if text.trim().is_empty() {
                        continue;
                    }
                    self.n += 1;
                    out.push(json!({
                        "t": "text", "key": format!("{id}:h{}", self.n),
                        "html": render(text), "raw": text,
                    }));
                }
                Some("thinking") if !streamed => {
                    let text = b["thinking"].as_str().unwrap_or_default();
                    self.n += 1;
                    out.push(
                        json!({"t": "thinking", "key": format!("{id}:h{}", self.n), "text": text}),
                    );
                }
                Some("tool_use") | Some("server_tool_use") => out.push(json!({
                    "t": "tool", "id": b["id"], "name": b["name"], "input": b["input"],
                })),
                Some("web_search_tool_result") => out.push(json!({
                    "t": "tool_result", "id": b["tool_use_id"], "text": "", "error": false,
                })),
                _ => {}
            }
        }
    }

    fn user(&mut self, v: &Value, out: &mut Vec<Value>) {
        if v["isMeta"] == true
            || v["isCompactSummary"] == true
            || v["isVisibleInTranscriptOnly"] == true
        {
            return;
        }
        let content = &v["message"]["content"];
        let mut texts = vec![];
        let mut images = 0;
        match content {
            Value::String(s) => texts.push(s.clone()),
            Value::Array(items) => {
                for it in items {
                    match it["type"].as_str() {
                        Some("tool_result") => {
                            let (text, imgs) = result_text(&it["content"]);
                            out.push(json!({
                                "t": "tool_result",
                                "id": it["tool_use_id"],
                                "text": text,
                                "images": imgs,
                                "error": it["is_error"] == true,
                            }));
                        }
                        Some("text") => texts.push(it["text"].as_str().unwrap_or_default().into()),
                        Some("image") => images += 1,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        for text in texts {
            let t = text.trim();
            if let Some(rest) = t.strip_prefix("<local-command-stdout>") {
                let rest = rest.trim_end_matches("</local-command-stdout>").trim();
                if !rest.is_empty() {
                    out.push(json!({"t": "note", "text": rest}));
                }
                continue;
            }
            if let Some(cmd) = tag(t, "command-name") {
                // A slash command you ran, shown as you typed it (history only:
                // a live one is shown when you send it).
                if self.history {
                    let args = tag(t, "command-args").unwrap_or_default();
                    self.n += 1;
                    out.push(json!({
                        "t": "user", "key": format!("h{}", self.n),
                        "text": format!("{cmd} {args}").trim(), "images": 0, "uuid": v["uuid"],
                    }));
                }
                continue;
            }
            if t.is_empty() || t.starts_with('<') || t.starts_with("[Request interrupted") {
                if t.starts_with("[Request interrupted") {
                    out.push(json!({"t": "note", "text": "Interrupted"}));
                }
                continue;
            }
            // Live, your messages are shown as you send them.
            if self.history {
                self.n += 1;
                out.push(json!({
                    "t": "user", "key": format!("h{}", self.n), "text": t, "images": images,
                    "uuid": v["uuid"],
                }));
            }
        }
    }
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim())
}

/// A tool result's text (cut to [`MAX_RESULT`]) and how many images it has.
fn result_text(content: &Value) -> (String, usize) {
    let mut text = String::new();
    let mut images = 0;
    match content {
        Value::String(s) => text.push_str(s),
        Value::Array(items) => {
            for it in items {
                match it["type"].as_str() {
                    Some("text") => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(it["text"].as_str().unwrap_or_default());
                    }
                    Some("image") => images += 1,
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if text.len() > MAX_RESULT {
        let mut cut = MAX_RESULT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let more = text.len() - cut;
        text.truncate(cut);
        let _ = write!(text, "\n… {more} more bytes");
    }
    (text, images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_command_resumes_with_settings() {
        let start = Start {
            session: Some("fb276ed8-926e-467b-91cd-3a24258c3ef7".into()),
            model: Some("opus[1m]".into()),
            effort: Some("high".into()),
            mode: Some("acceptEdits".into()),
        };
        assert_eq!(
            terminal_command(&start),
            "claude --resume 'fb276ed8-926e-467b-91cd-3a24258c3ef7' --model 'opus[1m]' \
             --effort 'high' --permission-mode 'acceptEdits'"
        );
        assert_eq!(terminal_command(&Start::default()), "claude");
        let bad = Start {
            session: Some("x; rm -rf ~".into()),
            model: Some("o'pus".into()),
            ..Default::default()
        };
        assert_eq!(terminal_command(&bad), "claude");
    }

    #[test]
    fn finds_the_session_claude_records_for_a_child() {
        let dir = std::env::temp_dir().join(format!("abromux-running-{}", std::process::id()));
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::write(
            dir.join(format!("sessions/{}.json", child.id())),
            r#"{"pid":1,"sessionId":"95a19daa-2cbc-42a2-843c-208f000a0e8c","cwd":"/p","status":"busy"}"#,
        )
        .unwrap();
        let got = running_under(std::process::id(), Some(&dir));
        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            got,
            Some(Running {
                session: "95a19daa-2cbc-42a2-843c-208f000a0e8c".into(),
                cwd: "/p".into(),
                busy: true,
            })
        );
    }

    #[test]
    fn follows_a_transcript_a_whole_line_at_a_time() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("abromux-follow-{}", std::process::id()));
        let session = "95a19daa-2cbc-42a2-843c-208f000a0e8d";
        let f = Follow {
            shell: None,
            config: Some(dir.clone()),
            session: session.into(),
            cwd: "/p/app".into(),
            mode: None,
            busy: false,
            watched: false,
        };
        let mut tail = Tail::new();
        let mut out = vec![];
        // No file yet (a new conversation): nothing, and no harm.
        tail.read(&f, &mut out);
        assert!(out.is_empty());
        let project = dir.join("projects/-p-app");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{session}.jsonl"));
        let user =
            json!({"type":"user","message":{"role":"user","content":"Fix the bug"}}).to_string();
        let reply = json!({"type":"assistant","message":{"id":"m","content":[{"type":"text","text":"Done."}]}}).to_string();
        let mut fh = std::fs::File::create(&path).unwrap();
        write!(fh, "{user}\n{}", &reply[..10]).unwrap();
        tail.read(&f, &mut out);
        let kinds = |out: &[Value]| {
            out.iter()
                .map(|e| e["t"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds(&out), ["user"]);
        out.clear();
        // The rest of a half-written line.
        writeln!(fh, "{}", &reply[10..]).unwrap();
        tail.read(&f, &mut out);
        assert_eq!(kinds(&out), ["text"]);
        out.clear();
        // Rewritten shorter: from the start again.
        std::fs::write(&path, format!("{user}\n")).unwrap();
        tail.read(&f, &mut out);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(kinds(&out), ["reset", "user"]);
    }

    #[test]
    fn url_round_trip() {
        let cwd = Path::new("/Users/me/my code/app");
        let start = Start {
            session: Some("fb276ed8-926e-467b-91cd-3a24258c3ef7".into()),
            model: Some("opus[1m]".into()),
            effort: Some("high".into()),
            mode: Some("plan".into()),
        };
        let url = url_for(cwd, &start);
        assert!(is_chat_url(&url));
        assert_eq!(parse(&url), Some((cwd.to_path_buf(), start)));
        assert_eq!(
            parse(&url_for(cwd, &Start::default())),
            Some((cwd.to_path_buf(), Start::default()))
        );
        // Nothing but ids and known names reach the command line.
        assert_eq!(
            parse(
                "abromux-claude://localhost/tmp?session=--dangerously&model=--x&mode=sudo&effort=11"
            ),
            Some((PathBuf::from("/tmp"), Start::default()))
        );
        assert_eq!(mode_name("accept-edits"), Some("acceptEdits"));
        assert_eq!(mode_name("bypass"), Some("bypassPermissions"));
        assert_eq!(effort_name("extra-high"), Some("xhigh"));
        assert!(!is_chat_url("abromux-md://localhost/tmp"));
    }

    #[test]
    fn render_drops_raw_html() {
        let html = render("hi <img src=x onerror=alert(1)> **bold**");
        assert!(!html.contains("<img"), "{html}");
        assert!(html.contains("<strong>bold</strong>"));
    }

    fn run(lines: &[Value]) -> Vec<Value> {
        let mut tr = Translator::default();
        let mut out = vec![];
        for l in lines {
            tr.line(l, &mut out);
        }
        out
    }

    #[test]
    fn streams_text_then_skips_the_full_copy() {
        let out = run(&[
            json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}),
            json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"**Hi**"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}}),
            json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"**Hi**"}]}}),
        ]);
        let texts: Vec<_> = out.iter().filter(|e| e["t"] == "text").collect();
        assert_eq!(texts.len(), 2, "{out:?}");
        assert!(texts.iter().all(|e| e["key"] == "m1:0"));
        assert!(
            texts[1]["html"]
                .as_str()
                .unwrap()
                .contains("<strong>Hi</strong>")
        );
        assert!(texts[1].get("live").is_none());
    }

    #[test]
    fn tools_and_results() {
        let out = run(&[
            json!({"type":"assistant","message":{"id":"m2","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"a\nb","is_error":false}]}}),
            json!({"type":"assistant","parent_tool_use_id":"t9","message":{"id":"m3","content":[{"type":"text","text":"sub"}]}}),
        ]);
        assert_eq!(out.len(), 3, "{out:?}");
        assert_eq!(out[0]["t"], "tool");
        assert_eq!(out[0]["input"]["command"], "ls");
        assert_eq!(out[1]["t"], "tool_result");
        assert_eq!(out[1]["text"], "a\nb");
        // A subagent's words stay in its card.
        assert_eq!(out[2]["t"], "sub");
        assert_eq!(out[2]["parent"], "t9");
    }

    #[test]
    fn rewind_resumes_at_the_reply_before() {
        let t = [
            json!({"type":"user","uuid":"u1","parentUuid":null}),
            json!({"type":"assistant","uuid":"a1","parentUuid":"u1"}),
            json!({"type":"system","uuid":"s1","parentUuid":"a1"}),
            json!({"type":"user","uuid":"u2","parentUuid":"s1"}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        assert_eq!(resume_point(&t, "u2").as_deref(), Some("a1"));
        // The first message: nothing to resume, a new conversation.
        assert_eq!(resume_point(&t, "u1"), None);
    }

    #[test]
    fn your_message_back_gets_its_id() {
        let mut chat = Chat::new(PathBuf::from("/tmp"), Start::default());
        chat.emit(json!({"t":"user","key":"u0","text":"/model","images":0}));
        chat.emit(json!({"t":"user","key":"u1","text":"hi","images":0}));
        let mut notices = vec![];
        let line = json!({"type":"user","isReplay":true,"uuid":"abc","message":{"role":"user","content":"hi"}});
        chat.line(0, Line::Out(line.to_string()), &mut notices);
        // Not the slash command before it, which never comes back.
        assert!(chat.events[0].get("uuid").is_none());
        assert_eq!(chat.events[1]["uuid"], "abc");
        assert_eq!(chat.events.len(), 2);
    }

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b"hi"), "aGk=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"a"), "YQ==");
    }

    #[test]
    fn drops_become_mentions() {
        let mut chat = Chat::new(PathBuf::from("/repo"), Start::default());
        chat.dropped(&[
            PathBuf::from("/repo/src/main.rs"),
            PathBuf::from("/tmp/my notes.txt"),
        ]);
        let texts: Vec<_> = chat.outbox.iter().map(|e| e["text"].clone()).collect();
        assert_eq!(
            texts,
            [json!("@src/main.rs"), json!("@\"/tmp/my notes.txt\"")]
        );
    }

    #[test]
    fn files_match_by_name_first() {
        let files: Vec<String> = [
            "src/main.rs",
            "docs/maintain.md",
            "src/ui/domain.rs",
            "README.md",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            match_files(&files, "main"),
            ["src/main.rs", "docs/maintain.md", "src/ui/domain.rs"]
        );
        assert_eq!(match_files(&files, "ui/"), ["src/ui/domain.rs"]);
        assert_eq!(match_files(&files, "").len(), 4);
    }

    #[test]
    fn history_shows_your_messages_but_not_meta() {
        let mut tr = Translator {
            history: true,
            ..Default::default()
        };
        let mut out = vec![];
        for l in [
            json!({"type":"user","message":{"role":"user","content":"Fix the bug"}}),
            json!({"type":"user","isMeta":true,"message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}),
            json!({"type":"user","message":{"role":"user","content":"<command-name>/review</command-name><command-args>12</command-args>"}}),
            json!({"type":"assistant","message":{"id":"m","content":[{"type":"text","text":"Done."}]}}),
        ] {
            tr.line(&l, &mut out);
        }
        let users: Vec<_> = out.iter().filter(|e| e["t"] == "user").collect();
        assert_eq!(users.len(), 2, "{out:?}");
        assert_eq!(users[0]["text"], "Fix the bug");
        assert_eq!(users[1]["text"], "/review 12");
        assert_eq!(out.last().unwrap()["t"], "text");
    }

    #[test]
    fn events_replace_by_key() {
        let mut chat = Chat::new(PathBuf::from("/tmp"), Start::default());
        chat.emit(json!({"t":"text","key":"a","html":"1"}));
        chat.emit(json!({"t":"text","key":"a","html":"12"}));
        chat.emit(json!({"t":"note","text":"x"}));
        assert_eq!(chat.events.len(), 2);
        assert_eq!(chat.events[0]["html"], "12");
    }
}
