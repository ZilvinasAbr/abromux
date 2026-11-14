//! The native (Objective-C) parts, compiled with the frameworks they use.

fn main() {
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
