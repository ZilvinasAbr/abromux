//! Workspace folders (⇧⌘I): a workspace that has its own folder under
//! [`channel::workspaces_dir`]. It's a scoped place for an agent to work in:
//! projects from elsewhere on the machine are checked out into it, a git
//! worktree for each repo (`abromux folder add <repo>`), a copy otherwise.
//!
//! The folder is the source of truth: `.abromux/workspace.json` names it and
//! lists what was checked out, and an `AGENTS.md` (with a `CLAUDE.md` that
//! imports it) tells the agent how to work there.

use crate::{channel, settings::Settings};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const META: &str = ".abromux/workspace.json";

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct Meta {
    /// The name it was made with. Renaming the workspace later doesn't move
    /// the folder: shells and worktrees point into it.
    pub name: String,
    /// Seconds since the epoch.
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub checkouts: Vec<Checkout>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Checkout {
    /// The folder in the workspace, relative to it.
    pub path: String,
    /// Where it came from: the repo's top level, or the copied folder.
    pub source: String,
    /// The worktree's branch; None for a copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

const AGENTS_MD: &str = "\
# AbroMux workspace

This folder is a scoped workspace made by AbroMux. Do your work here, not in
the projects' own folders elsewhere on this machine.

To work on a project, check it out into this folder first:

    abromux folder add <path-to-project> [--branch <name>]

For a git repo that makes a worktree here on a new branch (`ws/<folder>` by
default), so the original checkout and its branch stay untouched. A folder
that isn't a git repo is copied. `abromux folder list` shows what is checked
out; `abromux folder repos` lists git repos it knows of on this machine.

Commit in the worktrees as usual: the commits land in the original repo.
";

/// The workspace folder `path` is in, if any: the nearest folder holding
/// `.abromux/workspace.json`.
pub fn find(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|p| p.join(META).is_file())
        .map(Path::to_path_buf)
}

pub fn read_meta(dir: &Path) -> Option<Meta> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(META)).ok()?).ok()
}

fn write_meta(dir: &Path, meta: &Meta) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join(".abromux"))?;
    let json = serde_json::to_string_pretty(meta).map_err(std::io::Error::other)?;
    std::fs::write(dir.join(META), json + "\n")
}

/// A folder name for `name`: lowercase letters, digits and dashes.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        "workspace".into()
    } else {
        out.into()
    }
}

/// `base/name`, or `base/name-2`, `-3`… if that's taken.
fn unique(base: &Path, name: &str) -> PathBuf {
    let first = base.join(name);
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| base.join(format!("{name}-{n}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// A name like `quiet-amber-falcon`, for a workspace not named yet.
pub fn random_name() -> String {
    const ADJ: &[&str] = &[
        "amber", "brave", "calm", "crisp", "dusty", "eager", "fuzzy", "gentle", "hidden", "jolly",
        "keen", "lively", "mellow", "nimble", "quiet", "rapid", "rusty", "silent", "sunny",
        "swift", "tidy", "vivid", "witty", "zesty",
    ];
    const NOUN: &[&str] = &[
        "anchor", "badger", "beacon", "canyon", "comet", "falcon", "garden", "harbor", "island",
        "lantern", "meadow", "otter", "pebble", "quarry", "river", "summit", "thicket", "tundra",
        "valley", "willow", "orchard", "glacier", "ember", "forge",
    ];
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded per process; mix in the time for repeat calls.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    let r = h.finish() as usize;
    format!(
        "{}-{}-{}",
        ADJ[r % ADJ.len()],
        ADJ[(r / 31) % ADJ.len()],
        NOUN[(r / 997) % NOUN.len()]
    )
}

/// Make a workspace folder for `name` under `base`. Returns its path.
pub fn create_in(base: &Path, name: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(base)?;
    let dir = unique(base, &slug(name));
    std::fs::create_dir(&dir)?;
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    write_meta(
        &dir,
        &Meta {
            name: name.into(),
            created,
            checkouts: vec![],
        },
    )?;
    std::fs::write(dir.join("AGENTS.md"), AGENTS_MD)?;
    std::fs::write(dir.join("CLAUDE.md"), "@AGENTS.md\n")?;
    Ok(dir)
}

/// Where workspace folders go: `$ABROMUX_WORKSPACES_DIR`, else
/// `app.workspaceFoldersDirectory`, else the channel's `~/.abromux/ws`.
pub fn root(s: &Settings) -> PathBuf {
    let set = std::env::var("ABROMUX_WORKSPACES_DIR")
        .ok()
        .or_else(|| s.app.workspace_folders_directory.clone())
        .filter(|d| !d.trim().is_empty());
    match set {
        Some(d) => expand(d.trim()),
        None => channel::workspaces_dir(),
    }
}

/// `~` and `~/…` from home.
fn expand(path: &str) -> PathBuf {
    let home = || PathBuf::from(std::env::var("HOME").unwrap_or_default());
    match path.strip_prefix('~') {
        Some("") => home(),
        Some(rest) if rest.starts_with('/') => home().join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Check `source` out into workspace folder `ws`: a worktree on `branch`
/// (default `ws/<folder>`) for a git repo, a copy for anything else.
pub fn add(ws: &Path, source: &Path, branch: Option<&str>) -> Result<Checkout, String> {
    let mut meta =
        read_meta(ws).ok_or_else(|| format!("{}: not a workspace folder", ws.display()))?;
    let source = source
        .canonicalize()
        .map_err(|e| format!("{}: {e}", source.display()))?;
    if source.starts_with(ws) {
        return Err(format!("{} is already in this workspace", source.display()));
    }
    let top = git(&source, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from);
    let from = top.clone().unwrap_or_else(|| source.clone());
    let name = from
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let dest = unique(ws, &name);
    let rel = dest.file_name().unwrap().to_string_lossy().into_owned();
    let checkout = match top {
        Some(top) => {
            let ws_name = ws
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let branch = branch
                .map(String::from)
                .unwrap_or_else(|| format!("ws/{ws_name}"));
            let dest_s = dest.to_string_lossy();
            let exists = git(
                &top,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
            )
            .is_ok();
            let args: Vec<&str> = if exists {
                vec!["worktree", "add", &dest_s, &branch]
            } else {
                vec!["worktree", "add", "-b", &branch, &dest_s]
            };
            git(&top, &args)?;
            Checkout {
                path: rel,
                source: top.to_string_lossy().into_owned(),
                branch: Some(branch),
            }
        }
        None => {
            if !source.is_dir() {
                return Err(format!("{}: not a folder", source.display()));
            }
            let out = Command::new("/bin/cp")
                .arg("-Rp")
                .arg(&source)
                .arg(&dest)
                .output()
                .map_err(|e| format!("cp: {e}"))?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            Checkout {
                path: rel,
                source: source.to_string_lossy().into_owned(),
                branch: None,
            }
        }
    };
    meta.checkouts.push(checkout.clone());
    write_meta(ws, &meta).map_err(|e| e.to_string())?;
    Ok(checkout)
}

/// Every workspace folder, oldest first.
pub fn list(root: &Path) -> Vec<(PathBuf, Meta)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return vec![];
    };
    let mut out: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| read_meta(&p).map(|m| (p, m)))
        .collect();
    out.sort_by_key(|(p, m)| (m.created, p.clone()));
    out
}

/// Git repos to check out from: the folders right under the usual project
/// roots (`~/repos`, `~/src`, `~/code`, `~/projects`, `~/dev`, `~/Developer`)
/// that hold a `.git`.
pub fn known_repos() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let mut out = vec![];
    for root in ["repos", "src", "code", "projects", "dev", "Developer"] {
        let Ok(entries) = std::fs::read_dir(home.join(root)) else {
            continue;
        };
        let mut found: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join(".git").exists())
            .collect();
        found.sort();
        out.extend(found);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("abromux-folder-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn root_takes_the_setting() {
        let mut s = Settings::default();
        s.app.workspace_folders_directory = Some("~/work/ws".into());
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        if std::env::var("ABROMUX_WORKSPACES_DIR").is_err() {
            assert_eq!(root(&s), home.join("work/ws"));
            s.app.workspace_folders_directory = Some("/tmp/x".into());
            assert_eq!(root(&s), PathBuf::from("/tmp/x"));
            s.app.workspace_folders_directory = None;
            assert!(root(&s).starts_with(home.join(".abromux")));
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("My API work!"), "my-api-work");
        assert_eq!(slug("  --  "), "workspace");
        assert_eq!(slug("quiet-amber-falcon"), "quiet-amber-falcon");
    }

    #[test]
    fn random_names_are_slugs() {
        let n = random_name();
        assert_eq!(slug(&n), n);
        assert_eq!(n.split('-').count(), 3);
    }

    #[test]
    fn create_writes_meta_and_agents_and_keeps_names_apart() {
        let base = tmp("create");
        let a = create_in(&base, "Fix login").unwrap();
        let b = create_in(&base, "Fix login").unwrap();
        assert_eq!(a, base.join("fix-login"));
        assert_eq!(b, base.join("fix-login-2"));
        assert_eq!(read_meta(&a).unwrap().name, "Fix login");
        assert!(a.join("AGENTS.md").is_file());
        std::fs::create_dir_all(a.join("x/y")).unwrap();
        assert_eq!(find(&a.join("x/y")), Some(a.clone()));
        assert_eq!(find(&base), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn add_copies_a_plain_folder() {
        let base = tmp("copy");
        let ws = create_in(&base.join("ws"), "w").unwrap();
        let src = base.join("notes");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.txt"), "hi").unwrap();
        let c = add(&ws, &src, None).unwrap();
        assert_eq!(c.branch, None);
        assert_eq!(
            std::fs::read_to_string(ws.join("notes/a.txt")).unwrap(),
            "hi"
        );
        assert_eq!(read_meta(&ws).unwrap().checkouts, vec![c]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn add_makes_a_worktree_for_a_repo() {
        let base = tmp("git");
        let repo = base.join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| git(&repo, args).unwrap();
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("f"), "x").unwrap();
        run(&["add", "f"]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ]);
        let ws = create_in(&base.join("ws"), "Login fix").unwrap();
        // From a subfolder: the whole repo is checked out.
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        let c = add(&ws, &repo.join("sub"), None).unwrap();
        assert_eq!(c.path, "proj");
        assert_eq!(c.branch.as_deref(), Some("ws/login-fix"));
        assert!(ws.join("proj/f").is_file());
        assert_eq!(
            git(&ws.join("proj"), &["branch", "--show-current"]).unwrap(),
            "ws/login-fix"
        );
        // Once more: the branch is checked out already, so git says no.
        assert!(add(&ws, &repo, None).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
