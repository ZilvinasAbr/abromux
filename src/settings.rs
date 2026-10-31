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
//!   },
//!   "shortcuts": {
//!     "splitRight": "cmd-\\",
//!     "closeTab": null,
//!     "flashPane": ["cmd-shift-h", "ctrl-a f"],
//!     "find": { "keys": "cmd-shift-f", "when": "Terminal" }
//!   },
//!   "commands": [
//!     { "title": "Run Tests", "run": "cargo test", "shortcut": "cmd-alt-t" },
//!     { "title": "Dev Server", "run": "npm run dev", "newTab": true },
//!     { "title": "Open Docs", "url": "https://docs.rs" }
//!   ]
//! }
//! ```
//!
//! `"shortcuts"` maps the action names in `keymap::ACTIONS` to new keys;
//! `null` or `[]` unbinds one.

use crate::theme::{ACCENT, BORDER};
use gpui::Keystroke;
use gpui::{Global, Rgba, rgb, rgba};
use gpui_component::notification::Notification;
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Settings {
    /// For editors that offer completion from a JSON schema.
    #[serde(rename = "$schema")]
    pub schema: Option<String>,
    pub panes: Panes,
    /// Keys for the app's actions, by `keymap::ACTIONS` name.
    #[serde(deserialize_with = "shortcuts")]
    pub shortcuts: BTreeMap<String, Shortcut>,
    /// Extra palette commands.
    pub commands: Vec<Command>,
}

impl Global for Settings {}

/// The keys bound to an action or command.
#[derive(Clone, Debug, PartialEq)]
pub struct Shortcut {
    /// Each one a keystroke, or a chord of several.
    pub keys: Vec<Vec<Keystroke>>,
    /// The key context it's bound in, like `Terminal`; by default the
    /// action's own contexts (app-wide for a command).
    pub when: Option<String>,
}

/// A palette command from the config file. It types `run` into the focused
/// terminal and presses return, or opens `url` in a new browser tab.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Command {
    pub title: String,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Run it in a new terminal tab instead of the focused terminal.
    #[serde(default)]
    pub new_tab: bool,
    #[serde(default, deserialize_with = "shortcut")]
    pub shortcut: Option<Shortcut>,
}

/// How the config file writes a shortcut: `"cmd-k"`, `["cmd-k", "ctrl-a k"]`,
/// `{ "keys": …, "when": "Terminal" }`, or `null` for none.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawKeys {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawShortcut {
    Keys(RawKeys),
    Full { keys: RawKeys, when: Option<String> },
}

impl TryFrom<Option<RawShortcut>> for Shortcut {
    type Error = String;

    fn try_from(raw: Option<RawShortcut>) -> Result<Self, String> {
        let (keys, when) = match raw {
            None => (RawKeys::Many(vec![]), None),
            Some(RawShortcut::Keys(k)) => (k, None),
            Some(RawShortcut::Full { keys, when }) => (keys, when),
        };
        let keys = match keys {
            RawKeys::One(k) => vec![k],
            RawKeys::Many(ks) => ks,
        };
        if let Some(w) = &when {
            crate::keymap::check_context(w)?;
        }
        Ok(Shortcut {
            keys: keys
                .iter()
                .map(|k| crate::keymap::parse_seq(k))
                .collect::<Result<_, _>>()?,
            when,
        })
    }
}

fn shortcuts<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, Shortcut>, D::Error> {
    let raw = BTreeMap::<String, Option<RawShortcut>>::deserialize(d)?;
    raw.into_iter()
        .map(|(name, s)| {
            if crate::keymap::find(&name).is_none() {
                return Err(format!(
                    "unknown shortcut `{name}`; `abromux config shortcuts` lists them"
                ));
            }
            let s = Shortcut::try_from(s).map_err(|e| format!("shortcut `{name}`: {e}"))?;
            Ok((name, s))
        })
        .collect::<Result<_, String>>()
        .map_err(serde::de::Error::custom)
}

fn shortcut<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Shortcut>, D::Error> {
    Option::<RawShortcut>::deserialize(d)?
        .map(|r| Shortcut::try_from(Some(r)))
        .transpose()
        .map_err(serde::de::Error::custom)
}

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
    let s: Settings = serde_json::from_str(text)?;
    for c in &s.commands {
        if c.run.is_some() == c.url.is_some() {
            return Err(serde::de::Error::custom(format!(
                "command {:?} needs one of \"run\" or \"url\"",
                c.title
            )));
        }
    }
    Ok(s)
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
    fn shortcuts() {
        let s = parse(
            r#"{
              "shortcuts": {
                "splitRight": "cmd-\\",
                "closeTab": null,
                "flashPane": ["⌘⇧H", "ctrl-a f"],
                "find": { "keys": "cmd-shift-f", "when": "Terminal" }
              }
            }"#,
        )
        .unwrap();
        let sc = &s.shortcuts;
        assert_eq!(sc["splitRight"].keys.len(), 1);
        assert!(sc["closeTab"].keys.is_empty());
        assert_eq!(sc["flashPane"].keys[1].len(), 2);
        assert_eq!(sc["find"].when.as_deref(), Some("Terminal"));
    }

    #[test]
    fn shortcut_errors() {
        let err = |text: &str| parse(text).unwrap_err().to_string();
        let e = err(r#"{"shortcuts": {"splitLeft": "cmd-d"}}"#);
        assert!(e.contains("unknown shortcut `splitLeft`"), "{e}");
        let e = err(r#"{"shortcuts": {"splitRight": "cmd-dd"}}"#);
        assert!(
            e.contains("shortcut `splitRight`") && e.contains("isn't a keystroke"),
            "{e}"
        );
        let e = err(r#"{"shortcuts": {"find": {"keys": "cmd-f", "when": "a &&"}}}"#);
        assert!(e.contains("key context"), "{e}");
    }

    #[test]
    fn commands() {
        let s = parse(
            r#"{"commands": [
              { "title": "Tests", "run": "cargo test", "shortcut": "cmd-alt-t" },
              { "title": "Docs", "url": "https://docs.rs" }
            ]}"#,
        )
        .unwrap();
        assert_eq!(s.commands[0].run.as_deref(), Some("cargo test"));
        assert!(s.commands[0].shortcut.is_some() && !s.commands[0].new_tab);
        assert!(s.commands[1].shortcut.is_none());
        let e = parse(r#"{"commands": [{ "title": "x" }]}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("needs one of"), "{e}");
        let e = parse(r#"{"commands": [{ "title": "x", "run": "a", "url": "b" }]}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("needs one of"), "{e}");
    }

    #[test]
    fn missing_file_is_defaults() {
        let p = Path::new("/nonexistent/abromux/config.json");
        assert_eq!(read(p).unwrap(), Settings::default());
    }
}
