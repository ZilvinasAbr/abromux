//! The app's part of the `abromux` CLI (abromux_cli): its socket, and the
//! commands that need the app on this machine: files and folders here
//! (`open` a Markdown file, `edit`, `markdown`, `claude`, `abromux <dir>`),
//! its settings (`config`), Lua (`do`, init.lua actions), the phone
//! remote, and local diffs.

use crate::settings;
use abromux_cli::{EXIT_STATUS, call, fail, ipc, s};
use serde_json::{Map, Value, json};
use std::{io::IsTerminal, path::Path};

pub struct App;

impl abromux_cli::Host for App {
    fn socket_path(&self) -> std::path::PathBuf {
        crate::ipc::socket_path()
    }

    fn command(
        &self,
        cmd: &str,
        pos: Vec<String>,
        opts: &Map<String, Value>,
        as_json: bool,
    ) -> Option<Result<i32, String>> {
        let opts = opts.clone();
        let done = |r: Result<(), String>| {
            r.map(|()| EXIT_STATUS.load(std::sync::atomic::Ordering::Relaxed))
        };
        Some(match cmd {
            "diff" => done(diff_cmd(pos, opts, as_json)),
            "config" => done(config(&pos)),
            "edit" => done(edit(pos, opts)),
            "default-app" => done(default_app(pos)),
            "markdown" | "md" => done(markdown(pos, opts)),
            "claude" | "claude-tab" => done(claude(pos, opts)),
            "do" => crate::lua::run_do(pos, &opts, as_json),
            "remote" => done(crate::remote::cli(&pos, &opts, as_json)),
            // An action of init.lua: `abromux folder add ~/repos/x --branch y`
            // runs action `folder` with args { "add", "~/repos/x", branch = "y" }.
            _ => {
                let mut opts = opts;
                opts.remove("caller");
                return crate::lua::run_cli_action(cmd, pos, &opts, as_json);
            }
        })
    }

    fn key_label(&self, name: &str) -> String {
        crate::keymap::default_label(name)
    }

    fn named_source(&self, from: &str) -> bool {
        crate::session::named_source(from).is_some()
    }

    fn open_dir(&self, dir: &str) -> Option<i32> {
        Some(open_dir(dir))
    }

    fn view_url(&self, path: &Path) -> Option<String> {
        (crate::markdown::is_markdown(path) || crate::markdown::is_image(path))
            .then(|| crate::markdown::view_url_for(path))
    }
}

pub fn run(argv: Vec<String>) -> i32 {
    abromux_cli::run(argv, Box::new(App))
}

fn absolute(p: &str) -> String {
    abromux_cli::absolute(p)
}

/// `abromux diff …`: any git diff reviewed in the GitLab tab
/// (`abromux_gitlab::local`). `list`, `threads` and `reply` work on files
/// only, so an agent can answer without the app.
fn diff_cmd(mut pos: Vec<String>, opts: Map<String, Value>, as_json: bool) -> Result<(), String> {
    use abromux_gitlab::local;
    local::init(crate::channel::support_dir().join("local-diffs"));
    let text = |k: &str, opts: &Map<String, Value>| {
        opts.get(k).and_then(Value::as_str).map(str::to_string)
    };
    let e = |e: anyhow::Error| format!("{e:#}");
    match pos.first().map(String::as_str) {
        Some("list" | "ls") => {
            let all = local::all();
            if as_json {
                println!("{}", serde_json::to_string_pretty(&all).unwrap_or_default());
                return Ok(());
            }
            if all.is_empty() {
                println!("No local diff reviews (open one with `abromux diff`).");
            }
            for s in all {
                let open = s.threads.iter().filter(|t| !t.resolved()).count();
                println!(
                    "{}  {}  {}  ({} threads, {open} open)",
                    s.id,
                    s.repo,
                    s.describe(),
                    s.threads.len()
                );
            }
            return Ok(());
        }
        Some("threads") => {
            let id = local::find(pos.get(1).ok_or("diff threads: missing <id>")?).map_err(e)?;
            let s = local::load(&id).map_err(e)?;
            let unresolved = opts.contains_key("unresolved");
            let threads: Vec<&local::Thread> = s
                .threads
                .iter()
                .filter(|t| !unresolved || !t.resolved())
                .collect();
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&threads).unwrap_or_default()
                );
                return Ok(());
            }
            for t in threads {
                let at = t.position().map_or("(the whole diff)".to_string(), |p| {
                    let line = p.new_line.map_or_else(
                        || format!("old {}", p.old_line.unwrap_or(0)),
                        |n| n.to_string(),
                    );
                    format!("{}:{line}", p.path())
                });
                println!(
                    "{}  {}  {at}",
                    t.id,
                    if t.resolved() { "resolved" } else { "open" }
                );
                for n in &t.notes {
                    let mut lines = n.body.lines();
                    println!("  {}: {}", n.author.name, lines.next().unwrap_or_default());
                    for l in lines {
                        println!("    {l}");
                    }
                }
            }
            return Ok(());
        }
        Some("reply") => {
            let id = local::find(pos.get(1).ok_or("diff reply: missing <id>")?).map_err(e)?;
            let thread = pos.get(2).ok_or("diff reply: missing <thread>")?.clone();
            let mut body = pos[3..].join(" ");
            if body.is_empty() && !std::io::stdin().is_terminal() {
                use std::io::Read as _;
                std::io::stdin()
                    .read_to_string(&mut body)
                    .map_err(|e| e.to_string())?;
            }
            let resolve = opts.contains_key("resolve");
            if body.trim().is_empty() && !resolve {
                return Err("diff reply: give the answer (as text, or on stdin)".into());
            }
            let t = local::reply(&id, &thread, &body, resolve, text("as", &opts).as_deref())
                .map_err(e)?;
            println!("OK {id} {t}{}", if resolve { " resolved" } else { "" });
            return Ok(());
        }
        _ => {}
    }
    // Open one: the range, from here (or --cwd).
    let cwd = match text("cwd", &opts) {
        Some(c) => std::path::PathBuf::from(c),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    // `--merge-base X`, as git diff's; `--since-fork`: from the default branch.
    let spec = match (text("merge_base", &opts), opts.contains_key("since_fork")) {
        (Some(base), _) => format!("--merge-base {base}"),
        (None, true) => local::work_spec(&cwd),
        (None, false) if pos.is_empty() => String::new(),
        (None, false) => pos.remove(0),
    };
    let caller = opts.get("caller").cloned();
    let send_to = match (opts.contains_key("send_here"), text("send_to", &opts)) {
        (_, Some(t)) => Some(Some(t)),
        (true, None) => Some(Some(
            caller
                .as_ref()
                .and_then(|c| c["surface"].as_str())
                .map(str::to_string)
                .ok_or("--send-here: not in an abromux terminal")?,
        )),
        // Asked again without either: comments start agents again.
        (false, None) => Some(None),
    };
    let open = local::Open {
        title: text("title", &opts),
        agent: text("agent", &opts),
        send_to,
    };
    let id = local::create(&cwd, &spec, open).map_err(e)?;
    let s = local::load(&id).map_err(e)?;
    let host = local::host_of(&id);
    let focus = opts.get("focus").cloned().unwrap_or(json!(true));
    // Already open: that tab, fetched again.
    let tree = ipc::call("workspace.tree", json!({ "all": true })).map_err(|e| e.to_string())?;
    let existing = tree
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|w| w["panes"].as_array().into_iter().flatten())
        .find_map(|p| {
            p["surfaces"]
                .as_array()?
                .iter()
                .find(|t| t["state"]["local"] == json!(id))
                .and_then(|t| t["ref"].as_str().map(str::to_string))
        });
    let surface = match existing {
        Some(r) => {
            if focus != json!(false) {
                ipc::call("surface.focus", json!({ "surface": r })).map_err(|e| e.to_string())?;
            }
            ipc::call(
                "surface.plugin",
                json!({ "surface": r, "command": "refresh" }),
            )
            .map_err(|e| e.to_string())?;
            r
        }
        None => {
            let mut p = json!({
                "type": "gitlab", "host": host, "project": local::PROJECT, "iid": local::IID,
                "section": "changes", "focus": focus,
            });
            if let Some(c) = caller {
                p["caller"] = c;
            }
            let v = match text("pane", &opts) {
                Some(pane) => {
                    p["pane"] = json!(pane);
                    ipc::call("surface.create", p).map_err(|e| e.to_string())?
                }
                None => {
                    p["direction"] =
                        json!(text("direction", &opts).unwrap_or_else(|| "right".into()));
                    ipc::call("surface.split", p).map_err(|e| e.to_string())?["surface"].clone()
                }
            };
            v["ref"].as_str().unwrap_or_default().to_string()
        }
    };
    if as_json {
        println!(
            "{}",
            json!({ "id": id, "surface": surface, "repo": s.repo, "diff": s.describe(), "agent": s.agent, "send_to": s.send_to })
        );
    } else {
        let to = s
            .send_to
            .as_ref()
            .map_or_else(|| format!("start `{}`", s.agent), |t| format!("go to {t}"));
        println!(
            "OK {surface} {id}  {} in {}  (comments {to})",
            s.describe(),
            s.repo
        );
    }
    Ok(())
}

/// `config path`, `config open`, `config check [<file>]`, `config project` and `config shortcuts`, which
/// need no running app.
fn config(pos: &[String]) -> Result<(), String> {
    match pos.first().map(String::as_str) {
        Some("path") => println!("{}", settings::path().display()),
        Some("open") => {
            let path = settings::ensure_file();
            let status =
                abromux_platform::opener::open_and_wait(&path).map_err(|e| format!("open: {e}"))?;
            if !status.success() {
                return Err(format!("open {} failed ({status})", path.display()));
            }
            println!("OK {}", path.display());
        }
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
        _ => return Err("config: expected path, open, check, project or shortcuts".into()),
    }
    Ok(())
}

/// `abromux edit <file>…`: each in the editor, in a pane split off beside
/// the caller. A file that doesn't exist yet is fine if its folder does.
fn edit(pos: Vec<String>, opts: Map<String, Value>) -> Result<(), String> {
    if pos.is_empty() {
        return Err("edit: missing <file>".into());
    }
    for file in pos {
        let path = std::path::absolute(&file).map_err(|e| format!("edit: {file}: {e}"))?;
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if path.is_dir() || !path.parent().is_some_and(Path::is_dir) {
            return Err(format!("edit: not a file: {file}"));
        }
        let mut o = opts.clone();
        o.insert("path".into(), json!(path.to_string_lossy()));
        let v = call("file.edit", o)?;
        println!("OK {}", s(&v["surface"]["ref"]));
    }
    Ok(())
}

/// `abromux default-app <ext>… | code | list`: this app opens those files
/// on a double-click in Finder.
fn default_app(pos: Vec<String>) -> Result<(), String> {
    // What `code` covers; scripts/bundle-app.sh declares them in Info.plist.
    if pos.first().is_some_and(|p| p == "list") {
        println!("{}", crate::editor::CODE_EXTENSIONS.join("\n"));
        return Ok(());
    }
    let id = crate::channel::bundle_id().ok_or(
        "default-app: run the abromux inside AbroMux.app (a dev build has no app to make the default)",
    )?;
    let exts: Vec<String> = if pos.iter().any(|p| p == "code") {
        crate::editor::CODE_EXTENSIONS
            .iter()
            .map(|e| e.to_string())
            .collect()
    } else {
        pos.iter()
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect()
    };
    if exts.is_empty() {
        return Err("default-app: missing <ext>... or code".into());
    }
    crate::editor::set_default_app(&id, &exts).map_err(|e| format!("default-app: {e}"))?;
    println!(
        "OK {} opens .{}",
        crate::channel::app_name(),
        exts.join(" .")
    );
    Ok(())
}

/// `abromux markdown [open] <file>…`: each in a Markdown tab, split off
/// next to the caller as `browser open` does.
fn markdown(mut pos: Vec<String>, opts: Map<String, Value>) -> Result<(), String> {
    if pos.first().is_some_and(|p| p == "open") {
        pos.remove(0);
    }
    if pos.is_empty() {
        return Err("markdown: missing <file>".into());
    }
    for file in pos {
        let path = absolute(&file);
        if !Path::new(&path).is_file() {
            return Err(format!("markdown: no such file: {file}"));
        }
        let mut o = opts.clone();
        o.insert(
            "url".into(),
            json!(crate::markdown::view_url_for(Path::new(&path))),
        );
        let v = call("browser.open", o)?;
        println!(
            "OK {}",
            s(v.get("surface")
                .and_then(|s| s.get("ref"))
                .unwrap_or(&v["ref"]))
        );
    }
    Ok(())
}

/// `abromux claude [<dir> [<prompt>...]] [options]`: Claude Code as a chat
/// tab in `dir` (default: the current directory): a new tab in the caller's
/// pane, or with `--split` a pane beside it. The model, effort and mode are
/// checked here so a typo fails before a tab opens.
fn claude(mut pos: Vec<String>, mut opts: Map<String, Value>) -> Result<(), String> {
    use crate::claude_chat::{effort_name, is_model_name, mode_name};
    let dir = if pos.is_empty() {
        ".".to_string()
    } else {
        pos.remove(0)
    };
    let dir = absolute(&dir);
    if !Path::new(&dir).is_dir() {
        return Err(format!("claude: no such folder: {dir}"));
    }
    let take = |opts: &mut Map<String, Value>, k: &str| {
        opts.remove(k).and_then(|v| v.as_str().map(String::from))
    };
    let mut p = Map::new();
    p.insert("type".into(), json!("claude"));
    p.insert("cwd".into(), json!(dir));
    if let Some(m) = take(&mut opts, "model") {
        if !is_model_name(&m) {
            return Err(format!("claude: not a model name: {m:?}"));
        }
        p.insert("model".into(), json!(m));
    }
    if let Some(e) = take(&mut opts, "effort") {
        let e = effort_name(&e).ok_or_else(|| {
            format!("claude: unknown effort {e:?} (low, medium, high, xhigh, max)")
        })?;
        p.insert("effort".into(), json!(e));
    }
    if let Some(m) = take(&mut opts, "mode").or_else(|| take(&mut opts, "permission_mode")) {
        let m = mode_name(&m).ok_or_else(|| {
            format!("claude: unknown mode {m:?} (ask, accept-edits, plan, auto, dont-ask, bypass)")
        })?;
        p.insert("mode".into(), json!(m));
    }
    if let Some(id) = take(&mut opts, "resume").or_else(|| take(&mut opts, "session")) {
        p.insert("session".into(), json!(id));
    }
    if opts.remove("continue").is_some() {
        p.insert("continue".into(), json!(true));
    }
    let prompt = take(&mut opts, "prompt").or_else(|| (!pos.is_empty()).then(|| pos.join(" ")));
    if let Some(text) = prompt {
        p.insert("prompt".into(), json!(text));
    }
    let split = take(&mut opts, "split");
    // What's left: where it goes and whether it's focused.
    for (k, v) in opts {
        p.insert(k, v);
    }
    let v = match split {
        Some(dir) => {
            p.insert("direction".into(), json!(dir));
            call("surface.split", p)?
        }
        None => call("surface.create", p)?,
    };
    let r = v
        .get("surface")
        .and_then(|s| s.get("ref"))
        .unwrap_or(&v["ref"]);
    println!("OK {}", s(r));
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
/// bundle, and the bundle's own launch from Finder or `open`. On Linux an
/// installed copy (anything not run from a `target/` folder) brings the
/// running app of its channel forward, as a launcher click should, and
/// otherwise runs here.
pub fn launch() -> Option<i32> {
    let exe = std::env::current_exe()
        .and_then(|e| e.canonicalize())
        .ok()?;
    if !abromux_platform::MACOS {
        let dev = exe
            .ancestors()
            .any(|p| p.file_name().is_some_and(|n| n == "target"));
        if !dev && ipc::call("window.focus", json!({})).is_ok() {
            return Some(0);
        }
        return None;
    }
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

#[cfg(test)]
mod tests {
    /// Every `{key:action}` in the CLI's help names an action with a
    /// default shortcut in the app's keymap.
    #[test]
    fn help_keys_name_actions() {
        let mut rest = abromux_cli::HELP;
        while let Some(start) = rest.find("{key:") {
            let after = &rest[start + 5..];
            let name = &after[..after.find('}').unwrap()];
            assert!(
                !crate::keymap::default_label(name).is_empty(),
                "{name} has no default key"
            );
            rest = after;
        }
    }
}
