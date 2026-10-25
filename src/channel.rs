//! Stable vs nightly. `scripts/bundle-app.sh --nightly` builds "AbroMux
//! Nightly.app" with the bundle id `dev.abromux.nightly`; a binary inside it
//! uses its own name, neon icon and support folder (socket, session), so it
//! runs side by side with the signed release. Anything else is stable.

use std::{path::PathBuf, sync::OnceLock};

pub const NIGHTLY_ID: &str = "dev.abromux.nightly";

pub fn nightly() -> bool {
    static NIGHTLY: OnceLock<bool> = OnceLock::new();
    *NIGHTLY.get_or_init(|| {
        let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize()) else { return false };
        let Some(app) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) else { return false };
        std::fs::read_to_string(app.join("Contents/Info.plist"))
            .is_ok_and(|plist| plist.contains(&format!("<string>{NIGHTLY_ID}</string>")))
    })
}

pub fn app_name() -> &'static str {
    if nightly() { "AbroMux Nightly" } else { "AbroMux" }
}

/// `~/Library/Application Support/abromux` (or `abromux-nightly`).
pub fn support_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = if nightly() { "abromux-nightly" } else { "abromux" };
    PathBuf::from(home).join("Library/Application Support").join(dir)
}
