//! `abromux <command>`: drive a running abromux over its control socket. Command
//! names and flags follow cmux's CLI where the two overlap.

use crate::{agent, ipc, settings, skills};
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
                [--layout <json>|@<file>]                  (build a cmux-style layout: splits, panes, tabs)
  select-workspace --workspace <id|ref|index>
  close-workspace --workspace <id|ref|index>
  rename-workspace [--workspace <id|ref|index>] <title>
  tree [--all] [--workspace <id|ref|index>]
  list-panes [--workspace <id|ref|index>]
  focus-pane --pane <id|ref|index> [--workspace <id|ref|index>]
  last-pane [--workspace <id|ref|index>]                     (focus the pane focused before this one)
  swap-pane [--pane <id|ref|index>] [--workspace <id|ref|index>] [-L|-R|-U|-D]   (swap tabs with the pane that way; default -R)
  resize-pane [--pane <id|ref|index>] [--workspace <id|ref|index>] [-L|-R|-U|-D] [--amount <px>]   (default -R, panes.resizeStep: 20px)
  new-pane [--type <terminal|browser>] [--direction <left|right|up|down>] [--workspace <ref>] [--url <url>] [--command <text>] [--focus <true|false>]
  new-split <left|right|up|down> [--surface <ref>] [--workspace <ref>] [--type <terminal|browser>] [--url <url>] [--command <text>] [--focus <true|false>]
  list-pane-surfaces [--workspace <ref>] [--pane <ref>]
  new-surface [--type <terminal|browser>] [--pane <ref>] [--workspace <ref>] [--url <url>] [--command <text>] [--focus <true|false>]
  close-surface [--surface <ref>] [--workspace <ref>]
  focus-surface --surface <ref>
  rename-tab [--surface <ref>] [--workspace <ref>] [<title>]  (no title: back to the program's own)
  respawn-pane [--surface <ref>] [--workspace <ref>] [--command <text>]   (kill the shell and start a fresh one in its cwd)
  send [--surface <ref>] [--workspace <ref>] <text>          (\\n presses enter; also \\t \\e \\\\)
  send-key [--surface <ref>] [--workspace <ref>] <key>       (enter, tab, escape, up, ctrl-c, alt-b, ...)
  read-screen [--surface <ref>] [--workspace <ref>] [--scrollback] [--lines <n>]
  capture-pane                                               (alias for read-screen)
  clear-history [--surface <ref>] [--workspace <ref>]        (clear screen and scrollback, like ⌘K)
  notify [--title <text>] [--subtitle <text>] [--body <text>] [--surface <ref>]   (rings that terminal's pane)
  list-notifications
  open-notification <id>                                     (select its terminal and mark it read)
  jump-to-unread                                             (the newest unread notification, like ⌘⇧U)
  clear-notifications
  set-status <key> <value> [--icon <name>] [--color <#rrggbb>] [--workspace <ref>]   (a pill under the workspace in the sidebar)
  clear-status <key> [--workspace <ref>]
  list-status [--workspace <ref>]
  set-progress <0.0-1.0> [--label <text>] [--workspace <ref>]
  clear-progress [--workspace <ref>]
  log [--level <info|progress|success|warning|error>] [--source <name>] [--workspace <ref>] <message>
  list-log [--limit <n>] [--workspace <ref>]
  clear-log [--workspace <ref>]
  open <path-or-url>...                                      (directories open workspaces, URLs open browser splits)
  focus-window [--window <ref>]                              (bring abromux to the front; there is one window)
  sidebar <toggle|show|hide>
  reload-config                                              (read the config file again, like ⌘⇧,)
  config path                                                (where abromux's config.json lives)
  config check [<file>]                                      (validate the config file; needs no running app)
  config shortcuts                                           (each shortcut's name for config.json, and its keys now)
  config project                                             (the project commands file found from here, and its commands)
  hooks install|uninstall [agent...]                         (agent hooks: status, notifications, resume on restore;
                                                             claude, codex, gemini, droid, cursor, opencode, pi)
  skills install|uninstall                                   (Claude Code and Codex skills that teach them this CLI)
  skills show [abromux|abromux-browser]                      (print a skill: an agent-facing guide)
  quit
  rpc <method> [json-params]

  browser [--surface <ref>] <subcommand> ...
  browser open [url] [--direction <dir>] [--focus <true|false>]   (browser split next to the caller)
  browser open-split [url]
  browser goto|navigate <url>
  browser back|forward|reload
  browser url|get-url
  browser eval <script>

  Automation (<sel> is a CSS selector or a ref like e3 from the last snapshot):
  browser snapshot [--interactive] [--compact] [--selector <sel>]   (accessibility tree with refs)
  browser click|dblclick|hover|focus|check|uncheck|scroll-into-view <sel>
  browser fill <sel> <text>                                  (replace the field's value)
  browser type <sel> <text>                                  (key by key, into what's there)
  browser press <key> [--selector <sel>]                     (Enter, Tab, Escape, ArrowDown, ctrl+a, ...)
  browser select <sel> <value>...
  browser scroll [up|down|left|right] [px] [--selector <sel>]   (default down 500)
  browser get url|title|text|html|value|attr|count|box|styles [<sel>] [<attr or style names>...]
  browser is visible|hidden|enabled|disabled|checked|editable|focused <sel>
  browser find role|text|label|placeholder|alt|title|testid <value> [--name <name>] [--exact]
  browser wait [<sel>] [--state visible|hidden|attached|detached] [--text <t>] [--url <part>]
               [--load interactive|complete] [--js <expr>] [--timeout <ms>]   (default 10000)
  browser screenshot [<path.png>]                            (default: a file in $TMPDIR)
";

/// Flags that take no value.
const SWITCHES: &[&str] = &[
    "json",
    "scrollback",
    "all",
    "help",
    "interactive",
    "compact",
    "exact",
];

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
    // Runs on every agent hook event, so skip the usual parsing.
    if argv.len() == 2
        && argv[0] == "hooks"
        && let Some(kind) = argv[1].strip_suffix("-event").and_then(agent::Kind::parse)
    {
        return agent::hook_event(kind);
    }
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

fn dispatch(
    cmd: &str,
    mut pos: Vec<String>,
    mut opts: Map<String, Value>,
    as_json: bool,
) -> Result<(), String> {
    let mut first = |name: &str, opts: &mut Map<String, Value>| -> Result<(), String> {
        if pos.is_empty() {
            return Err(format!("{cmd}: missing <{name}>"));
        }
        opts.insert(name.into(), json!(pos.remove(0)));
        Ok(())
    };
    let (method, fmt): (&str, fn(&Value) -> String) = match cmd {
        "version" => {
            println!("abromux {}", env!("ABROMUX_VERSION"));
            return Ok(());
        }
        "ping" => ("system.ping", |v| v.as_str().unwrap_or_default().into()),
        "identify" => ("system.identify", pretty),
        "list-workspaces" => ("workspace.list", |v| lines(v, workspace_line)),
        "current-workspace" => ("workspace.current", workspace_line),
        "new-workspace" => {
            // `--layout @file.json` reads it from a file.
            if let Some(Value::String(l)) = opts.get_mut("layout")
                && let Some(file) = l.strip_prefix('@')
            {
                *l = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            }
            // The app runs elsewhere: make the directory absolute here.
            if let Some(Value::String(c)) = opts.get_mut("cwd")
                && let Ok(abs) = std::path::absolute(&*c)
            {
                *c = abs.to_string_lossy().into_owned();
            }
            ("workspace.create", |v| format!("OK {}", s(&v["ref"])))
        }
        "select-workspace" => ("workspace.select", |v| format!("OK {}", s(&v["ref"]))),
        "close-workspace" => ("workspace.close", |v| format!("OK {}", s(&v["closed"]))),
        "rename-workspace" => {
            first("title", &mut opts)?;
            ("workspace.rename", |v| {
                format!("OK {} {}", s(&v["ref"]), s(&v["name"]))
            })
        }
        "tree" => ("workspace.tree", tree),
        "list-panes" => ("pane.list", |v| lines(v, pane_line)),
        "focus-pane" => ("pane.focus", |v| format!("OK {}", s(&v["pane"]))),
        "last-pane" => ("pane.last", |v| format!("OK {}", s(&v["pane"]))),
        "swap-pane" => {
            let dir = match pos.first().map(String::as_str) {
                None | Some("-R") => "right",
                Some("-L") => "left",
                Some("-U") => "up",
                Some("-D") => "down",
                Some(d) => {
                    return Err(format!("swap-pane: expected -L, -R, -U or -D, got {d:?}"));
                }
            };
            opts.insert("direction".into(), json!(dir));
            ("pane.swap", |v| format!("OK {}", s(&v["pane"])))
        }
        "resize-pane" => {
            let dir = match pos.first().map(String::as_str) {
                None | Some("-R") => "right",
                Some("-L") => "left",
                Some("-U") => "up",
                Some("-D") => "down",
                Some(d) => {
                    return Err(format!("resize-pane: expected -L, -R, -U or -D, got {d:?}"));
                }
            };
            opts.insert("direction".into(), json!(dir));
            ("pane.resize", |v| format!("OK {}", s(&v["pane"])))
        }
        "new-pane" => ("pane.create", created),
        "new-split" => {
            first("direction", &mut opts)?;
            ("surface.split", created)
        }
        "list-pane-surfaces" => ("surface.list", |v| lines(v, surface_line)),
        "new-surface" => ("surface.create", |v| format!("OK {}", s(&v["ref"]))),
        "close-surface" => ("surface.close", |v| format!("OK {}", s(&v["closed"]))),
        "focus-surface" => ("surface.focus", |v| format!("OK {}", s(&v["ref"]))),
        "rename-tab" => {
            if !pos.is_empty() {
                opts.insert("title".into(), json!(pos.join(" ")));
            }
            ("surface.rename", |v| {
                format!("OK {} {}", s(&v["ref"]), s(&v["title"]))
            })
        }
        "respawn-pane" => ("surface.respawn", |v| format!("OK {}", s(&v["ref"]))),
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
        "notify" => {
            if !opts.contains_key("body") && !pos.is_empty() {
                opts.insert("body".into(), json!(pos.join(" ")));
            }
            ("notification.create", |v| match v["id"].as_u64() {
                Some(id) => format!("OK notification:{id} {}", s(&v["surface"])),
                None => "OK (you were looking at it)".into(),
            })
        }
        "list-notifications" => ("notification.list", |v| lines(v, notification_line)),
        "open-notification" => {
            first("id", &mut opts)?;
            ("notification.open", |v| {
                format!("OK notification:{}", v["id"])
            })
        }
        "jump-to-unread" => ("notification.jump_unread", |v| {
            if v["jumped"] == true {
                "OK".into()
            } else {
                "OK no unread notifications".into()
            }
        }),
        "clear-notifications" => ("notification.clear", |_| "OK".into()),
        "set-status" => {
            first("key", &mut opts)?;
            if pos.is_empty() {
                return Err("set-status: missing <value>".into());
            }
            opts.insert("value".into(), json!(pos.join(" ")));
            ("status.set", |v| format!("OK {}", s(&v["workspace"])))
        }
        "clear-status" => {
            first("key", &mut opts)?;
            ("status.clear", |v| {
                if v["cleared"] == true {
                    "OK".into()
                } else {
                    "OK (no such status)".into()
                }
            })
        }
        "list-status" => ("status.list", |v| {
            lines(v, |st| {
                let color = st["color"]
                    .as_str()
                    .map(|c| format!("  {c}"))
                    .unwrap_or_default();
                format!("{}  {}{color}", s(&st["key"]), s(&st["value"]))
            })
        }),
        "set-progress" => {
            first("value", &mut opts)?;
            ("progress.set", |v| format!("OK {}", s(&v["workspace"])))
        }
        "clear-progress" => ("progress.clear", |_| "OK".into()),
        "log" => {
            if pos.is_empty() {
                return Err("log: missing <message>".into());
            }
            opts.insert("message".into(), json!(pos.join(" ")));
            ("log.add", |_| "OK".into())
        }
        "list-log" => ("log.list", |v| {
            lines(v, |l| {
                let source = l["source"]
                    .as_str()
                    .map(|s| format!("[{s}] "))
                    .unwrap_or_default();
                format!(
                    "{:<8} {}s ago  {source}{}",
                    s(&l["level"]),
                    l["age_seconds"],
                    s(&l["message"])
                )
            })
        }),
        "clear-log" => ("log.clear", |_| "OK".into()),
        "sidebar" => {
            opts.insert(
                "state".into(),
                json!(pos.first().map(String::as_str).unwrap_or("toggle")),
            );
            ("app.sidebar", |v| format!("OK visible={}", v["visible"]))
        }
        "quit" => ("app.quit", |_| "OK".into()),
        "reload-config" => ("app.reload_config", |v| format!("OK {}", s(&v["path"]))),
        "config" => return config(&pos),
        "focus-window" => ("window.focus", |v| format!("OK {}", s(&v["window"]))),
        "hooks" => {
            let msg = match pos.first().map(String::as_str) {
                Some("install") => agent::hooks(true, &pos[1..]),
                Some("uninstall") => agent::hooks(false, &pos[1..]),
                _ => return Err("hooks: expected install or uninstall".into()),
            };
            println!("{}", msg.map_err(|e| format!("{e:#}"))?);
            return Ok(());
        }
        "skills" => {
            println!("{}", skills::run(&pos).map_err(|e| format!("{e:#}"))?);
            return Ok(());
        }
        "open" => return open(pos, opts),
        "browser" => return browser(pos, opts, as_json),
        "rpc" => {
            let method = pos.first().ok_or("rpc: missing <method>")?.clone();
            let params = match pos.get(1) {
                Some(j) => {
                    serde_json::from_str(j).map_err(|e| format!("rpc: bad JSON params: {e}"))?
                }
                None => Value::Object(opts),
            };
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &ipc::call(&method, params).map_err(|e| e.to_string())?
                )
                .unwrap()
            );
            return Ok(());
        }
        _ => return Err(format!("unknown command {cmd:?} (see `abromux --help`)")),
    };
    print_result(&call(method, opts)?, fmt, as_json);
    Ok(())
}

/// `config path`, `config check [<file>]`, `config project` and `config shortcuts`, which
/// need no running app.
fn config(pos: &[String]) -> Result<(), String> {
    match pos.first().map(String::as_str) {
        Some("path") => println!("{}", settings::path().display()),
        Some("check") => {
            let path = pos.get(1).map_or_else(settings::path, Into::into);
            settings::read(&path)?;
            let note = if path.exists() {
                ""
            } else {
                " (no file; using defaults)"
            };
            println!("OK {}{note}", path.display());
        }
        Some("project") => {
            let here = std::env::current_dir().map_err(|e| e.to_string())?;
            let Some(file) = settings::find_project(&here) else {
                println!(
                    "No project file here or above (looked for {})",
                    settings::PROJECT_FILES.join(", ")
                );
                return Ok(());
            };
            let cmds = settings::read_project(&file)?;
            println!("OK {} ({} commands)", file.display(), cmds.len());
            for (c, raw) in cmds {
                let trusted = if settings::is_trusted(&file, &raw) {
                    ""
                } else {
                    "  (asks before running)"
                };
                println!("  {}{trusted}", c.title);
            }
        }
        Some("shortcuts") => {
            let settings = settings::load()?;
            print!("{}", crate::keymap::describe(&settings));
        }
        _ => return Err("config: expected path, check, project or shortcuts".into()),
    }
    Ok(())
}

fn browser(
    mut pos: Vec<String>,
    mut opts: Map<String, Value>,
    as_json: bool,
) -> Result<(), String> {
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
            opts.insert(
                "url".into(),
                json!(if arg.is_empty() {
                    return Err(format!("browser {sub}: missing <url>"));
                } else {
                    arg
                }),
            );
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
            ("browser.eval", raw)
        }
        "snapshot" => {
            if let Some(sel) = pos.first() {
                opts.insert("selector".into(), json!(sel));
            }
            ("browser.snapshot", |v| s(&v["snapshot"]))
        }
        "click" | "dblclick" | "hover" | "focus" | "check" | "uncheck" | "scroll-into-view" => {
            selector(&mut pos, &mut opts, &sub)?;
            let method = match sub.as_str() {
                "click" => "browser.click",
                "dblclick" => "browser.dblclick",
                "hover" => "browser.hover",
                "focus" => "browser.focus",
                "check" => "browser.check",
                "uncheck" => {
                    opts.insert("checked".into(), json!(false));
                    "browser.check"
                }
                _ => "browser.scroll_into_view",
            };
            (method, |_| "OK".into())
        }
        "fill" | "type" => {
            selector(&mut pos, &mut opts, &sub)?;
            if pos.is_empty() && sub == "type" {
                return Err("browser type: missing <text>".into());
            }
            opts.insert("text".into(), json!(pos.join(" ")));
            (
                if sub == "fill" {
                    "browser.fill"
                } else {
                    "browser.type"
                },
                |_| "OK".into(),
            )
        }
        "press" => {
            if arg.is_empty() {
                return Err("browser press: missing <key>".into());
            }
            opts.insert("key".into(), json!(arg));
            ("browser.press", |_| "OK".into())
        }
        "select" => {
            selector(&mut pos, &mut opts, &sub)?;
            if pos.is_empty() {
                return Err("browser select: missing <value>".into());
            }
            opts.insert("values".into(), json!(pos));
            ("browser.select", |v| format!("OK {}", lines(v, s)))
        }
        "scroll" => {
            let dir = pos
                .first()
                .filter(|d| ["up", "down", "left", "right"].contains(&d.as_str()))
                .cloned();
            if dir.is_some() {
                pos.remove(0);
            }
            let amount: f64 = match pos.first() {
                Some(n) => n
                    .parse()
                    .map_err(|_| format!("browser scroll: bad amount {n:?}"))?,
                None => 500.,
            };
            let (dx, dy) = match dir.as_deref().unwrap_or("down") {
                "up" => (0., -amount),
                "down" => (0., amount),
                "left" => (-amount, 0.),
                _ => (amount, 0.),
            };
            opts.insert("dx".into(), json!(dx));
            opts.insert("dy".into(), json!(dy));
            ("browser.scroll", |_| "OK".into())
        }
        "get" => {
            if pos.is_empty() {
                return Err("browser get: missing what to get (url, title, text, html, value, attr, count, box or styles)".into());
            }
            let what = pos.remove(0);
            if !pos.is_empty() {
                opts.insert("selector".into(), json!(pos.remove(0)));
            }
            match what.as_str() {
                "attr" => {
                    opts.insert(
                        "name".into(),
                        json!(pos.first().ok_or("browser get attr: missing <attribute>")?),
                    );
                }
                "styles" => {
                    opts.insert("names".into(), json!(pos));
                }
                _ => {}
            }
            opts.insert("what".into(), json!(what));
            ("browser.get", |v| match v {
                Value::Object(_) => pretty(v),
                v => raw(v),
            })
        }
        "is" => {
            if pos.is_empty() {
                return Err("browser is: missing <state>".into());
            }
            opts.insert("state".into(), json!(pos.remove(0)));
            selector(&mut pos, &mut opts, &sub)?;
            ("browser.is", raw)
        }
        "find" => {
            if pos.len() < 2 {
                return Err("browser find: expected <by> <value>".into());
            }
            opts.insert("by".into(), json!(pos.remove(0)));
            opts.insert("value".into(), json!(pos.join(" ")));
            ("browser.find", |v| {
                lines(v, |e| match e["name"].as_str().filter(|n| !n.is_empty()) {
                    Some(n) => format!("- {} {} [ref={}]", s(&e["role"]), json!(n), s(&e["ref"])),
                    None => format!("- {} [ref={}]", s(&e["role"]), s(&e["ref"])),
                })
            })
        }
        "wait" => {
            if let Some(sel) = pos.first() {
                opts.insert("selector".into(), json!(sel));
            }
            ("browser.wait", |v| format!("OK {}ms", v["waited_ms"]))
        }
        "screenshot" => {
            if let Some(path) = pos.first() {
                let path = Path::new(path);
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    std::env::current_dir()
                        .map_err(|e| e.to_string())?
                        .join(path)
                };
                opts.insert("path".into(), json!(path));
            }
            ("browser.screenshot", |v| s(&v["path"]))
        }
        _ => return Err(format!("unknown browser subcommand {sub:?}")),
    };
    print_result(&call(method, opts)?, fmt, as_json);
    Ok(())
}

/// The first positional as `--selector`, unless that was given.
fn selector(pos: &mut Vec<String>, opts: &mut Map<String, Value>, sub: &str) -> Result<(), String> {
    if !opts.contains_key("selector") {
        if pos.is_empty() {
            return Err(format!("browser {sub}: missing <selector or ref>"));
        }
        opts.insert("selector".into(), json!(pos.remove(0)));
    }
    Ok(())
}

/// Strings as they are, anything else as JSON.
fn raw(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
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
        println!(
            "OK {}",
            s(v.get("ref")
                .or(v.get("surface").and_then(|s| s.get("ref")))
                .unwrap_or(&Value::Null))
        );
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
    let exe = std::env::current_exe()
        .and_then(|e| e.canonicalize())
        .ok()?;
    let bundle = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))?;
    if ipc::call("window.focus", json!({})).is_ok() {
        return Some(0);
    }
    if !std::io::stdin().is_terminal() {
        return None;
    }
    match std::process::Command::new("/usr/bin/open")
        .arg(bundle)
        .status()
    {
        Ok(s) if s.success() => Some(0),
        Ok(s) => Some(fail(format!("open {}: {s}", bundle.display()))),
        Err(e) => Some(fail(format!("can't run open: {e}"))),
    }
}

fn absolute(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.into())
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
    v.as_array()
        .map(|a| a.iter().map(f).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

fn mark(on: bool) -> &'static str {
    if on { "*" } else { " " }
}

fn workspace_line(w: &Value) -> String {
    let mut line = format!(
        "{} {}  {}",
        mark(w["selected"] == true),
        s(&w["ref"]),
        s(&w["name"])
    );
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

fn notification_line(n: &Value) -> String {
    let text = match (s(&n["title"]), s(&n["body"])) {
        (t, b) if t.is_empty() => b,
        (t, b) if b.is_empty() => t,
        (t, b) => format!("{t}: {b}"),
    };
    let unread = if n["read"] == true { " " } else { "•" };
    format!(
        "{unread} notification:{}  {} {}  {}s ago  {text}",
        n["id"],
        s(&n["workspace"]),
        s(&n["surface"]),
        n["age_seconds"]
    )
}

fn pane_line(p: &Value) -> String {
    let n = p["surfaces"].as_array().map_or(0, Vec::len);
    format!(
        "{} {}  {n} surface{}",
        mark(p["focused"] == true),
        s(&p["ref"]),
        if n == 1 { "" } else { "s" }
    )
}

fn surface_line(t: &Value) -> String {
    let detail = t["url"].as_str().or(t["cwd"].as_str()).unwrap_or_default();
    format!(
        "{} {}  {:<8}  {}  {detail}",
        mark(t["active"] == true),
        s(&t["ref"]),
        s(&t["type"]),
        s(&t["title"])
    )
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
    format!(
        "OK {} {} {}",
        s(&v["surface"]["ref"]),
        s(&v["pane"]),
        s(&v["workspace"])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Args {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn parses_flags_switches_and_positionals() {
        let a = args(&[
            "send",
            "--surface",
            "surface:2",
            "ls\\n",
            "--no-focus",
            "--scrollback",
        ]);
        assert_eq!(a.pos, ["send", "ls\\n"]);
        assert_eq!(a.opts["surface"], "surface:2");
        assert_eq!(a.opts["focus"], false);
        assert_eq!(a.opts["scrollback"], true);

        let a = args(&["send", "--", "--not-a-flag"]);
        assert_eq!(a.pos, ["send", "--not-a-flag"]);
        assert!(parse(&["--pane".to_string()]).is_err());
    }
}
