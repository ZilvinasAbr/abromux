//! Stable vs nightly. `scripts/bundle-app.sh --nightly` builds "AbroMux
//! Nightly.app" with the bundle id `dev.abromux.nightly`; a binary inside it
//! uses its own name, neon icon and support folder (socket, session), so it
//! runs side by side with the signed release. `--name foo` makes a named
//! nightly, "AbroMux Nightly foo.app" (`dev.abromux.nightly.foo`), that runs
//! beside both. Anything else is stable.

use std::sync::OnceLock;

pub const NIGHTLY_ID: &str = "dev.abromux.nightly";

/// None for stable, Some("") for the nightly, Some(name) for a named one.
pub fn flavor() -> Option<&'static str> {
    static FLAVOR: OnceLock<Option<String>> = OnceLock::new();
    FLAVOR
        .get_or_init(|| {
            let exe = std::env::current_exe()
                .and_then(|e| e.canonicalize())
                .ok()?;
            let app = exe
                .ancestors()
                .find(|p| p.extension().is_some_and(|e| e == "app"))?;
            let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
            flavor_of(&plist).map(str::to_string)
        })
        .as_deref()
}

/// The bundle id of the app this binary is in; None outside an app bundle.
pub fn bundle_id() -> Option<String> {
    let exe = std::env::current_exe()
        .and_then(|e| e.canonicalize())
        .ok()?;
    let app = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))?;
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
    id_of(&plist).map(str::to_string)
}

/// The bundle id in an Info.plist (bundle-app.sh writes it on one line).
fn id_of(plist: &str) -> Option<&str> {
    plist
        .split("<key>CFBundleIdentifier</key><string>")
        .nth(1)?
        .split('<')
        .next()
}

/// The flavor an Info.plist's bundle id names (bundle-app.sh writes it on one line).
fn flavor_of(plist: &str) -> Option<&str> {
    match id_of(plist)?.strip_prefix(NIGHTLY_ID)? {
        "" => Some(""),
        rest => rest.strip_prefix('.'),
    }
}

pub fn nightly() -> bool {
    flavor().is_some()
}

pub fn app_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| match flavor() {
        None => "AbroMux".into(),
        Some("") => "AbroMux Nightly".into(),
        Some(name) => format!("AbroMux Nightly {name}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavors() {
        let plist = |id: &str| format!("<key>CFBundleIdentifier</key><string>{id}</string>");
        assert_eq!(flavor_of(&plist("dev.abromux.app")), None);
        assert_eq!(flavor_of(&plist("dev.abromux.nightly")), Some(""));
        assert_eq!(flavor_of(&plist("dev.abromux.nightly.zoom")), Some("zoom"));
        assert_eq!(flavor_of(&plist("dev.abromux.nightlyx")), None);
    }
}
