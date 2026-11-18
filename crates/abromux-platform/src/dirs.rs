//! Where abromux keeps its files: the home folder, and each channel's
//! support folder, runtime folder (sockets), state folder (prefs, logs) and
//! workspace folders (see [`crate::channel`]). On macOS the first three are
//! one; on Linux they're the XDG ones.

use crate::channel::flavor;
pub use abromux_core::paths::{home, home_var};
use std::path::PathBuf;

/// `~/Library/Application Support/abromux` on macOS, `$XDG_DATA_HOME/abromux`
/// (`~/.local/share/abromux`) on Linux; or `abromux-nightly`, or
/// `abromux-nightly-<name>`.
pub fn support_dir() -> PathBuf {
    base().join(own_dir_name())
}

/// This app's folder name: its channel's, or `abromux-chromium` for the
/// Chromium edition.
fn own_dir_name() -> String {
    if flavor().is_none() && crate::channel::chromium_edition() {
        "abromux-chromium".into()
    } else {
        dir_name(flavor())
    }
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

/// Where workspace folders (⇧⌘I) live unless `app.workspaceFoldersDirectory`
/// says otherwise: `~/.abromux/ws`, or `~/.abromux/ws-nightly` /
/// `ws-nightly-<name>`, so channels keep apart. No space in it: shells and
/// agents trip on those.
pub fn workspaces_dir() -> PathBuf {
    let dir = match flavor() {
        None if crate::channel::chromium_edition() => "ws-chromium".into(),
        None => "ws".into(),
        Some("") => "ws-nightly".into(),
        Some(name) => format!("ws-nightly-{name}"),
    };
    home().join(".abromux").join(dir)
}

fn support_dir_of(flavor: Option<&str>) -> PathBuf {
    base().join(dir_name(flavor))
}

/// `abromux`, `abromux-nightly` or `abromux-nightly-<name>`.
fn dir_name(flavor: Option<&str>) -> String {
    match flavor {
        None => "abromux".into(),
        Some("") => "abromux-nightly".into(),
        Some(name) => format!("abromux-nightly-{name}"),
    }
}

/// Where the app's sockets go (the control socket, the holder's): the
/// support folder on macOS; on Linux `$XDG_RUNTIME_DIR/abromux` (or
/// `abromux-nightly`, ...), the session's private tmpfs, or the support
/// folder without one.
pub fn runtime_dir() -> PathBuf {
    if cfg!(target_os = "linux")
        && let Some(dir) = xdg("XDG_RUNTIME_DIR")
    {
        return dir.join(own_dir_name());
    }
    support_dir()
}

/// Where the app keeps state that isn't worth backing up, such as prefs and
/// logs: the support folder on macOS; on Linux `$XDG_STATE_HOME/abromux`
/// (`~/.local/state/abromux`, or `abromux-nightly`, ...).
pub fn state_dir() -> PathBuf {
    if cfg!(target_os = "linux") {
        let base = xdg("XDG_STATE_HOME").unwrap_or_else(|| home().join(".local/state"));
        return base.join(own_dir_name());
    }
    support_dir()
}

/// An XDG base directory variable, if set to an absolute path.
fn xdg(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

#[cfg(target_os = "macos")]
fn base() -> PathBuf {
    home().join("Library/Application Support")
}

#[cfg(target_os = "linux")]
fn base() -> PathBuf {
    xdg("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local/share"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_channels() {
        let name = |c: &str| support_dir_named(c).map(|p| p.file_name().unwrap().to_owned());
        assert_eq!(name("stable").unwrap(), "abromux");
        assert_eq!(name("nightly").unwrap(), "abromux-nightly");
        assert_eq!(name("nightly-zoom").unwrap(), "abromux-nightly-zoom");
        assert!(name("nightly-").is_none() && name("./x.json").is_none());
    }
}
