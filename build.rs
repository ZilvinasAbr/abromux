use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vt = root.join("vendor/ghostty-vt-out");
    // Link the static archive only (the lib dir also has a dylib the linker would prefer).
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    std::fs::copy(vt.join("lib/libghostty-vt.a"), out.join("libghostty-vt.a")).expect("build libghostty-vt first");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:rerun-if-changed=vendor/ghostty-vt-out/lib/libghostty-vt.a");
    println!("cargo:rerun-if-changed=src/ghostty/wrapper.h");

    bindgen::Builder::default()
        .header(root.join("src/ghostty/wrapper.h").to_string_lossy())
        .clang_arg(format!("-I{}", vt.join("include").display()))
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen libghostty-vt")
        .write_to_file(PathBuf::from(env::var("OUT_DIR").unwrap()).join("ghostty_vt.rs"))
        .unwrap();
}
