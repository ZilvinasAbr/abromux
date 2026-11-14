use std::{env, path::Path, path::PathBuf, process::Command};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    version(&root);
}

/// Sets ABROMUX_VERSION to the Cargo version plus the commit, e.g.
/// `0.1.0+6dea3c2`, with `-dirty` when tracked files have uncommitted changes.
/// A clean checkout of the release tag (`v0.1.0`) is just `0.1.0`, and so is a
/// build without git (a source tarball). Also sets ABROMUX_COMMIT (the short
/// hash) and ABROMUX_BUILD (the commit count, as in `CFBundleVersion`) for the
/// About window; both are empty without git.
fn version(root: &Path) {
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let mut version = env::var("CARGO_PKG_VERSION").unwrap();
    let (mut commit, mut build) = (String::new(), String::new());
    if let Some(sha) = git(&["rev-parse", "--short", "HEAD"]) {
        commit = sha.clone();
        build = git(&["rev-list", "--count", "HEAD"]).unwrap_or_default();
        let dirty =
            git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
        let tag = format!("v{version}^{{commit}}");
        let release = !dirty && git(&["rev-parse", "--short", &tag]).is_some_and(|t| t == sha);
        if !release {
            version += &format!("+{sha}");
        }
        if dirty {
            version += "-dirty";
        }
        // Rerun on commit, checkout, staging, tagging and edits.
        let mut watch = vec![
            "HEAD".to_string(),
            "index".into(),
            "packed-refs".into(),
            "refs/tags".into(),
        ];
        watch.extend(git(&["symbolic-ref", "-q", "HEAD"]));
        for p in watch {
            if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &p]) {
                println!("cargo:rerun-if-changed={path}");
            }
        }
        println!("cargo:rerun-if-changed=src");
    }
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rustc-env=ABROMUX_VERSION={version}");
    println!("cargo:rustc-env=ABROMUX_COMMIT={commit}");
    println!("cargo:rustc-env=ABROMUX_BUILD={build}");
}
