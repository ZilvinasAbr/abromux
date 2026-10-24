//! `abromux <command>`: drive a running abromux over its control socket. Command
//! names and flags follow cmux's CLI where the two overlap.

use crate::ipc;
use serde_json::{Map, Value, json};
use std::{io::IsTerminal, path::Path};

const HELP: &str = "abromux - control abromux via Unix socket

Usage:
  abromux                       Launch the app (from AbroMux.app: focus it if running)
  abromux <path>                Open a directory in a new workspace (launches abromux if needed)
  abromux [--json] [--socket <path>] <command> [options]

Targets:
  --workspace, --pane and --surface take a ref (workspace:2, pane:5, surface:3)
  or a 0-based index. Without one, commands act on the terminal abromux runs in
  ($ABROMUX_SURFACE_ID), else on whatever is focused.
  --json prints the raw JSON result instead of text.

Commands:
  ping | version | identify
  list-workspaces
  current-workspace
  new-workspace [--name <title>] [--cwd <path>] [--command <text>] [--focus <true|false>]
  select-workspace --workspace <id|ref|index>
  close-workspace --workspace <id|ref|index>
  rename-workspace [--workspace <id|ref|index>] <title>
  tree [--all] [--workspace <id|ref|index>]
  list-panes [--workspace <id|ref|index>]
  focus-pane --pane <id|ref|index> [--workspace <id|ref|index>]
  new-pane [--type <terminal|browser>] [--direction <left|right|up|down>] [--workspace <ref>] [--url <url>] [--command <text>] [--focus <true|false>]
  new-split <left|right|up|down> [--surface <ref>] [--workspace <ref>] [--type <terminal|browser>] [--url <url>] [--command <text>] [--focus <true|false>]
  list-pane-surfaces [--workspace <ref>] [--pane <ref>]
  new-surface [--type <terminal|browser>] [--pane <ref>] [--workspace <ref>] [--url <url>] [--command <text>] [--focus <true|false>]
  close-surface [--surface <ref>] [--workspace <ref>]
  focus-surface --surface <ref>
  send [--surface <ref>] [--workspace <ref>] <text>          (\\n presses enter; also \\t \\e \\\\)
  send-key [--surface <ref>] [--workspace <ref>] <key>       (enter, tab, escape, up, ctrl-c, alt-b, ...)
  read-screen [--surface <ref>] [--workspace <ref>] [--scrollback] [--lines <n>]
  capture-pane                                               (alias for read-screen)
  clear-history [--surface <ref>] [--workspace <ref>]        (clear screen and scrollback, like ⌘K)
  open <path-or-url>...                                      (directories open workspaces, URLs open browser splits)
  focus-window [--window <ref>]                              (bring abromux to the front; there is one window)
  sidebar <toggle|show|hide>
  quit
  rpc <method> [json-params]

  browser [--surface <ref>] <subcommand> ...
  browser open [url] [--direction <dir>] [--focus <true|false>]   (browser split next to the caller)
  browser open-split [url]
  browser goto|navigate <url>
  browser back|forward|reload
  browser url|get-url
  browser eval <script>
";

/// Flags that take no value.
const SWITCHES: &[&str] = &["json", "scrollback", "all", "help"];

struct Args {
    pos: Vec<String>,
    opts: Map<String, Value>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let (mut pos, mut opts) = (vec![], Map::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            pos.extend(it.by_ref().cloned());
            break;
        }
        match a.strip_prefix("--") {
            Some("no-focus") => {
                opts.insert("focus".into(), json!(false));
            }
            Some(k) if SWITCHES.contains(&k) => {
                opts.insert(k.into(), json!(true));
            }
            Some(k) => {
                let v = it.next().ok_or_else(|| format!("--{k} needs a value"))?;
                opts.insert(k.replace('-', "_"), json!(v));
            }
            None if a == "-h" => {
                opts.insert("help".into(), json!(true));
            }
            None => pos.push(a.clone()),
        }
    }
    Ok(Args { pos, opts })
}

pub fn run(argv: Vec<String>) -> i32 {
    let Args { mut pos, mut opts } = match parse(&argv) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    if let Some(Value::String(s)) = opts.remove("socket") {
        // SAFETY: single-threaded at this point; read back by ipc::socket_path.
        unsafe { std::env::set_var("ABROMUX_SOCKET_PATH", s) };
    }
    let as_json = opts.remove("json").is_some();
    if pos.is_empty() || opts.contains_key("help") || pos[0] == "help" {
        print!("{HELP}");
        return 0;
    }
    if let Ok(s) = std::env::var("ABROMUX_SURFACE_ID") {
        opts.insert("caller".into(), json!({ "surface": s }));
    }
    let cmd = pos.remove(0);
    if pos.is_empty() && Path::new(&cmd).is_dir() {
        return open_dir(&cmd);
    }
    match dispatch(&cmd, pos, opts, as_json) {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

fn fail(e: impl std::fmt::Display) -> i32 {
    eprintln!("abromux: {e}");
    1
}

fn call(method: &str, params: Map<String, Value>) -> Result<Value, String> {
    ipc::call(method, Value::Object(params)).map_err(|e| e.to_string())
}

fn dispatch(cmd: &str, mut pos: Vec<String>, mut opts: Map<String, Value>, as_json: bool) -> Result<(), String> {
    let mut first = |name: &str, opts: &mut Map<String, Value>| -> Result<(), String> {
        if pos.is_empty() {
            return Err(format!("{cmd}: missing <{name}>"));
        }
        opts.insert(name.into(), json!(pos.remove(0)));
        Ok(())
    };
    let (method, fmt): (&str, fn(&Value) -> String) = match cmd {
        "version" => {
            println!("abromux {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        "ping" => ("system.ping", |v| v.as_str().unwrap_or_default().into()),
        "identify" => ("system.identify", pretty),
        "list-workspaces" => ("workspace.list", |v| lines(v, workspace_line)),
        "current-workspace" => ("workspace.current", workspace_line),
        "new-workspace" => ("workspace.create", |v| format!("OK {}", s(&v["ref"]))),
        "select-workspace" => ("workspace.select", |v| format!("OK {}", s(&v["ref"]))),
        "close-workspace" => ("workspace.close", |v| format!("OK {}", s(&v["closed"]))),
        "rename-workspace" => {
            first("title", &mut opts)?;
            ("workspace.rename", |v| format!("OK {} {}", s(&v["ref"]), s(&v["name"])))
        }
        "tree" => ("workspace.tree", tree),
        "list-panes" => ("pane.list", |v| lines(v, pane_line)),
        "focus-pane" => ("pane.focus", |v| format!("OK {}", s(&v["pane"]))),
        "new-pane" => ("pane.create", created),
        "new-split" => {
            first("direction", &mut opts)?;
            ("surface.split", created)
        }
        "list-pane-surfaces" => ("surface.list", |v| lines(v, surface_line)),
        "new-surface" => ("surface.create", |v| format!("OK {}", s(&v["ref"]))),
        "close-surface" => ("surface.close", |v| format!("OK {}", s(&v["closed"]))),
        "focus-surface" => ("surface.focus", |v| format!("OK {}", s(&v["ref"]))),
        "send" => {
            first("text", &mut opts)?;
            ("surface.send_text", |_| "OK".into())
        }
        "send-key" => {
            first("key", &mut opts)?;
            ("surface.send_key", |_| "OK".into())
        }
        "read-screen" | "capture-pane" => {
            if let Some(Value::String(n)) = opts.get("lines") {
                let n: u64 = n.parse().map_err(|_| "--lines needs a number")?;
                opts.insert("lines".into(), json!(n));
            }
            ("surface.read_text", |v| s(v))
        }
        "clear-history" => ("surface.clear_history", |_| "OK".into()),
        "sidebar" => {
            opts.insert("state".into(), json!(pos.first().map(String::as_str).unwrap_or("toggle")));
            ("app.sidebar", |v| format!("OK visible={}", v["visible"]))
        }
        "quit" => ("app.quit", |_| "OK".into()),
        "focus-window" => ("window.focus", |v| format!("OK {}", s(&v["window"]))),
        "open" => return open(pos, opts),
        "browser" => return browser(pos, opts, as_json),
        "rpc" => {
            let method = pos.first().ok_or("rpc: missing <method>")?.clone();
            let params = match pos.get(1) {
                Some(j) => serde_json::from_str(j).map_err(|e| format!("rpc: bad JSON params: {e}"))?,
                None => Value::Object(opts),
            };
            println!("{}", serde_json::to_string_pretty(&ipc::call(&method, params).map_err(|e| e.to_string())?).unwrap());
            return Ok(());
        }
        _ => return Err(format!("unknown command {cmd:?} (see `abromux --help`)")),
    };
    print_result(&call(method, opts)?, fmt, as_json);
    Ok(())
}

fn browser(mut pos: Vec<String>, mut opts: Map<String, Value>, as_json: bool) -> Result<(), String> {
    // `browser surface:3 url` is shorthand for `browser --surface surface:3 url`.
    if pos.first().is_some_and(|p| p.starts_with("surface:")) {
        opts.insert("surface".into(), json!(pos.remove(0)));
    }
    if pos.is_empty() {
        return Err("browser: missing <subcommand>".into());
    }
    let sub = pos.remove(0);
    let arg = pos.join(" ");
    let (method, fmt): (&str, fn(&Value) -> String) = match sub.as_str() {
        "open" | "open-split" => {
            if !arg.is_empty() {
                opts.insert("url".into(), json!(arg));
            }
            ("browser.open", created)
        }
        "goto" | "navigate" => {
            opts.insert("url".into(), json!(if arg.is_empty() { return Err(format!("browser {sub}: missing <url>")) } else { arg }));
            ("browser.navigate", |v| format!("OK {}", s(&v["url"])))
        }
        "back" => ("browser.back", |_| "OK".into()),
        "forward" => ("browser.forward", |_| "OK".into()),
        "reload" => ("browser.reload", |_| "OK".into()),
        "url" | "get-url" => ("browser.url", |v| s(&v["url"])),
        "eval" => {
            if arg.is_empty() {
                return Err("browser eval: missing <script>".into());
            }
            opts.insert("script".into(), json!(arg));
            ("browser.eval", |v| match v {
                Value::String(s) => s.clone(),
                v => v.to_string(),
            })
        }
        _ => return Err(format!("unknown browser subcommand {sub:?}")),
    };
    print_result(&call(method, opts)?, fmt, as_json);
    Ok(())
}

/// `open` each argument: directories become workspaces, anything else a browser split.
fn open(pos: Vec<String>, opts: Map<String, Value>) -> Result<(), String> {
    if pos.is_empty() {
        return Err("open: missing <path-or-url>".into());
    }
    for target in pos {
        let mut o = opts.clone();
        let v = if Path::new(&target).is_dir() {
            o.insert("cwd".into(), json!(absolute(&target)));
            call("workspace.create", o)?
        } else {
            o.insert("url".into(), json!(target));
            call("browser.open", o)?
        };
        println!("OK {}", s(v.get("ref").or(v.get("surface").and_then(|s| s.get("ref"))).unwrap_or(&Value::Null)));
    }
    Ok(())
}

/// `abromux <dir>`: a new workspace in the running app, or launch one there.
fn open_dir(dir: &str) -> i32 {
    let dir = absolute(dir);
    let mut o = Map::new();
    o.insert("cwd".into(), json!(dir));
    match call("workspace.create", o) {
        Ok(v) => {
            println!("OK {}", s(&v["ref"]));
            0
        }
        Err(_) => {
            let exe = match std::env::current_exe() {
                Ok(e) => e,
                Err(e) => return fail(e),
            };
            let spawned = std::process::Command::new(exe)
                .env("ABROMUX_CWD", &dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match spawned {
                Ok(_) => {
                    println!("OK launched abromux in {dir}");
                    0
                }
                Err(e) => fail(format!("can't launch abromux: {e}")),
            }
        }
    }
}

/// Bare `abromux` from an installed AbroMux.app: bring the running app forward, or
/// start the bundle through Launch Services rather than as a second app tied
/// to this terminal. None means "run the app here": dev builds outside a
/// bundle, and the bundle's own launch from Finder or `open`.
pub fn launch() -> Option<i32> {
    let exe = std::env::current_exe().and_then(|e| e.canonicalize()).ok()?;
    let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"))?;
    if ipc::call("window.focus", json!({})).is_ok() {
        return Some(0);
    }
    if !std::io::stdin().is_terminal() {
        return None;
    }
    match std::process::Command::new("/usr/bin/open").arg(bundle).status() {
        Ok(s) if s.success() => Some(0),
        Ok(s) => Some(fail(format!("open {}: {s}", bundle.display()))),
        Err(e) => Some(fail(format!("can't run open: {e}"))),
    }
}

fn absolute(p: &str) -> String {
    std::fs::canonicalize(p).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| p.into())
}

// ---- output -------------------------------------------------------------

fn print_result(v: &Value, fmt: fn(&Value) -> String, as_json: bool) {
    let out = if as_json { pretty(v) } else { fmt(v) };
    if !out.is_empty() {
        println!("{out}");
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

fn lines(v: &Value, f: fn(&Value) -> String) -> String {
    v.as_array().map(|a| a.iter().map(f).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

fn mark(on: bool) -> &'static str {
    if on { "*" } else { " " }
}

fn workspace_line(w: &Value) -> String {
    let mut line = format!("{} {}  {}", mark(w["selected"] == true), s(&w["ref"]), s(&w["name"]));
    if let Some(cwd) = w["cwd"].as_str() {
        line += &format!("  {cwd}");
    }
    if let Some(b) = w["branch"].as_str() {
        line += &format!(" ({b})");
    }
    if w["unread"] == true {
        line += "  [unread]";
    }
    line
}

fn pane_line(p: &Value) -> String {
    let n = p["surfaces"].as_array().map_or(0, Vec::len);
    format!("{} {}  {n} surface{}", mark(p["focused"] == true), s(&p["ref"]), if n == 1 { "" } else { "s" })
}

fn surface_line(t: &Value) -> String {
    let detail = t["url"].as_str().or(t["cwd"].as_str()).unwrap_or_default();
    format!("{} {}  {:<8}  {}  {detail}", mark(t["active"] == true), s(&t["ref"]), s(&t["type"]), s(&t["title"]))
}

fn tree(v: &Value) -> String {
    let mut out = vec![];
    for w in v.as_array().into_iter().flatten() {
        out.push(workspace_line(w));
        for p in w["panes"].as_array().into_iter().flatten() {
            out.push(format!("    {}", pane_line(p)));
            for t in p["surfaces"].as_array().into_iter().flatten() {
                out.push(format!("        {}", surface_line(t)));
            }
        }
    }
    out.join("\n")
}

fn created(v: &Value) -> String {
    format!("OK {} {} {}", s(&v["surface"]["ref"]), s(&v["pane"]), s(&v["workspace"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Args {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn parses_flags_switches_and_positionals() {
        let a = args(&["send", "--surface", "surface:2", "ls\\n", "--no-focus", "--scrollback"]);
        assert_eq!(a.pos, ["send", "ls\\n"]);
        assert_eq!(a.opts["surface"], "surface:2");
        assert_eq!(a.opts["focus"], false);
        assert_eq!(a.opts["scrollback"], true);

        let a = args(&["send", "--", "--not-a-flag"]);
        assert_eq!(a.pos, ["send", "--not-a-flag"]);
        assert!(parse(&["--pane".to_string()]).is_err());
    }
}
