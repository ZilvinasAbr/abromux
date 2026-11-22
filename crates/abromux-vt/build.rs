//! libghostty-vt: link the static library `scripts/build-libghostty.sh`
//! builds into vendor/ghostty-vt-out, and generate its bindings.

use std::{env, fs, path::PathBuf};

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = crate_dir.join("../..");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let vt = root.join("vendor/ghostty-vt-out");
    let lib = vt.join("lib/libghostty-vt.a");
    let header = crate_dir.join("src/wrapper.h");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:rerun-if-changed={}", lib.display());
    println!("cargo:rerun-if-changed=src/wrapper.h");

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
        .write_to_file(out.join("ghostty_vt.rs"))
        .unwrap();
}
