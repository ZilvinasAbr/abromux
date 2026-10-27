//! abromux's own settings, from `$XDG_CONFIG_HOME/abromux/config.json`
//! (default `~/.config/abromux/config.json`). The stable and nightly apps
//! share it, as they share the Ghostty config (`crate::config`).
//!
//! Every key is optional. Unknown keys are errors, so a typo isn't silently
//! ignored. "Reload Config" (⌘⇧,) and `abromux reload-config` read the file
//! again without a restart; a file that doesn't parse leaves the current
//! settings in place. `abromux config check` validates it.
//!
//! ```json
//! {
//!   "panes": {
//!     "resizeStep": 20,
//!     "dividerColor": "#3a3935",
//!     "focusBorderColor": "#c96442b0"
//!   }
//! }
//! ```

use crate::theme::{ACCENT, BORDER};
use gpui::{Global, Rgba, rgb, rgba};
use gpui_component::notification::Notification;
use serde::{Deserialize, Deserializer};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Settings {
    /// For editors that offer completion from a JSON schema.
    #[serde(rename = "$schema")]
    pub schema: Option<String>,
    pub panes: Panes,
}

impl Global for Settings {}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Panes {
    /// Pixels a divider moves per ⌃⇧H/J/K/L press, and `resize-pane`'s
    /// default (cmux's `paneResizeStepPixels`).
    #[serde(deserialize_with = "step")]
    pub resize_step: f32,
    /// The line between split panes.
    #[serde(deserialize_with = "color")]
    pub divider_color: Rgba,
    /// The border around the focused pane when a workspace has splits.
    #[serde(deserialize_with = "color")]
    pub focus_border_color: Rgba,
}

impl Default for Panes {
    fn default() -> Self {
        Self {
            resize_step: 20.,
            divider_color: rgb(BORDER),
            focus_border_color: rgba(ACCENT << 8 | 0xb0),
        }
    }
}

fn step<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = f32::deserialize(d)?;
    if v.is_finite() && v > 0. {
        Ok(v)
    } else {
        Err(serde::de::Error::custom(format!(
            "resizeStep must be a positive number of pixels, got {v}"
        )))
    }
}

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
fn color<'de, D: Deserializer<'de>>(d: D) -> Result<Rgba, D::Error> {
    let s = String::deserialize(d)?;
    Rgba::try_from(s.as_str()).map_err(|_| {
        serde::de::Error::custom(format!(
            "{s:?} isn't a color; use #rgb, #rgba, #rrggbb or #rrggbbaa"
        ))
    })
}

/// The toast for a config file that couldn't be read. It stays until
/// closed, and a newer one replaces it.
pub fn error_notification(e: String) -> Notification {
    Notification::error(e)
        .id::<Settings>()
        .title("Config not loaded")
        .autohide(false)
}

pub fn path() -> PathBuf {
    let xdg = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        });
    xdg.join("abromux/config.json")
}

/// The settings in the config file, or the defaults if there is none.
pub fn load() -> Result<Settings, String> {
    read(&path())
}

pub fn read(path: &Path) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn parse(text: &str) -> Result<Settings, serde_json::Error> {
    // An empty file is a valid "no settings".
    if text.trim().is_empty() {
        return Ok(Settings::default());
    }
    serde_json::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        assert_eq!(parse("").unwrap(), Settings::default());
        assert_eq!(parse("{}").unwrap(), Settings::default());
        assert_eq!(parse(r#"{"panes": {}}"#).unwrap(), Settings::default());
    }

    #[test]
    fn panes() {
        let s = parse(
            r##"{
              "$schema": "https://example.com/schema.json",
              "panes": { "resizeStep": 35.5, "dividerColor": "#ff0000", "focusBorderColor": "#00ff0080" }
            }"##,
        )
        .unwrap();
        assert_eq!(s.panes.resize_step, 35.5);
        assert_eq!(s.panes.divider_color, rgb(0xff0000));
        assert_eq!(s.panes.focus_border_color, rgba(0x00ff0080));
    }

    #[test]
    fn errors_name_the_problem_and_line() {
        let err = |text: &str| parse(text).unwrap_err().to_string();
        let e = err("{\n  \"panes\": { \"resizeStpe\": 10 }\n}");
        assert!(
            e.contains("unknown field `resizeStpe`") && e.contains("line 2"),
            "{e}"
        );
        let e = err(r#"{"panes": {"dividerColor": "red"}}"#);
        assert!(e.contains("\"red\" isn't a color"), "{e}");
        let e = err(r#"{"panes": {"resizeStep": 0}}"#);
        assert!(e.contains("positive"), "{e}");
        assert!(err("{ \"panes\": ").contains("EOF"));
    }

    #[test]
    fn missing_file_is_defaults() {
        let p = Path::new("/nonexistent/abromux/config.json");
        assert_eq!(read(p).unwrap(), Settings::default());
    }
}
