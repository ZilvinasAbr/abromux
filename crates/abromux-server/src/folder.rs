//! Workspace folders on the server's machine (docs/server-plan.md, phase
//! 2): the app's workspace folders (src/lua/workspace_folders.lua: the same
//! files, the same rules), made, filled and removed where the workspace's
//! terminals run. For a workspace on a VPS, its folder and the worktrees in
//! it are the VPS's.
//!
//! `abromux-server folder new|add|list|all|path|remove`, and
//! `{"op":"folder","args":[..]}` on the server's socket (what the app's
//! `new-workspace --host H --folder NAME` asks of H).

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const META: &str = ".abromux/workspace.json";
const LUA: &str = include_str!("../../../src/lua/workspace_folders.lua");

/// The AGENTS.md a folder starts with: the app's, from its Lua module.
fn agents_md() -> &'static str {
    let start = LUA
        .find("M.AGENTS_MD = [==[\n")
        .map(|i| i + "M.AGENTS_MD = [==[\n".len());
    match start.and_then(|s| LUA[s..].find("]==]").map(|e| &LUA[s..s + e])) {
        Some(t) => t,
        None => "# AbroMux workspace\n",
    }
}

/// Where folders go: `$ABROMUX_WORKSPACES_DIR`, else `~/.abromux/ws`.
pub fn root() -> PathBuf {
    std::env::var_os("ABROMUX_WORKSPACES_DIR")
        .filter(|v| !v.is_empty())
        .map(|v| abromux_core::paths::expand_tilde(&v.to_string_lossy()))
        .unwrap_or_else(|| abromux_core::paths::home().join(".abromux/ws"))
}

pub use abromux_core::names::slug;

fn unique(base: &Path, name: &str) -> PathBuf {
    let mut p = base.join(name);
    let mut n = 1;
    while p.exists() {
        n += 1;
        p = base.join(format!("{name}-{n}"));
    }
    p
}

pub fn read_meta(dir: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(dir.join(META)).ok()?).ok()
}

fn write_meta(dir: &Path, meta: &Value) -> Result<(), String> {
    std::fs::create_dir_all(dir.join(".abromux")).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(META), serde_json::to_string_pretty(meta).unwrap())
        .map_err(|e| e.to_string())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A new folder named `name` under [`root`]: its path.
pub fn create(name: &str) -> Result<PathBuf, String> {
    let base = root();
    std::fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    let dir = unique(&base, &slug(name));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    write_meta(
        &dir,
        &json!({ "name": name, "created": now(), "checkouts": [] }),
    )?;
    std::fs::write(dir.join("AGENTS.md"), agents_md()).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("CLAUDE.md"), "@AGENTS.md\n").map_err(|e| e.to_string())?;
    Ok(dir)
}

/// The folder `path` is in: the nearest one holding its META.
pub fn find(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|p| p.join(META).exists())
        .map(Path::to_path_buf)
}

/// The folder this shell is in.
pub fn current() -> Result<PathBuf, String> {
    if let Some(d) = std::env::var_os("ABROMUX_WORKSPACE_DIR").filter(|d| !d.is_empty()) {
        return Ok(d.into());
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    find(&cwd).ok_or_else(|| "not in a workspace folder (make one with `folder new <name>`)".into())
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

/// Check `source` out into folder `ws`: a worktree on `branch` (default
/// `ws/<folder>`) for a git repo, a copy otherwise. The checkout's entry.
pub fn add(ws: &Path, source: &Path, branch: Option<&str>) -> Result<Value, String> {
    let mut meta =
        read_meta(ws).ok_or_else(|| format!("{}: not a workspace folder", ws.display()))?;
    let src = source
        .canonicalize()
        .map_err(|e| format!("{}: {e}", source.display()))?;
    if src.starts_with(ws) {
        return Err(format!("{} is already in this workspace", src.display()));
    }
    let top = git(&src, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from);
    let name = top
        .as_deref()
        .unwrap_or(&src)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dest = unique(ws, &name);
    let mut c = json!({
        "path": dest.file_name().unwrap().to_string_lossy(),
        "source": top.as_deref().unwrap_or(&src),
    });
    if let Some(top) = &top {
        let ws_name = ws.file_name().unwrap().to_string_lossy();
        let branch = branch
            .map(String::from)
            .unwrap_or_else(|| format!("ws/{ws_name}"));
        let had = git(
            top,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok();
        let d = dest.to_string_lossy();
        if had {
            git(top, &["worktree", "add", &d, &branch])?;
        } else {
            git(top, &["worktree", "add", "-b", &branch, &d])?;
        }
        c["branch"] = json!(branch);
        if !had {
            c["new_branch"] = json!(true);
        }
    } else {
        let st = Command::new("cp")
            .arg("-Rp")
            .arg(&src)
            .arg(&dest)
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            return Err(format!("cp {}: {st}", src.display()));
        }
    }
    meta["checkouts"]
        .as_array_mut()
        .ok_or("bad workspace.json")?
        .push(c.clone());
    write_meta(ws, &meta)?;
    Ok(c)
}

/// Why removing `ws` would lose work: its worktrees' uncommitted files.
pub fn blockers(ws: &Path) -> Vec<String> {
    let meta = read_meta(ws).unwrap_or_default();
    meta["checkouts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["branch"].is_string())
        .filter_map(|c| {
            let path = c["path"].as_str()?;
            let n = git(&ws.join(path), &["status", "--porcelain"])
                .ok()?
                .lines()
                .count();
            (n > 0).then(|| format!("{path}: {n} uncommitted or untracked file(s)"))
        })
        .collect()
}

/// Delete folder `ws`: each worktree out of its repo (and the branch `add`
/// made, if merged), then the folder. Without `force` it refuses while a
/// worktree has changes. What it did, a line each.
pub fn remove(ws: &Path, force: bool) -> Result<Vec<String>, String> {
    let meta = read_meta(ws).ok_or_else(|| format!("{}: not a workspace folder", ws.display()))?;
    if ws == Path::new("/") || ws == abromux_core::paths::home() {
        return Err(format!("{}: won't delete that", ws.display()));
    }
    if !force {
        let b = blockers(ws);
        if !b.is_empty() {
            return Err(format!(
                "work would be lost (commit it, or --force to drop it):\n  {}",
                b.join("\n  ")
            ));
        }
    }
    let mut done = vec![];
    for c in meta["checkouts"].as_array().into_iter().flatten() {
        let path = c["path"].as_str().unwrap_or_default();
        let source = Path::new(c["source"].as_str().unwrap_or_default());
        let Some(branch) = c["branch"].as_str() else {
            done.push(format!("{path}: deleted (a copy of {})", source.display()));
            continue;
        };
        if !source.is_dir() {
            done.push(format!(
                "{path}: {} is gone, deleted its files",
                source.display()
            ));
            continue;
        }
        let wt = ws.join(path).to_string_lossy().into_owned();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&wt);
        git(source, &args).map_err(|e| format!("{path}: {e}"))?;
        let note = if c["new_branch"] != true {
            format!("kept branch {branch} (it was there before)")
        } else if git(source, &["branch", "-d", branch]).is_ok() {
            format!("deleted branch {branch} (merged)")
        } else {
            format!("kept branch {branch} (not merged)")
        };
        done.push(format!(
            "{path}: removed the worktree from {}, {note}",
            source.display()
        ));
    }
    std::fs::remove_dir_all(ws).map_err(|e| format!("{}: {e}", ws.display()))?;
    done.push(format!("deleted {}", ws.display()));
    Ok(done)
}

/// Every folder under [`root`], oldest first.
pub fn all() -> Vec<(PathBuf, Value)> {
    let mut out: Vec<(PathBuf, Value)> = std::fs::read_dir(root())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| read_meta(&e.path()).map(|m| (e.path(), m)))
        .collect();
    out.sort_by_key(|(p, m)| (m["created"].as_u64().unwrap_or(0), p.clone()));
    out
}

/// `folder <args>`: the command's output lines, or why not.
pub fn command(args: &[String]) -> Result<Vec<String>, String> {
    let flag = |name: &str| args.iter().position(|a| a == name);
    let value = |name: &str| flag(name).and_then(|i| args.get(i + 1)).map(String::as_str);
    let words: Vec<&str> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && !(*i > 0 && args[i - 1] == "--branch"))
        .map(|(_, a)| a.as_str())
        .collect();
    match words.first().copied().unwrap_or("path") {
        "new" => {
            let name = words
                .get(1..)
                .map(|w| w.join(" "))
                .filter(|n| !n.is_empty())
                .ok_or("folder new: missing <name>")?;
            Ok(vec![create(&name)?.display().to_string()])
        }
        "add" => {
            let src = words.get(1).ok_or("folder add: missing <path>")?;
            let src = abromux_core::paths::expand_tilde(src);
            let c = add(&current()?, &src, value("--branch"))?;
            Ok(vec![format!(
                "OK {} ({})",
                c["path"].as_str().unwrap_or_default(),
                c["branch"]
                    .as_str()
                    .map_or("a copy".into(), |b| format!("worktree on {b}"))
            )])
        }
        "list" => {
            let ws = current()?;
            let meta = read_meta(&ws).unwrap_or_default();
            Ok(meta["checkouts"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    format!(
                        "{}  {}  from {}",
                        c["path"].as_str().unwrap_or_default(),
                        c["branch"]
                            .as_str()
                            .map_or("copy".into(), |b| format!("worktree {b}")),
                        c["source"].as_str().unwrap_or_default()
                    )
                })
                .collect())
        }
        "all" => Ok(all()
            .into_iter()
            .map(|(p, m)| {
                format!(
                    "{}  {}",
                    p.display(),
                    m["name"].as_str().unwrap_or_default()
                )
            })
            .collect()),
        "path" => Ok(vec![current()?.display().to_string()]),
        "root" => Ok(vec![root().display().to_string()]),
        "remove" => {
            let ws = match words.get(1) {
                Some(p) => abromux_core::paths::expand_tilde(p),
                None => current()?,
            };
            remove(&ws, flag("--force").is_some())
        }
        other => Err(format!(
            "folder: unknown command {other:?} (new, add, list, all, path, root, remove)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_with_a_worktree_comes_and_goes() {
        let base = std::env::temp_dir().join(format!("abx-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // SAFETY: tests here don't read it otherwise.
        unsafe { std::env::set_var("ABROMUX_WORKSPACES_DIR", base.join("ws")) };
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let g = |args: &[&str]| git(&repo, args).unwrap();
        g(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("README"), "hi\n").unwrap();
        g(&["add", "."]);
        g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "first",
        ]);

        let ws = create("Fix the Login!").unwrap();
        assert_eq!(ws.file_name().unwrap(), "fix-the-login");
        assert!(
            std::fs::read_to_string(ws.join("AGENTS.md"))
                .unwrap()
                .contains("abromux folder add")
        );
        assert_eq!(
            create("Fix the Login!").unwrap().file_name().unwrap(),
            "fix-the-login-2"
        );

        let c = add(&ws, &repo, None).unwrap();
        assert_eq!(c["branch"], "ws/fix-the-login");
        assert!(ws.join("repo/README").exists());
        // Uncommitted work holds the removal back.
        std::fs::write(ws.join("repo/new.txt"), "x").unwrap();
        assert!(
            remove(&ws, false)
                .unwrap_err()
                .contains("work would be lost")
        );
        std::fs::remove_file(ws.join("repo/new.txt")).unwrap();
        let done = remove(&ws, false).unwrap();
        assert!(
            done.iter()
                .any(|l| l.contains("deleted branch ws/fix-the-login (merged)")),
            "{done:?}"
        );
        assert!(!ws.exists());
        assert_eq!(all().len(), 1, "the other one is still there");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn slugs_and_agents_md() {
        assert_eq!(slug("Quiet Amber!"), "quiet-amber");
        assert_eq!(slug("!!!"), "workspace");
        assert!(agents_md().starts_with("# AbroMux workspace"));
    }
}
