//! The native (Objective-C) parts, compiled with the frameworks they use.
//! Nothing to build on other systems.

fn main() {
    // channel.rs reads it with option_env!.
    println!("cargo:rerun-if-env-changed=ABROMUX_CHANNEL");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    objc("browser_native", "abromux-browser-native");
    objc("cursor", "abromux-cursor");
    objc("webauthn", "abromux-webauthn");
    for framework in [
        "AppKit",
        "AuthenticationServices",
        "CoreBluetooth",
        "Security",
        "WebKit",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

/// Compile `src/macos/<name>.m` into the static library `lib`.
fn objc(name: &str, lib: &str) {
    let file = format!("src/macos/{name}.m");
    println!("cargo:rerun-if-changed={file}");
    cc::Build::new()
        .file(&file)
        .flag("-fobjc-arc")
        .flag("-mmacosx-version-min=13.0")
        .compile(lib);
}
