//! Where things are under your home folder, worked out one way: what
//! `$HOME` is (and what to use without it), `~` paths, and the config
//! folders abromux reads.

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

/// `~` and `~/…` under [`home`]; anything else as it is.
pub fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home(),
        Some(rest) if rest.starts_with('/') => home().join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

/// `$XDG_CONFIG_HOME` if it's absolute (as the spec asks), else `~/.config`.
pub fn xdg_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
}

/// The Ghostty config files abromux reads, in the order it reads them: a
/// setting in a later one wins.
pub fn ghostty_configs() -> [PathBuf; 2] {
    [
        xdg_config().join("ghostty/config"),
        home().join("Library/Application Support/com.mitchellh.ghostty/config"),
    ]
}

/// The program `name` on PATH, in `~/.local/bin`, `/opt/homebrew/bin` or
/// `/usr/local/bin`: an app opened from the Dock has only the system's PATH.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let home = home();
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain([
            home.join(".local/bin"),
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
        ])
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde() {
        assert_eq!(expand_tilde("~"), home());
        assert_eq!(expand_tilde("~/code"), home().join("code"));
        assert_eq!(expand_tilde("~bob/x"), PathBuf::from("~bob/x"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
        assert!(home().is_absolute() && xdg_config().is_absolute());
    }
}
