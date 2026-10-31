use std::{env, fs, path::Path, path::PathBuf, process::Command};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    version(&root);
    webauthn();
    cursor();

    let vt = root.join("vendor/ghostty-vt-out");
    let lib = vt.join("lib/libghostty-vt.a");
    let header = root.join("src/ghostty/wrapper.h");
    let bindings = out.join("ghostty_vt.rs");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:rerun-if-changed=vendor/ghostty-vt-out/lib/libghostty-vt.a");
    println!("cargo:rerun-if-changed=src/ghostty/wrapper.h");
    // This script also reruns on every source edit (for the version's dirty
    // mark), so skip the slow part when its inputs haven't changed.
    if newer(&bindings, &[&lib, &header]) {
        return;
    }

    // Link the static archive only (the lib dir also has a dylib the linker would prefer).
    fs::copy(&lib, out.join("libghostty-vt.a")).expect("build libghostty-vt first");
    bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", vt.join("include").display()))
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen libghostty-vt")
        .write_to_file(&bindings)
        .unwrap();
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

/// The native WebAuthn bridge (src/webauthn.m), linked with the frameworks it uses.
fn webauthn() {
    println!("cargo:rerun-if-changed=src/webauthn.m");
    cc::Build::new()
        .file("src/webauthn.m")
        .flag("-fobjc-arc")
        .flag("-mmacosx-version-min=13.0")
        .compile("abromux-webauthn");
    for framework in [
        "AuthenticationServices",
        "CoreBluetooth",
        "Security",
        "WebKit",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

fn newer(out: &Path, inputs: &[&Path]) -> bool {
    let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(t) = mtime(out) else { return false };
    inputs.iter().all(|i| mtime(i).is_some_and(|m| m <= t))
}

/// The cursor guard for browser panes (src/cursor.m).
fn cursor() {
    println!("cargo:rerun-if-changed=src/cursor.m");
    cc::Build::new()
        .file("src/cursor.m")
        .flag("-fobjc-arc")
        .flag("-mmacosx-version-min=13.0")
        .compile("abromux-cursor");
}
