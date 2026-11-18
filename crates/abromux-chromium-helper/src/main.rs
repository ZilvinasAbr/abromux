//! A Chromium child process: the renderer, GPU, network and other helpers
//! the browser panes' `chromium` engine starts
//! (crates/abromux-browser/src/backend/chromium.rs). `scripts/bundle-app.sh`
//! copies it into AbroMux.app as `abromux Helper.app`, `abromux Helper
//! (Renderer).app` and the rest, next to the framework, which is where
//! Chromium looks for them.
//!
//! It needs no code of its own: everything the app asks of a page goes
//! through the DevTools protocol, so the helper only hands its process to CEF.

#[cfg(target_os = "macos")]
fn main() {
    use cef::{args::Args, library_loader::LibraryLoader};
    let args = Args::new();
    // The renderer's seatbelt sandbox, before anything else runs.
    let _sandbox = {
        let mut sandbox = cef::sandbox::Sandbox::new();
        sandbox.initialize(args.as_main_args());
        sandbox
    };
    // Contents/Frameworks/<helper>.app/Contents/MacOS/<helper> → the
    // framework three folders up.
    let exe = std::env::current_exe().expect("helper path");
    let loader = LibraryLoader::new(&exe, true);
    if !loader.load() {
        eprintln!("abromux-chromium-helper: can't load Chromium Embedded Framework");
        std::process::exit(1);
    }
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let code = cef::execute_process(
        Some(args.as_main_args()),
        None::<&mut cef::App>,
        std::ptr::null_mut(),
    );
    std::process::exit(code.max(0));
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("abromux-chromium-helper: the Chromium engine is macOS-only");
    std::process::exit(1);
}
