//! Stable vs nightly. `scripts/bundle-app.sh --nightly` builds "AbroMux
//! Nightly.app" with the bundle id `dev.abromux.nightly`; a binary inside it
//! uses its own name, neon icon and support folder (socket, session), so it
//! runs side by side with the signed release. `--name foo` makes a named
//! nightly, "AbroMux Nightly foo.app" (`dev.abromux.nightly.foo`), that runs
//! beside both. Anything else is stable.

use std::{path::PathBuf, sync::OnceLock};

pub const NIGHTLY_ID: &str = "dev.abromux.nightly";

/// None for stable, Some("") for the nightly, Some(name) for a named one.
fn flavor() -> Option<&'static str> {
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

/// The flavor an Info.plist's bundle id names (bundle-app.sh writes it on one line).
fn flavor_of(plist: &str) -> Option<&str> {
    let id = plist
        .split("<key>CFBundleIdentifier</key><string>")
        .nth(1)?
        .split('<')
        .next()?;
    match id.strip_prefix(NIGHTLY_ID)? {
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

/// `~/Library/Application Support/abromux` (or `abromux-nightly`, or
/// `abromux-nightly-<name>`).
pub fn support_dir() -> PathBuf {
    support_dir_of(flavor())
}

/// The support folder of another channel, by name: `stable`, `nightly` or
/// `nightly-<name>`. None for anything else.
pub fn support_dir_named(channel: &str) -> Option<PathBuf> {
    match channel {
        "stable" => Some(support_dir_of(None)),
        "nightly" => Some(support_dir_of(Some(""))),
        c => c
            .strip_prefix("nightly-")
            .filter(|n| !n.is_empty())
            .map(|n| support_dir_of(Some(n))),
    }
}

fn support_dir_of(flavor: Option<&str>) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = match flavor {
        None => "abromux".into(),
        Some("") => "abromux-nightly".into(),
        Some(name) => format!("abromux-nightly-{name}"),
    };
    PathBuf::from(home)
        .join("Library/Application Support")
        .join(dir)
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

    #[test]
    fn named_channels() {
        let name = |c: &str| support_dir_named(c).map(|p| p.file_name().unwrap().to_owned());
        assert_eq!(name("stable").unwrap(), "abromux");
        assert_eq!(name("nightly").unwrap(), "abromux-nightly");
        assert_eq!(name("nightly-zoom").unwrap(), "abromux-nightly-zoom");
        assert!(name("nightly-").is_none() && name("./x.json").is_none());
    }
}
