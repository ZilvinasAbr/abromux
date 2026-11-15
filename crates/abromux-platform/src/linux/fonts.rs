//! The default fonts: fontconfig's `monospace` and `sans-serif` (Omarchy
//! sets `monospace` to a Nerd Font), each asked for once.

use std::{process::Command, sync::OnceLock};

/// The terminal's font when the Ghostty config names none.
pub fn monospace() -> String {
    static FONT: OnceLock<String> = OnceLock::new();
    FONT.get_or_init(|| fc_match("monospace").unwrap_or_else(|| "DejaVu Sans Mono".into()))
        .clone()
}

/// The UI's font.
pub fn ui() -> String {
    static FONT: OnceLock<String> = OnceLock::new();
    FONT.get_or_init(|| fc_match("sans-serif").unwrap_or_else(|| "DejaVu Sans".into()))
        .clone()
}

/// The family fontconfig picks for `pattern`.
fn fc_match(pattern: &str) -> Option<String> {
    let out = Command::new("fc-match")
        .args(["-f", "%{family[0]}", pattern])
        .output()
        .ok()?;
    let family = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !family.is_empty()).then_some(family)
}

#[cfg(test)]
mod tests {
    #[test]
    fn fontconfig_names_a_font() {
        // Without fontconfig these fall back to DejaVu; either way, a name.
        assert!(!super::monospace().is_empty());
        assert!(!super::ui().is_empty());
    }
}
