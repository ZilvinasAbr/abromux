//! Where abromux keeps its files: the home folder, and each channel's
//! support folder and workspace folders (see [`crate::channel`]).

use crate::channel::flavor;
use std::path::PathBuf;

/// `$HOME`, if it's set to an absolute path.
pub fn home_var() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// `$HOME`, else `/tmp`: absolute and writable, so a path made from it is
/// never relative to wherever abromux was started.
pub fn home() -> PathBuf {
    home_var().unwrap_or_else(|| "/tmp".into())
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

/// Where workspace folders (⇧⌘I) live unless `app.workspaceFoldersDirectory`
/// says otherwise: `~/.abromux/ws`, or `~/.abromux/ws-nightly` /
/// `ws-nightly-<name>`, so channels keep apart. No space in it: shells and
/// agents trip on those.
pub fn workspaces_dir() -> PathBuf {
    let dir = match flavor() {
        None => "ws".into(),
        Some("") => "ws-nightly".into(),
        Some(name) => format!("ws-nightly-{name}"),
    };
    home().join(".abromux").join(dir)
}

fn support_dir_of(flavor: Option<&str>) -> PathBuf {
    let dir = match flavor {
        None => "abromux".into(),
        Some("") => "abromux-nightly".into(),
        Some(name) => format!("abromux-nightly-{name}"),
    };
    home().join("Library/Application Support").join(dir)
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
