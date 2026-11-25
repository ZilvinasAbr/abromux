//! ABROMUX_VERSION for `abromux-server --version` and `hello`: the app's
//! version (the workspace's Cargo.toml), plus the commit unless it's the
//! release tag's, as the app's own build.rs does.

use std::{env, path::PathBuf, process::Command};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let manifest = root.join("Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(&manifest).unwrap_or_default();
    let mut version = text
        .lines()
        .find_map(|l| {
            l.strip_prefix("version = \"")?
                .strip_suffix('"')
                .map(String::from)
        })
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap());
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    if let Some(sha) = git(&["rev-parse", "--short", "HEAD"]) {
        let tag = format!("v{version}^{{commit}}");
        if git(&["rev-parse", "--short", &tag]).as_deref() != Some(&sha) {
            version += &format!("+{sha}");
        }
        if let Some(head) = git(&["rev-parse", "--path-format=absolute", "--git-path", "HEAD"]) {
            println!("cargo:rerun-if-changed={head}");
        }
    }
    println!("cargo:rustc-env=ABROMUX_VERSION={version}");
}
