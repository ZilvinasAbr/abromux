//! Running git, the same way everywhere in abromux and its plugins. Much
//! of it runs in the background (checking branches, reading diffs), so it
//! never takes git's optional locks: your own `git` in a terminal is
//! never refused the index lock because of abromux.

use std::{path::Path, process::Command};

/// `git -C dir args`: its stdout, or why it failed (git's stderr, trimmed).
/// Paths come out unquoted and nothing is coloured.
pub fn run(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotePath=false", "-c", "color.ui=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// [`run`]'s stdout as text, trimmed.
pub fn text(dir: &Path, args: &[&str]) -> Result<String, String> {
    run(dir, args).map(|o| String::from_utf8_lossy(&o).trim().to_string())
}
