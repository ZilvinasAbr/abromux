//! Linux has no NSUserDefaults to import preferences from.

/// Never inside an app bundle, so `prefs::import_once` has nothing to copy.
pub fn bundled() -> bool {
    false
}

pub fn bool(_key: &str) -> bool {
    false
}
