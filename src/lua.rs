//! Lua scripting, after Rex's (`rex do`, `rex.action`):
//!
//! - `abromux do <script.lua>` runs a script in the CLI process. It drives
//!   the app over the control socket like any client: every socket method
//!   is `abromux.<namespace>.<method>{...}` (`abromux.surface.split{...}`),
//!   with `abromux.call` for any by name. Around that: `exec` (a command,
//!   no shell), `sleep`, `wait` (for an event), `on` (a handler for every
//!   such event), `every`/`after` (timers), `ui.input`/`ui.pick` (a
//!   question in the palette), `json`, `log` and `stop`.
//! - `~/.config/abromux/init.lua` defines actions (`abromux.action{...}`).
//!   The app reads it with `collect` (no calls, a time limit) and lists each
//!   in the palette, with its `key` as a shortcut; choosing one runs
//!   `abromux do init.lua --action <name>` (settings.rs, main.rs).
//!
//! A script's events come from one `events.subscribe` connection opened
//! when it starts, so `wait` sees what happened since, and handlers and
//! `wait` only hear the script's workspace unless it runs with `--all`.
//! Handlers, timers and the script's own lines never run at the same time.

use crate::{api, ipc};
use mlua::{
    DeserializeOptions, Function, HookTriggers, Lua, LuaSerdeExt, MultiValue, RegistryKey,
    SerializeOptions, Table, Value as Lv, VmState,
};
use serde_json::{Map, Value, json};
use std::{
    cell::RefCell,
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

const PRELUDE: &str = include_str!("lua_prelude.lua");
/// Modules that come with abromux, for `require`: features written as Lua
/// that init.lua turns on (`require("abromux.workspace_folders").setup{}`).
const MODULES: &[(&str, &str)] = &[(
    "abromux.workspace_folders",
    include_str!("lua/workspace_folders.lua"),
)];
/// More timers than this at once ends the script: it's almost always a
/// handler that makes one per event and never cancels it (as in Rex).
const MAX_TIMERS: usize = 1024;
/// Events kept for `wait` and handlers that haven't run yet; older ones go.
const MAX_PENDING: usize = 1000;
/// How long init.lua may take to load inside the app.
const LOAD_LIMIT: Duration = Duration::from_secs(2);

/// The actions file, next to config.json.
pub fn init_path() -> PathBuf {
    crate::settings::path().with_file_name("init.lua")
}

/// init.lua's path, after making one with a short starter if there's
/// none, so it can be opened in an editor.
pub fn ensure_init_file() -> PathBuf {
    let path = init_path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, INIT_STARTER);
    }
    path
}

const INIT_STARTER: &str = r#"-- AbroMux init.lua: your own commands, in the palette (⌘⇧P) and as
-- `abromux <name>` on the command line. AbroMux reloads it on save.
-- The guide's "Lua scripts" section has the whole API.

-- Workspace folders: ⇧⌘I makes one, ⇧⌘X cleans it up.
-- require("abromux.workspace_folders").setup{}

-- abromux.action{
--   name  = "hello",
--   title = "Say Hello",
--   key   = "cmd-alt-h",
--   run   = function(ctx)
--     abromux.notification.create{ title = "Hello from init.lua" }
--   end,
-- }
"#;

/// An action `abromux.action` defined, as the palette lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionDef {
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    pub keywords: Vec<String>,
    /// Its shortcut as config.json writes one (`"cmd-alt-t"`, a list, or
    /// `{ "keys": …, "when": … }`).
    pub key: Option<Value>,
    /// Listed in the palette (`palette = false` keeps a command for the
    /// CLI, `abromux <name>`, out of it).
    pub palette: bool,
}

struct Timer {
    id: u64,
    due: Instant,
    every: Option<Duration>,
    f: RegistryKey,
}

#[derive(Default)]
struct State {
    /// init.lua loading in the app: nothing reaches the socket.
    loading: bool,
    /// The script's top level is done; `on` is no longer allowed.
    looping: bool,
    /// What API calls default to (`caller`): the script's surface, or the
    /// event's while a handler runs.
    caller: Option<String>,
    /// Only this workspace's events (None: every workspace's).
    scope: Option<String>,
    events: Option<mpsc::Receiver<Value>>,
    pending: VecDeque<Value>,
    handlers: Vec<(String, RegistryKey)>,
    timers: Vec<Timer>,
    next_timer: u64,
    fail_fast: bool,
}

type Shared = Rc<RefCell<State>>;

fn rt(msg: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(msg)
}

/// An error as a person reads it: the message, without mlua's wrapping
/// or the stack traceback.
pub fn error_text(e: &mlua::Error) -> String {
    let text = match e {
        mlua::Error::RuntimeError(m) | mlua::Error::SyntaxError { message: m, .. } => m.clone(),
        mlua::Error::CallbackError { cause, .. } => return error_text(cause),
        mlua::Error::ExternalError(e) => e.to_string(),
        e => e.to_string(),
    };
    match text.split_once("\nstack traceback:") {
        Some((msg, _)) => msg.to_string(),
        None => text,
    }
}

/// JSON → Lua: `null` is nil, as in Rex.
pub fn to_lua(lua: &Lua, v: &Value) -> mlua::Result<Lv> {
    lua.to_value_with(
        v,
        SerializeOptions::new()
            .serialize_none_to_null(false)
            .serialize_unit_to_null(false),
    )
}

/// Lua → JSON: a table with keys 1..n is an array, any other (an empty one
/// too) an object, its keys sorted so the same table always prints the same.
pub fn from_lua(lua: &Lua, v: Lv) -> mlua::Result<Value> {
    lua.from_value_with(v, DeserializeOptions::new().sort_keys(true))
}

/// `n=2` is a number, `on=true` a boolean, anything else a string.
pub fn parse_arg(a: &str) -> Result<(String, Value), String> {
    let (k, v) = a
        .split_once('=')
        .filter(|(k, _)| !k.is_empty())
        .ok_or_else(|| format!("expected key=value, got {a:?}"))?;
    let v = match v {
        "true" => json!(true),
        "false" => json!(false),
        _ => match (v.parse::<i64>(), v.parse::<f64>()) {
            (Ok(n), _) => json!(n),
            (_, Ok(f)) if f.is_finite() => json!(f),
            _ => json!(v),
        },
    };
    Ok((k.to_string(), v))
}

/// `--for`: seconds, or a number with `s`, `m` or `h`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let (n, unit) = match s.char_indices().last() {
        Some((i, c @ ('s' | 'm' | 'h'))) => (&s[..i], c),
        _ => (s, 's'),
    };
    let n: f64 = n
        .parse()
        .ok()
        .filter(|n: &f64| n.is_finite() && *n >= 0.)
        .ok_or_else(|| format!("bad duration {s:?} (like 30, 30s, 10m or 2h)"))?;
    let mult = match unit {
        'm' => 60.,
        'h' => 3600.,
        _ => 1.,
    };
    Ok(Duration::from_secs_f64(n * mult))
}

fn signal_name(n: i32) -> String {
    match n {
        1 => "SIGHUP".into(),
        2 => "SIGINT".into(),
        3 => "SIGQUIT".into(),
        6 => "SIGABRT".into(),
        9 => "SIGKILL".into(),
        11 => "SIGSEGV".into(),
        13 => "SIGPIPE".into(),
        15 => "SIGTERM".into(),
        n => format!("SIG{n}"),
    }
}

fn tilde(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => crate::paths::home().join(rest),
        None if p == "~" => crate::paths::home(),
        None => PathBuf::from(p),
    }
}

/// `abromux.exec(argv, opts)`: run a command and wait for it.
fn exec(lua: &Lua, argv: Vec<String>, opts: Option<Table>) -> mlua::Result<(Lv, Option<String>)> {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    let Some(program) = argv.first() else {
        return Err(rt("abromux.exec: the command is empty"));
    };
    let mut cmd = Command::new(program);
    cmd.args(&argv[1..]);
    let mut stdin: Option<Vec<u8>> = None;
    let mut timeout = None;
    let (mut keep_out, mut keep_err, mut detach, mut clear_env) = (true, true, false, false);
    let mut env: Vec<(String, Option<String>)> = vec![];
    if let Some(o) = opts {
        for pair in o.pairs::<String, Lv>() {
            let (k, v) = pair?;
            match (k.as_str(), v) {
                ("cwd", Lv::String(s)) => {
                    cmd.current_dir(tilde(&s.to_string_lossy()));
                }
                ("env", Lv::Table(t)) => {
                    for pair in t.pairs::<String, Lv>() {
                        let (k, v) = pair?;
                        env.push((
                            k,
                            match v {
                                Lv::Boolean(false) | Lv::Nil => None,
                                Lv::String(s) => Some(s.to_string_lossy()),
                                v => Some(v.to_string()?),
                            },
                        ));
                    }
                }
                ("clear_env", v) => clear_env = v.as_boolean() == Some(true),
                ("stdin", Lv::String(s)) => stdin = Some(s.as_bytes().to_vec()),
                ("timeout", v) => timeout = v.as_f64().or(v.as_i64().map(|i| i as f64)),
                ("stdout", Lv::Boolean(b)) => keep_out = b,
                ("stderr", Lv::Boolean(b)) => keep_err = b,
                ("detach", v) => detach = v.as_boolean() == Some(true),
                (k, _) => eprintln!("abromux.exec: unknown option {k:?} ignored"),
            }
        }
    }
    if clear_env {
        cmd.env_clear();
    }
    for (k, v) in env {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    let piped = |keep| if keep { Stdio::piped() } else { Stdio::null() };
    if detach {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Its own process group: Ctrl-C on the script doesn't reach it.
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        return match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                // Reaped when it exits, so it doesn't linger as a zombie.
                std::thread::spawn(move || child.wait());
                let t = lua.create_table()?;
                t.set("pid", pid)?;
                Ok((Lv::Table(t), None))
            }
            Err(e) => Ok((Lv::Nil, Some(format!("{program}: {e}")))),
        };
    }
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(piped(keep_out))
    .stderr(piped(keep_err));
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Ok((Lv::Nil, Some(format!("{program}: {e}")))),
    };
    if let (Some(data), Some(mut w)) = (stdin, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = w.write_all(&data);
        });
    }
    let reader = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = vec![];
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = reader(child.stdout.take().map(|r| Box::new(r) as _));
    let err = reader(child.stderr.take().map(|r| Box::new(r) as _));
    let deadline = timeout.map(|t: f64| Instant::now() + Duration::from_secs_f64(t.max(0.)));
    let mut timed_out = false;
    let mut killed_at: Option<Instant> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => return Ok((Lv::Nil, Some(format!("{program}: {e}")))),
        }
        let now = Instant::now();
        if let Some(d) = deadline
            && now >= d
        {
            match killed_at {
                None => {
                    timed_out = true;
                    // SAFETY: signals the child we started.
                    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
                    killed_at = Some(now);
                }
                Some(k) if now.duration_since(k) >= Duration::from_secs(2) => {
                    let _ = child.kill();
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let code = status.code();
    let t = lua.create_table()?;
    t.set("code", code.unwrap_or(-1))?;
    t.set("signal", status.signal().map(signal_name))?;
    t.set("stdout", lua.create_string(&stdout)?)?;
    t.set("stderr", lua.create_string(&stderr)?)?;
    t.set("timed_out", timed_out)?;
    t.set("ok", code == Some(0) && !timed_out)?;
    Ok((Lv::Table(t), None))
}

/// One connection that reads every event, on a thread, from now on.
fn subscribe() -> Option<mpsc::Receiver<Value>> {
    let mut conn = UnixStream::connect(ipc::socket_path()).ok()?;
    let req = json!({ "id": 1, "method": "events.subscribe", "params": {} });
    writeln!(conn, "{req}").ok()?;
    let mut lines = BufReader::new(conn).lines();
    let ack: Value = serde_json::from_str(&lines.next()?.ok()?).ok()?;
    if ack["ok"] != true {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in lines {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    Some(rx)
}

fn in_scope(scope: &Option<String>, ev: &Value) -> bool {
    scope
        .as_deref()
        .is_none_or(|w| ev["workspace"].as_str().is_none_or(|e| e == w))
}

/// An event's name against `rex.on`-style `name`: exact, a prefix ending
/// in `.` or `*`, or `*` for all.
fn named(ev: &Value, name: &str) -> bool {
    crate::events::name_matches(ev["event"].as_str().unwrap_or_default(), name)
}

/// What a handler, `wait` or a timer is told about where it happened.
fn ctx_of(lua: &Lua, ev: &Value) -> mlua::Result<Table> {
    let ctx = lua.create_table()?;
    ctx.set("workspace", ev["workspace"].as_str())?;
    ctx.set("surface", ev["surface"].as_str())?;
    Ok(ctx)
}

fn keep_pending(st: &Shared, ev: Value) {
    let mut s = st.borrow_mut();
    if s.pending.len() >= MAX_PENDING {
        s.pending.pop_front();
    }
    s.pending.push_back(ev);
}

enum Filter {
    None,
    Fields(Value),
    Func(Function),
}

impl Filter {
    fn matches(&self, lua: &Lua, ev: &Value) -> mlua::Result<bool> {
        Ok(match self {
            Filter::None => true,
            Filter::Fields(Value::Object(want)) => want.iter().all(|(k, v)| &ev[k] == v),
            Filter::Fields(_) => true,
            Filter::Func(f) => f
                .call::<Lv>((to_lua(lua, ev)?, ctx_of(lua, ev)?))?
                .as_boolean()
                .unwrap_or(false),
        })
    }
}

/// `abromux.wait(event[, filter][, timeout])`.
fn wait(lua: &Lua, st: &Shared, args: MultiValue) -> mlua::Result<MultiValue> {
    let mut args = args.into_iter();
    let event = match args.next() {
        Some(Lv::String(s)) => s.to_string_lossy(),
        _ => return Err(rt("abromux.wait: expected an event name")),
    };
    let (mut filter, mut timeout) = (Filter::None, None);
    for a in args {
        match a {
            Lv::Table(t) => filter = Filter::Fields(from_lua(lua, Lv::Table(t))?),
            Lv::Function(f) => filter = Filter::Func(f),
            Lv::Integer(n) => timeout = Some(n as f64),
            Lv::Number(n) => timeout = Some(n),
            Lv::Nil => {}
            v => return Err(rt(format!("abromux.wait: unexpected {}", v.type_name()))),
        }
    }
    let found = |ev: Value| -> mlua::Result<MultiValue> {
        let ctx = ctx_of(lua, &ev)?;
        Ok(MultiValue::from_vec(vec![
            to_lua(lua, &ev)?,
            Lv::Table(ctx),
        ]))
    };
    // What came since the script started, oldest first.
    let mut i = 0;
    loop {
        let ev = {
            let s = st.borrow();
            match s.pending.get(i) {
                Some(ev) => ev.clone(),
                None => break,
            }
        };
        if named(&ev, &event) && filter.matches(lua, &ev)? {
            st.borrow_mut().pending.remove(i);
            return found(ev);
        }
        i += 1;
    }
    let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t.max(0.)));
    loop {
        let got = {
            let s = st.borrow();
            let Some(rx) = &s.events else {
                return Err(rt(
                    "abromux.wait: can't follow events (is abromux running?)",
                ));
            };
            match deadline {
                Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            }
        };
        match got {
            Ok(ev) => {
                if !in_scope(&st.borrow().scope, &ev) {
                    continue;
                }
                if named(&ev, &event) && filter.matches(lua, &ev)? {
                    return found(ev);
                }
                keep_pending(st, ev);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Ok(MultiValue::from_vec(vec![
                    Lv::Nil,
                    Lv::String(lua.create_string("timeout")?),
                ]));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                st.borrow_mut().events = None;
                return Ok(MultiValue::from_vec(vec![
                    Lv::Nil,
                    Lv::String(lua.create_string("abromux quit")?),
                ]));
            }
        }
    }
}

/// A Lua VM with the `abromux` table. `loading`: init.lua in the app, where
/// nothing may call out or end the process.
fn new_lua(st: &Shared, values: &[(&str, Value)]) -> mlua::Result<Lua> {
    let lua = Lua::new();
    let a = lua.create_table()?;
    lua.globals().set("abromux", &a)?;
    for (k, v) in values {
        a.set(*k, to_lua(&lua, v)?)?;
    }
    a.set("_namespaces", api::namespaces())?;
    // Where things are, for scripts that keep files of their own.
    a.set(
        "config_path",
        crate::settings::path().to_string_lossy().into_owned(),
    )?;
    a.set(
        "workspaces_dir",
        crate::channel::workspaces_dir()
            .to_string_lossy()
            .into_owned(),
    )?;
    let preload: Table = lua.globals().get::<Table>("package")?.get("preload")?;
    for (name, src) in MODULES {
        preload.set(
            *name,
            lua.load(*src)
                .set_name(format!("={name}"))
                .into_function()?,
        )?;
    }

    let s = st.clone();
    a.set(
        "call",
        lua.create_function(move |lua, (method, params): (String, Option<Lv>)| {
            if s.borrow().loading {
                return Ok((Lv::Nil, Some(format!(
                    "abromux.call({method:?}): not available while init.lua loads (call it from an action)"
                ))));
            }
            let mut p = match params {
                None | Some(Lv::Nil) => Value::Object(Map::new()),
                Some(v) => from_lua(lua, v)?,
            };
            let Value::Object(map) = &mut p else {
                return Err(rt(format!("{method}: the arguments must be a table")));
            };
            if !map.contains_key("caller")
                && let Some(c) = &s.borrow().caller
            {
                map.insert("caller".into(), json!({ "surface": c }));
            }
            match ipc::call(&method, p) {
                Ok(v) => Ok((to_lua(lua, &v)?, None)),
                Err(e) => Ok((Lv::Nil, Some(e.to_string()))),
            }
        })?,
    )?;

    a.set(
        "_log",
        lua.create_function(|_, (level, message): (Lv, Option<String>)| {
            let text = |v: &Lv| v.to_string().unwrap_or_else(|_| format!("{v:?}"));
            match message {
                None => eprintln!("{}", text(&level)),
                Some(m) => match text(&level).as_str() {
                    "info" => eprintln!("{m}"),
                    l => eprintln!("{l}: {m}"),
                },
            }
            Ok(())
        })?,
    )?;

    let s = st.clone();
    a.set(
        "exec",
        lua.create_function(move |lua, (argv, opts): (Vec<String>, Option<Table>)| {
            if s.borrow().loading {
                return Err(rt("abromux.exec: not available while init.lua loads"));
            }
            exec(lua, argv, opts)
        })?,
    )?;

    let s = st.clone();
    a.set(
        "sleep",
        lua.create_function(move |_, secs: f64| {
            if s.borrow().loading {
                return Err(rt("abromux.sleep: not available while init.lua loads"));
            }
            std::thread::sleep(Duration::from_secs_f64(secs.max(0.)));
            Ok(())
        })?,
    )?;

    let s = st.clone();
    a.set(
        "wait",
        lua.create_function(move |lua, args: MultiValue| {
            if s.borrow().loading {
                return Err(rt("abromux.wait: not available while init.lua loads"));
            }
            wait(lua, &s, args)
        })?,
    )?;

    let s = st.clone();
    a.set(
        "on",
        lua.create_function(move |lua, (event, f): (String, Function)| {
            let mut s = s.borrow_mut();
            if s.loading {
                return Err(rt(
                    "abromux.on: handlers only work in scripts run with `abromux do`",
                ));
            }
            if s.looping {
                return Err(rt(
                    "abromux.on: register handlers while the script first runs, not later",
                ));
            }
            s.handlers.push((event, lua.create_registry_value(f)?));
            Ok(())
        })?,
    )?;

    let s = st.clone();
    a.set(
        "_timer",
        lua.create_function(move |lua, (secs, f, repeat): (f64, Function, bool)| {
            let mut s = s.borrow_mut();
            if s.loading {
                return Err(rt("abromux timers: not available while init.lua loads"));
            }
            if repeat && secs <= 0. {
                return Err(rt("abromux.every: the interval must be more than zero"));
            }
            if s.timers.len() >= MAX_TIMERS {
                eprintln!(
                    "Error: more than {MAX_TIMERS} timers at once (is a handler making one per event?)"
                );
                std::process::exit(1);
            }
            let d = Duration::from_secs_f64(secs.max(0.));
            s.next_timer += 1;
            let id = s.next_timer;
            s.timers.push(Timer {
                id,
                due: Instant::now() + d,
                every: repeat.then_some(d),
                f: lua.create_registry_value(f)?,
            });
            Ok(id)
        })?,
    )?;
    let s = st.clone();
    a.set(
        "_cancel",
        lua.create_function(move |_, id: u64| {
            s.borrow_mut().timers.retain(|t| t.id != id);
            Ok(())
        })?,
    )?;
    let s = st.clone();
    a.set(
        "_active",
        lua.create_function(move |_, id: u64| Ok(s.borrow().timers.iter().any(|t| t.id == id)))?,
    )?;

    let s = st.clone();
    a.set(
        "stop",
        lua.create_function(move |_, code: Option<i32>| -> mlua::Result<()> {
            if s.borrow().loading {
                return Err(rt("abromux.stop: not available while init.lua loads"));
            }
            let _ = std::io::stdout().flush();
            std::process::exit(code.unwrap_or(0))
        })?,
    )?;

    let json_t = lua.create_table()?;
    json_t.set(
        "encode",
        lua.create_function(|lua, (v, opts): (Lv, Option<Table>)| {
            let indent = opts
                .map(|o| o.get::<Option<bool>>("indent"))
                .transpose()?
                .flatten()
                .unwrap_or(false);
            let v = match from_lua(lua, v) {
                Ok(v) => v,
                Err(e) => return Ok((None, Some(error_text(&e)))),
            };
            let text = if indent {
                serde_json::to_string_pretty(&v)
            } else {
                serde_json::to_string(&v)
            };
            Ok((text.ok(), None))
        })?,
    )?;
    json_t.set(
        "decode",
        lua.create_function(
            |lua, text: String| match serde_json::from_str::<Value>(&text) {
                Ok(v) => Ok((to_lua(lua, &v)?, None)),
                Err(e) => Ok((Lv::Nil, Some(e.to_string()))),
            },
        )?,
    )?;
    // An array even when empty (encode writes `[]`, not `{}`).
    json_t.set(
        "array",
        lua.create_function(|lua, t: Option<Table>| {
            let t = match t {
                Some(t) => t,
                None => lua.create_table()?,
            };
            t.set_metatable(Some(lua.array_metatable()));
            Ok(t)
        })?,
    )?;
    a.set("json", json_t)?;

    lua.load(PRELUDE).set_name("=abromux").exec()?;
    Ok(lua)
}

/// Let `require` find files next to `file`.
fn add_require_path(lua: &Lua, file: &Path) -> mlua::Result<()> {
    let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) else {
        return Ok(());
    };
    let package: Table = lua.globals().get("package")?;
    let path: String = package.get("path")?;
    let dir = dir.to_string_lossy();
    package.set("path", format!("{dir}/?.lua;{dir}/?/init.lua;{path}"))
}

/// A file's modification time and size, or None if it isn't there.
type Stamp = Option<(std::time::SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The last load of each actions file: the files it read (itself and what
/// it `require`d, or would have) as they were then, and what it gave.
type Loaded = (Vec<(PathBuf, Stamp)>, Result<Vec<ActionDef>, String>);

fn loads() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, Loaded>> {
    static LOADS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, Loaded>>,
    > = std::sync::OnceLock::new();
    LOADS.get_or_init(Default::default)
}

/// Load init.lua (or another actions file) the way the app does: nothing
/// can call out, `os.exit` can't end the app, and it has `LOAD_LIMIT` to
/// finish. The actions it defines, in order; none if there's no file.
pub fn collect(path: &Path) -> Result<Vec<ActionDef>, String> {
    let mut files = vec![path.to_path_buf()];
    let result = load_actions(path, &mut files);
    let stamps = files.into_iter().map(|f| {
        let s = stamp(&f);
        (f, s)
    });
    if let Ok(mut l) = loads().lock() {
        l.insert(path.to_path_buf(), (stamps.collect(), result.clone()));
    }
    result
}

/// [`collect`], or what it gave last time if none of the files it read has
/// changed since: for checking the file often (the Settings window shows
/// its error) without running it each time.
pub fn collect_cached(path: &Path) -> Result<Vec<ActionDef>, String> {
    if let Ok(l) = loads().lock()
        && let Some((files, result)) = l.get(path)
        && files.iter().all(|(f, s)| stamp(f) == *s)
    {
        return result.clone();
    }
    collect(path)
}

/// [`collect`]'s work. `files` gets each file `require` looked for in
/// init.lua's folder, found or not.
fn load_actions(path: &Path, files: &mut Vec<PathBuf>) -> Result<Vec<ActionDef>, String> {
    let Ok(src) = std::fs::read_to_string(path) else {
        return Ok(vec![]);
    };
    let at = |e: &mlua::Error| format!("{}: {}", path.display(), error_text(e));
    let st: Shared = Rc::new(RefCell::new(State {
        loading: true,
        ..Default::default()
    }));
    let lua = new_lua(&st, &[("args", json!({}))]).map_err(|e| at(&e))?;
    (|| -> mlua::Result<()> {
        let os: Table = lua.globals().get("os")?;
        os.set(
            "exit",
            lua.create_function(|_, ()| -> mlua::Result<()> {
                Err(rt("os.exit: not available while init.lua loads"))
            })?,
        )?;
        add_require_path(&lua, path)
    })()
    .map_err(|e| at(&e))?;
    let required = lua.create_table().map_err(|e| at(&e))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        // Ahead of Lua's own searcher: note where the module is (or would
        // be, in init.lua's folder) and let that one load it.
        lua.load(
            r#"local seen, dir = ...
            table.insert(package.searchers, 2, function(name)
              seen[#seen + 1] = package.searchpath(name, package.path)
                or (dir .. "/" .. name:gsub("%.", "/") .. ".lua")
              return nil
            end)"#,
        )
        .call::<()>((&required, dir.to_string_lossy().into_owned()))
        .map_err(|e| at(&e))?;
    }
    let start = Instant::now();
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(10_000),
        move |_, _| {
            if start.elapsed() > LOAD_LIMIT {
                Err(rt(format!(
                    "took longer than {}s to load",
                    LOAD_LIMIT.as_secs()
                )))
            } else {
                Ok(VmState::Continue)
            }
        },
    );
    let ran = lua
        .load(&src)
        .set_name(format!("@{}", path.display()))
        .exec();
    lua.remove_hook();
    files.extend(
        required
            .sequence_values::<String>()
            .flatten()
            .map(PathBuf::from),
    );
    ran.map_err(|e| at(&e))?;
    actions_of(&lua).map_err(|e| at(&e))
}

fn actions_of(lua: &Lua) -> mlua::Result<Vec<ActionDef>> {
    let a: Table = lua.globals().get("abromux")?;
    let order: Vec<String> = a.get("_order")?;
    let defs: Table = a.get("_actions")?;
    order
        .into_iter()
        .map(|name| {
            let d: Table = defs.get(name.as_str())?;
            let keywords = match d.get::<Lv>("keywords")? {
                Lv::String(s) => s
                    .to_string_lossy()
                    .split_whitespace()
                    .map(String::from)
                    .collect(),
                Lv::Table(t) => t.sequence_values::<String>().collect::<mlua::Result<_>>()?,
                _ => vec![],
            };
            let key = match d.get::<Lv>("key")? {
                Lv::Nil => None,
                v => Some(from_lua(lua, v)?),
            };
            Ok(ActionDef {
                title: d.get("title")?,
                description: d.get("description")?,
                keywords,
                key,
                palette: d.get::<Option<bool>>("palette")?.unwrap_or(true),
                name,
            })
        })
        .collect()
}

/// What `abromux do` runs.
enum Source {
    File(PathBuf),
    Inline(String),
    Stdin,
}

/// `abromux do`: run a script, or an action from init.lua. Returns the exit
/// status.
pub fn run_do(
    mut pos: Vec<String>,
    opts: &Map<String, Value>,
    as_json: bool,
) -> Result<i32, String> {
    let opt = |k: &str| opts.get(k).and_then(Value::as_str);
    if opts.contains_key("list") {
        let path = pos.first().map(|p| tilde(p)).unwrap_or_else(init_path);
        if !path.is_file() {
            return Err(format!("{}: no such file", path.display()));
        }
        let actions = collect(&path)?;
        if as_json {
            let list: Vec<Value> = actions
                .iter()
                .map(|a| {
                    json!({ "name": a.name, "title": a.title, "description": a.description,
                            "keywords": a.keywords, "key": a.key })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&list).unwrap_or_default()
            );
        } else {
            for a in &actions {
                let key = a.key.as_ref().map(|k| match k {
                    Value::String(s) => s.clone(),
                    k => k.to_string(),
                });
                println!(
                    "{:<24} {}{}",
                    a.name,
                    a.title,
                    key.map(|k| format!("  ({k})")).unwrap_or_default()
                );
            }
        }
        return Ok(0);
    }
    if pos.is_empty() {
        return Err("do: missing <script.lua>, -e <code>, - (stdin) or an action name".into());
    }
    let first = pos.remove(0);
    let mut action = opt("action").map(String::from);
    let source = if first == "-e" {
        if pos.is_empty() {
            return Err("do -e: missing the code".into());
        }
        Source::Inline(pos.remove(0))
    } else if first == "-" {
        Source::Stdin
    } else if first.ends_with(".lua") || first.contains('/') {
        Source::File(tilde(&first))
    } else {
        // A name: that action of init.lua.
        action = Some(first);
        Source::File(init_path())
    };
    let mut args = Map::new();
    if let Some(a) = opt("args") {
        let text = match a.strip_prefix('@') {
            Some(f) => std::fs::read_to_string(tilde(f)).map_err(|e| format!("--args {f}: {e}"))?,
            None => a.to_string(),
        };
        match serde_json::from_str(&text).map_err(|e| format!("--args: {e}"))? {
            Value::Object(m) => args.extend(m),
            _ => return Err("--args: expected a JSON object".into()),
        }
    }
    // `key=value`, and plain words as `args[1]`, `args[2]`…
    let mut words = vec![];
    for a in &pos {
        match parse_arg(a) {
            Ok((k, v)) => {
                args.insert(k, v);
            }
            Err(_) => words.push(a.clone()),
        }
    }
    let until = opt("for").map(parse_duration).transpose()?;
    let (src, name, file) = match source {
        Source::File(p) => {
            let src = std::fs::read_to_string(&p).map_err(|e| match (&action, e.kind()) {
                (Some(a), std::io::ErrorKind::NotFound) if first_is_action(&p) => {
                    format!("no action {a:?}: {} doesn't exist", p.display())
                }
                _ => format!("{}: {e}", p.display()),
            })?;
            (src, format!("@{}", p.display()), Some(p))
        }
        Source::Inline(code) => (code, "=-e".to_string(), None),
        Source::Stdin => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| format!("stdin: {e}"))?;
            (s, "=stdin".to_string(), None)
        }
    };

    let surface = std::env::var("ABROMUX_SURFACE_ID").ok();
    let workspace = std::env::var("ABROMUX_WORKSPACE_ID").ok();
    let st: Shared = Rc::new(RefCell::new(State {
        caller: surface.clone(),
        scope: workspace.clone().filter(|_| !opts.contains_key("all")),
        fail_fast: opts.contains_key("fail_fast"),
        // Before the script runs, so `wait` sees what follows its calls.
        events: subscribe(),
        ..Default::default()
    }));
    let lua = new_lua(
        &st,
        &[
            ("args", Value::Object(args.clone())),
            ("surface_id", json!(surface)),
            ("workspace_id", json!(workspace)),
            ("script", json!(file.as_ref().map(|f| f.to_string_lossy()))),
        ],
    )
    .map_err(|e| error_text(&e))?;
    let lua_args: Table = lua
        .globals()
        .get::<Table>("abromux")
        .and_then(|a| a.get("args"))
        .map_err(|e| error_text(&e))?;
    for (i, w) in words.iter().enumerate() {
        lua_args
            .raw_set(i + 1, w.as_str())
            .map_err(|e| error_text(&e))?;
    }
    if let Some(f) = &file {
        add_require_path(&lua, f).map_err(|e| error_text(&e))?;
    }
    let result = lua
        .load(&src)
        .set_name(name)
        .eval::<MultiValue>()
        .map_err(|e| error_text(&e))?;
    let result = match &action {
        None => result.into_iter().next().unwrap_or(Lv::Nil),
        Some(a) => {
            let origin = std::env::var("ABROMUX_ACTION_ORIGIN").unwrap_or_else(|_| "cli".into());
            let run: Function = lua
                .globals()
                .get::<Table>("abromux")
                .and_then(|t| t.get("_run_action"))
                .map_err(|e| error_text(&e))?;
            let out = run
                .call::<MultiValue>((a.as_str(), lua_args, origin))
                .map_err(|e| error_text(&e))?;
            let mut out = out.into_iter();
            let (r, err) = (out.next().unwrap_or(Lv::Nil), out.next());
            if let (Lv::Nil, Some(Lv::String(e))) = (&r, &err) {
                return Err(e.to_string_lossy());
            }
            // `true` from an action means done, nothing to show.
            if r == Lv::Boolean(true) { Lv::Nil } else { r }
        }
    };
    if result != Lv::Nil {
        let v = from_lua(&lua, result).map_err(|e| format!("the result: {}", error_text(&e)))?;
        let text = if as_json {
            serde_json::to_string_pretty(&v)
        } else {
            serde_json::to_string(&v)
        };
        println!("{}", text.unwrap_or_default());
        let _ = std::io::stdout().flush();
    }
    event_loop(&lua, &st, until.map(|d| Instant::now() + d))
}

/// `abromux <name> …` for a name that isn't one of the CLI's commands: if
/// init.lua has an action of that name, run it as `abromux do` would, with
/// `--key value` options as args too. None if it has no such action.
pub fn run_cli_action(
    name: &str,
    pos: Vec<String>,
    opts: &Map<String, Value>,
    as_json: bool,
) -> Option<Result<i32, String>> {
    let init = init_path();
    if !init.is_file() {
        return None;
    }
    match collect(&init) {
        Ok(actions) if actions.iter().any(|a| a.name == name) => {}
        Ok(_) => return None,
        Err(e) => {
            return Some(Err(format!(
                "{name}: not a command, and init.lua (which may define it) has an error: {e}"
            )));
        }
    }
    let mut words = vec![name.to_string()];
    words.extend(pos);
    for (k, v) in opts {
        words.push(match v {
            Value::String(s) => format!("{k}={s}"),
            v => format!("{k}={v}"),
        });
    }
    Some(run_do(words, &Map::new(), as_json))
}

/// A missing init.lua when an action was named.
fn first_is_action(p: &Path) -> bool {
    p == init_path()
}

/// Run handlers and timers until there are none, the time's up, or the app
/// quits. Returns the exit status.
fn event_loop(lua: &Lua, st: &Shared, until: Option<Instant>) -> Result<i32, String> {
    st.borrow_mut().looping = true;
    let (handlers, timers) = {
        let s = st.borrow();
        (s.handlers.len(), s.timers.len())
    };
    if handlers + timers == 0 {
        return Ok(0);
    }
    if handlers > 0 && st.borrow().events.is_none() {
        return Err("can't follow events (is abromux running?)".into());
    }
    // SAFETY: isatty only reads the descriptor's state.
    if unsafe { libc::isatty(2) } == 1 {
        let scope = st
            .borrow()
            .scope
            .clone()
            .unwrap_or_else(|| "every workspace".into());
        let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        let mut parts = vec![];
        if handlers > 0 {
            parts.push(format!("{}, {scope}", plural(handlers, "handler")));
        }
        if timers > 0 {
            parts.push(plural(timers, "timer"));
        }
        eprintln!("listening: {}; Ctrl-C to stop", parts.join("; "));
    }
    let report = |e: mlua::Error, fail_fast: bool| {
        eprintln!("Error: {}", error_text(&e));
        if fail_fast {
            std::process::exit(1);
        }
    };
    loop {
        let fail_fast = st.borrow().fail_fast;
        let now = Instant::now();
        if until.is_some_and(|u| now >= u) {
            return Ok(0);
        }
        // A timer that's due.
        let due = {
            let s = st.borrow();
            s.timers
                .iter()
                .filter(|t| t.due <= now)
                .min_by_key(|t| t.due)
                .map(|t| t.id)
        };
        if let Some(id) = due {
            let f = {
                let mut s = st.borrow_mut();
                let i = s.timers.iter().position(|t| t.id == id).unwrap();
                let f: Function = lua
                    .registry_value(&s.timers[i].f)
                    .map_err(|e| error_text(&e))?;
                match s.timers[i].every {
                    Some(every) => s.timers[i].due = Instant::now() + every,
                    None => {
                        s.timers.remove(i);
                    }
                }
                f
            };
            let caller = st.borrow().caller.clone();
            let ctx = lua.create_table().map_err(|e| error_text(&e))?;
            let _ = ctx.set("surface", caller);
            let _ = ctx.set("workspace", st.borrow().scope.clone());
            let _ = ctx.set("origin", "timer");
            if let Err(e) = f.call::<()>(ctx) {
                report(e, fail_fast);
            }
            continue;
        }
        // An event that's waiting.
        let ev = st.borrow_mut().pending.pop_front();
        if let Some(ev) = ev {
            dispatch(lua, st, &ev, fail_fast).map_err(|e| error_text(&e))?;
            continue;
        }
        let (handlers, next_timer, listening) = {
            let s = st.borrow();
            (
                s.handlers.len(),
                s.timers.iter().map(|t| t.due).min(),
                s.events.is_some(),
            )
        };
        if next_timer.is_none() && (handlers == 0 || !listening) {
            return Ok(0);
        }
        let limit = [next_timer, until].into_iter().flatten().min();
        let got = {
            let s = st.borrow();
            match (&s.events, limit) {
                (Some(rx), Some(l)) if handlers > 0 => {
                    rx.recv_timeout(l.saturating_duration_since(Instant::now()))
                }
                (Some(rx), None) if handlers > 0 => {
                    rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                }
                _ => {
                    drop(s);
                    if let Some(l) = limit {
                        std::thread::sleep(l.saturating_duration_since(Instant::now()));
                    }
                    continue;
                }
            }
        };
        match got {
            Ok(ev) => {
                if in_scope(&st.borrow().scope, &ev) {
                    keep_pending(st, ev);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // The app quit: handlers have nothing more to hear.
                st.borrow_mut().events = None;
            }
        }
    }
}

/// Call each handler for `ev`'s name, with API calls defaulting to the
/// event's tab.
fn dispatch(lua: &Lua, st: &Shared, ev: &Value, fail_fast: bool) -> mlua::Result<()> {
    let fns: Vec<Function> = {
        let s = st.borrow();
        s.handlers
            .iter()
            .filter(|(name, _)| named(ev, name))
            .map(|(_, k)| lua.registry_value(k))
            .collect::<mlua::Result<_>>()?
    };
    if fns.is_empty() {
        return Ok(());
    }
    let own = st.borrow().caller.clone();
    if let Some(s) = ev["surface"].as_str() {
        st.borrow_mut().caller = Some(s.to_string());
    }
    for f in fns {
        let r = f.call::<()>((ctx_of(lua, ev)?, to_lua(lua, ev)?));
        if let Err(e) = r {
            eprintln!("Error: {}", error_text(&e));
            if fail_fast {
                std::process::exit(1);
            }
        }
    }
    st.borrow_mut().caller = own;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(src: &str) -> (Lua, Shared) {
        let st: Shared = Rc::new(RefCell::new(State::default()));
        let lua = new_lua(&st, &[("args", json!({ "name": "rex" }))]).unwrap();
        lua.load(src).exec().unwrap();
        (lua, st)
    }

    fn eval(src: &str) -> Value {
        let st: Shared = Rc::new(RefCell::new(State::default()));
        let lua = new_lua(&st, &[("args", json!({ "name": "rex" }))]).unwrap();
        let v: Lv = lua.load(src).eval().unwrap();
        from_lua(&lua, v).unwrap()
    }

    #[test]
    fn args_are_typed() {
        assert_eq!(parse_arg("n=2").unwrap(), ("n".into(), json!(2)));
        assert_eq!(parse_arg("r=0.5").unwrap(), ("r".into(), json!(0.5)));
        assert_eq!(parse_arg("on=true").unwrap(), ("on".into(), json!(true)));
        assert_eq!(parse_arg("s=a=b").unwrap(), ("s".into(), json!("a=b")));
        assert_eq!(parse_arg("e=").unwrap(), ("e".into(), json!("")));
        assert!(parse_arg("nope").is_err());
        assert!(parse_arg("=x").is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_duration("1.5h").unwrap(), Duration::from_secs(5400));
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("-1").is_err());
    }

    #[test]
    fn json_round_trip() {
        assert_eq!(
            eval(r#"return abromux.json.decode('{"a":[1,2],"b":null,"c":{}}')"#),
            json!({ "a": [1, 2], "c": {} })
        );
        assert_eq!(
            eval(r#"return (abromux.json.encode({ name = "rex", tabs = { 1, 2 } }))"#),
            json!(r#"{"name":"rex","tabs":[1,2]}"#)
        );
        assert_eq!(eval("return {}"), json!({}));
        assert_eq!(eval("return abromux.args.name"), json!("rex"));
    }

    #[test]
    fn namespaces_call_methods() {
        // Loading, so nothing reaches a running app: the call is refused,
        // naming the method the namespace made of it.
        let st: Shared = Rc::new(RefCell::new(State {
            loading: true,
            ..Default::default()
        }));
        let lua = new_lua(&st, &[]).unwrap();
        lua.load("r, err = abromux.surface.split{ direction = 'right' }")
            .exec()
            .unwrap();
        assert_eq!(lua.globals().get::<Lv>("r").unwrap(), Lv::Nil);
        assert!(
            lua.globals()
                .get::<String>("err")
                .unwrap()
                .contains("\"surface.split\"")
        );
        // `log` is a function and the sidebar log's namespace.
        let (lua, _) = script("f = type(abromux.log.add); abromux.log('quiet')");
        assert_eq!(lua.globals().get::<String>("f").unwrap(), "function");
        // Values that aren't set stay nil, not a namespace.
        assert_eq!(eval("return abromux.surface_id == nil"), json!(true));
    }

    #[test]
    fn exec_runs_without_a_shell() {
        assert_eq!(
            eval(
                r#"local r = abromux.exec{ "printf", "%s", "a b" }; return { r.ok, r.code, r.stdout }"#
            ),
            json!([true, 0, "a b"])
        );
        assert_eq!(
            eval(
                r#"local r = abromux.exec({ "sh", "-c", "cat; exit 3" }, { stdin = "in" }); return { r.ok, r.code, r.stdout }"#
            ),
            json!([false, 3, "in"])
        );
        assert_eq!(
            eval(
                r#"local r = abromux.exec({ "sh", "-c", "echo $X" }, { env = { X = "y" } }); return r.stdout"#
            ),
            json!("y\n")
        );
        assert_eq!(
            eval(
                r#"local r = abromux.exec({ "sleep", "5" }, { timeout = 0.1 }); return { r.timed_out, r.signal, r.ok }"#
            ),
            json!([true, "SIGTERM", false])
        );
        assert_eq!(
            eval(
                r#"local r, err = abromux.exec{ "no-such-program-here" }; return { r == nil, err ~= nil }"#
            ),
            json!([true, true])
        );
    }

    #[test]
    fn timers_and_actions() {
        let (lua, st) = script(
            r#"
            n = 0
            t = abromux.every(0.01, function() n = n + 1; if n == 3 then t:cancel() end end)
            abromux.after(0, function() done = true end)
            "#,
        );
        assert_eq!(event_loop(&lua, &st, None), Ok(0));
        assert_eq!(lua.globals().get::<i64>("n").unwrap(), 3);
        assert!(lua.globals().get::<bool>("done").unwrap());

        let v = eval(
            r#"
            abromux.action{ name = "greet", title = "Greet", args = { name = "string", n = "integer?" },
                            run = function(ctx, a) return "hello " .. a.name .. (a.n or "") end }
            local ok = abromux.invoke("greet", { name = 7 })
            local _, missing = abromux.invoke("greet", {})
            local _, bogus = abromux.invoke("greet", { name = "x", bogus = 1 })
            local _, wrong = abromux.invoke("greet", { name = "x", n = 1.5 })
            local _, none = abromux.invoke("nope")
            return { ok, missing, bogus, wrong, none }
            "#,
        );
        assert_eq!(
            v,
            json!([
                "hello 7",
                "missing argument \"name\"",
                "no argument named \"bogus\"; greet takes n?, name",
                "argument \"n\" should be integer, got number",
                "no action named nope"
            ])
        );
    }

    #[test]
    fn collecting_init_lua() {
        let dir = std::env::temp_dir().join(format!("abromux-lua-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let init = dir.join("init.lua");
        std::fs::write(dir.join("more.lua"), "return 'Tests'").unwrap();
        std::fs::write(
            &init,
            r#"
            local title = require("more")
            -- Calls fail rather than run while loading.
            local r, err = abromux.workspace.list{}
            assert(r == nil and err:find("init.lua loads"))
            assert(not pcall(os.exit, 1))
            abromux.action{ name = "tests", title = title, key = "cmd-alt-t",
                            keywords = "check verify", run = function() end }
            abromux.action{ name = "deploy", title = "Deploy", description = "Ship it",
                            keywords = { "ship" }, run = function() end }
            "#,
        )
        .unwrap();
        let actions = collect(&init).unwrap();
        assert_eq!(
            actions,
            [
                ActionDef {
                    name: "tests".into(),
                    title: "Tests".into(),
                    description: None,
                    keywords: vec!["check".into(), "verify".into()],
                    key: Some(json!("cmd-alt-t")),
                    palette: true,
                },
                ActionDef {
                    name: "deploy".into(),
                    title: "Deploy".into(),
                    description: Some("Ship it".into()),
                    keywords: vec!["ship".into()],
                    key: None,
                    palette: true,
                },
            ]
        );

        std::fs::write(&init, "while true do end").unwrap();
        assert!(collect(&init).unwrap_err().contains("took longer"));
        std::fs::write(&init, "abromux.action{ name = 'Bad Name' }").unwrap();
        let e = collect(&init).unwrap_err();
        assert!(e.contains("init.lua:1:") && e.contains("lowercase"), "{e}");
        assert_eq!(collect(&dir.join("missing.lua")).unwrap(), []);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_folders_module() {
        let base = std::env::temp_dir()
            .join(format!("abromux-wsf-{}", std::process::id()))
            .canonicalize()
            .unwrap_or_else(|_| {
                let d = std::env::temp_dir().join(format!("abromux-wsf-{}", std::process::id()));
                std::fs::create_dir_all(&d).unwrap();
                d.canonicalize().unwrap()
            });
        let b = base.to_string_lossy();
        // A repo with a commit, and a plain folder.
        let sh = |cmd: &str| {
            let ok = std::process::Command::new("sh")
                .args(["-c", cmd])
                .current_dir(&base)
                .status()
                .unwrap()
                .success();
            assert!(ok, "{cmd}");
        };
        sh(
            "mkdir -p repo plain && echo hi > plain/a.txt && cd repo && git init -q -b main \
            && git -c user.name=t -c user.email=t@t commit -q --allow-empty -m init",
        );
        let r = eval(&format!(
            r#"local wf = require "abromux.workspace_folders"
            local base = "{b}"
            local out = {{ slug = wf.slug("Fix: the Login bug!"), empty = wf.slug("!!") }}
            local ws = wf.create(base .. "/ws", "My Task")
            local again = wf.create(base .. "/ws", "my task")
            out.dirs = {{ ws:sub(#base + 2), again:sub(#base + 2) }}
            out.meta = wf.read_meta(ws)
            out.agents = io.open(ws .. "/AGENTS.md"):read("a") == wf.AGENTS_MD
            out.copy = wf.add(ws, base .. "/plain")
            out.tree = wf.add(ws, base .. "/repo")
            out.listed = #wf.read_meta(ws).checkouts
            out.json = io.open(again .. "/.abromux/workspace.json"):read("a")
            -- A change in the worktree: remove refuses, unless forced.
            io.open(ws .. "/repo/new.txt", "w"):write("x")
            out.blockers = wf.blockers(ws)
            out.refused = select(2, pcall(wf.remove, ws, false))
            out.removed = wf.remove(ws, true)
            out.gone = io.open(ws .. "/AGENTS.md") == nil
            out.branch_kept = abromux.exec({{ "git", "-C", base .. "/repo", "branch", "--list", "ws/my-task" }}).stdout
            out.listed_after = #wf.list(base .. "/ws")
            return out"#
        ));
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(r["slug"], "fix-the-login-bug");
        assert_eq!(r["empty"], "workspace");
        assert_eq!(r["dirs"], json!(["ws/my-task", "ws/my-task-2"]));
        assert_eq!(r["meta"]["name"], "My Task");
        assert_eq!(r["agents"], true);
        assert_eq!(
            r["copy"],
            json!({ "path": "plain", "source": format!("{b}/plain") })
        );
        assert_eq!(
            r["tree"],
            json!({ "path": "repo", "source": format!("{b}/repo"), "branch": "ws/my-task", "new_branch": true })
        );
        assert_eq!(r["listed"], 2);
        assert!(
            r["json"].as_str().unwrap().contains(r#""checkouts": []"#),
            "{}",
            r["json"]
        );
        assert_eq!(
            r["blockers"],
            json!(["repo: 1 uncommitted or untracked file(s)"])
        );
        assert!(
            r["refused"]
                .as_str()
                .unwrap()
                .contains("work would be lost"),
            "{}",
            r["refused"]
        );
        assert_eq!(
            r["removed"].as_array().unwrap().len(),
            3,
            "{}",
            r["removed"]
        );
        assert!(
            r["removed"][1]
                .as_str()
                .unwrap()
                .contains("deleted branch ws/my-task (merged)"),
            "{}",
            r["removed"]
        );
        assert_eq!(r["gone"], true);
        assert_eq!(r["branch_kept"], "");
        assert_eq!(r["listed_after"], 1);
    }

    #[test]
    fn collect_cached_runs_only_after_a_change() {
        let dir = std::env::temp_dir().join(format!("abromux-lua-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let init = dir.join("init.lua");
        let runs = dir.join("runs");
        let titles = |r: Result<Vec<ActionDef>, String>| -> Vec<String> {
            r.unwrap().into_iter().map(|a| a.title).collect()
        };
        std::fs::write(
            &init,
            format!(
                "local f = io.open({:?}, 'a'); f:write('x'); f:close()\n\
                 local more = require 'more'\n\
                 abromux.action{{ name = 'a', title = more.title, run = print }}",
                runs.to_string_lossy()
            ),
        )
        .unwrap();
        // A module that isn't there yet: init.lua fails until it is.
        assert!(collect_cached(&init).is_err());
        assert!(collect_cached(&init).is_err());
        assert_eq!(std::fs::read_to_string(&runs).unwrap(), "x");
        std::fs::write(dir.join("more.lua"), "return { title = 'One' }").unwrap();
        assert_eq!(titles(collect_cached(&init)), ["One"]);
        assert_eq!(titles(collect_cached(&init)), ["One"]);
        assert_eq!(std::fs::read_to_string(&runs).unwrap(), "xx");
        // Editing the module (not init.lua) runs it again.
        std::fs::write(dir.join("more.lua"), "return { title = 'Second' }").unwrap();
        assert_eq!(titles(collect_cached(&init)), ["Second"]);
        assert_eq!(std::fs::read_to_string(&runs).unwrap(), "xxx");
        // `collect` always runs it.
        assert_eq!(titles(collect(&init)), ["Second"]);
        assert_eq!(std::fs::read_to_string(&runs).unwrap(), "xxxx");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
