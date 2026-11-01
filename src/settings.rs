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
//!   "persistence": "cmux-like",
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
//!   "browser": { "engine": "terminalBrowser" },
//!   "commands": [
//!     { "title": "Run Tests", "run": "cargo test", "shortcut": "cmd-alt-t" },
//!     { "title": "Dev Server", "run": "npm run dev", "newTab": true },
//!     { "title": "Open Docs", "url": "https://docs.rs" },
//!     { "title": "Web Dev", "restart": "ignore", "workspace": {
//!       "cwd": "~/code/web",
//!       "layout": { "direction": "horizontal", "children": [
//!         { "pane": { "surfaces": [{ "command": "npm run dev" }] } },
//!         { "pane": { "surfaces": [{ "type": "browser", "url": "localhost:3000" }] } }
//!       ] }
//!     } }
//!   ]
//! }
//! ```
//!
//! `"shortcuts"` maps the action names in `keymap::ACTIONS` to new keys;
//! `null` or `[]` unbinds one. A project can add its own commands in
//! `.abromux/config.json` (see `PROJECT_FILES`); they ask before their first
//! run.

use crate::{
    layout::WorkspaceSpec,
    theme::{ACCENT, BORDER},
};
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
    /// What keeps terminals going across a quit (`crate::hold`).
    pub persistence: Persistence,
    pub app: AppSettings,
    pub sidebar: SidebarSettings,
    pub sidebar_appearance: SidebarAppearance,
    pub workspace_colors: WorkspaceColors,
    pub workspace_groups: WorkspaceGroups,
    pub automation: Automation,
    pub browser: BrowserSettings,
}

impl Global for Settings {}

/// How terminals come back after abromux quits. Applies to terminals opened
/// after it's set; ones already running in the holder are taken back
/// whatever it says.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Persistence {
    /// `"abromux"`: shells run in abromux's holder (`crate::hold`) and keep
    /// running; the next launch takes them back as they were.
    #[default]
    Abromux,
    /// `"cmux-like"`: shells end with the app; the next launch starts new
    /// ones in the same folders, replays their output and resumes coding
    /// agents, as cmux does.
    CmuxLike,
}

impl Persistence {
    /// Its value in the config file.
    pub fn key(self) -> &'static str {
        match self {
            Persistence::Abromux => "abromux",
            Persistence::CmuxLike => "cmux-like",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct BrowserSettings {
    /// What a browser tab is.
    pub engine: BrowserEngine,
}

/// The built-in WebKit browser, or a terminal running `terminal-browser`.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum BrowserEngine {
    #[default]
    BuiltIn,
    #[serde(alias = "terminal-browser")]
    TerminalBrowser,
}

/// cmux's `app` keys that abromux has.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct AppSettings {
    /// ⌘N starts in the focused terminal's directory; off, in home.
    pub workspace_inherit_working_directory: bool,
    /// Where a new workspace goes in the sidebar, outside a group.
    pub new_workspace_placement: Placement,
    /// Look for a new release once a day (`crate::update`).
    pub automatically_check_for_updates: bool,
    /// Where ⇧⌘I makes workspace folders (`~` allowed); None for the
    /// channel's own, `~/.abromux/ws`.
    pub workspace_folders_directory: Option<String>,
    /// ⌘W on a workspace's last tab opens a fresh terminal in its place
    /// instead of closing the workspace.
    pub keep_workspace_open_when_closing_last_surface: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            workspace_inherit_working_directory: true,
            new_workspace_placement: Placement::End,
            automatically_check_for_updates: true,
            workspace_folders_directory: None,
            keep_workspace_open_when_closing_last_surface: false,
        }
    }
}

/// Write `"persistence"` into the config file (the palette's "Persistence:"
/// commands). The rest of the file is left as written: the value is replaced
/// in place, or the key added first. A file that wouldn't parse with it
/// isn't touched.
pub fn set_persistence(p: Persistence) -> Result<(), String> {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let new = with_persistence(&text, p)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))
}

fn with_persistence(text: &str, p: Persistence) -> Result<String, String> {
    let value = format!("\"{}\"", p.key());
    let new = if text.trim().is_empty() {
        format!("{{\n  \"persistence\": {value}\n}}\n")
    } else if let Some(at) = top_level_value(text, "persistence") {
        format!("{}{value}{}", &text[..at.start], &text[at.end..])
    } else {
        let open = text
            .find('{')
            .ok_or("the config file isn't a JSON object")?;
        let (head, rest) = text.split_at(open + 1);
        let comma = if rest.trim_start().starts_with('}') {
            ""
        } else {
            ","
        };
        format!("{head}\n  \"persistence\": {value}{comma}{rest}")
    };
    match parse(&new) {
        Ok(s) if s.persistence == p => Ok(new),
        Ok(_) => Err("couldn't set persistence in the config file".into()),
        Err(e) => Err(format!("{}: {e}", path().display())),
    }
}

/// Where the string value of top-level `key` is in a JSON object's text.
fn top_level_value(text: &str, key: &str) -> Option<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            b'"' => {
                let end = string_end(bytes, i)?;
                if depth == 1 && &text[i + 1..end] == key {
                    let rest = text[end + 1..].trim_start();
                    let rest = rest.strip_prefix(':')?.trim_start();
                    let start = text.len() - rest.len();
                    if rest.starts_with('"') {
                        return Some(start..string_end(bytes, start)? + 1);
                    }
                    return None;
                }
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The closing quote of the JSON string that opens at `start`.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'"' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Where a new workspace is put: first, right after the selected one (or
/// its group), or last.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Placement {
    Top,
    #[default]
    AfterCurrent,
    End,
}

/// The sidebar's rows, with cmux's `sidebar` keys.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SidebarSettings {
    /// The workspace name's size in points; the smaller lines scale with it.
    #[serde(deserialize_with = "font_size")]
    pub font_size: f32,
    /// Only names: no description, notification, status, branch or path.
    pub hide_all_details: bool,
    /// Let long names wrap instead of cutting them off.
    pub wrap_workspace_titles: bool,
    pub show_workspace_description: bool,
    #[serde(deserialize_with = "opt_color")]
    pub workspace_description_color: Option<Rgba>,
    pub show_notification_message: bool,
    /// The git branch and directory line.
    pub show_branch_directory: bool,
    /// The branch and the directory on lines of their own.
    pub stack_branch_directory: bool,
    /// Only the directory's last part (`…/api`), not the whole `~/` path.
    pub path_last_segment_only: bool,
    pub show_progress: bool,
    pub show_log: bool,
    /// Status pills from `set-status` and agents.
    pub show_custom_metadata: bool,
    /// The selected workspace's row; `workspaceColors.selectionColor` too.
    #[serde(deserialize_with = "opt_color")]
    pub selection_color: Option<Rgba>,
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            font_size: 12.5,
            hide_all_details: false,
            wrap_workspace_titles: false,
            show_workspace_description: true,
            workspace_description_color: None,
            show_notification_message: true,
            show_branch_directory: true,
            stack_branch_directory: false,
            path_last_segment_only: false,
            show_progress: true,
            show_log: true,
            show_custom_metadata: true,
            selection_color: None,
        }
    }
}

/// A tint over the sidebar's background.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SidebarAppearance {
    #[serde(deserialize_with = "opt_color")]
    pub tint_color: Option<Rgba>,
    /// 0–1.
    #[serde(deserialize_with = "unit")]
    pub tint_opacity: f32,
}

impl Default for SidebarAppearance {
    fn default() -> Self {
        Self {
            tint_color: None,
            tint_opacity: 0.18,
        }
    }
}

/// How workspace colors show, and the named ones to pick from.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceColors {
    pub indicator_style: IndicatorStyle,
    #[serde(deserialize_with = "opt_color")]
    pub selection_color: Option<Rgba>,
    /// With `solidFill`, how strongly rows other than the selected one show
    /// their color.
    #[serde(deserialize_with = "unit")]
    pub unselected_fill_opacity: f32,
    /// The palette, by name; cmux's sixteen by default. Set, it replaces
    /// them all, as cmux's does.
    #[serde(deserialize_with = "named_colors")]
    pub colors: Vec<(String, u32)>,
}

/// The cmux palette, in its order.
pub const WORKSPACE_COLORS: [(&str, u32); 16] = [
    ("Red", 0xC0392B),
    ("Crimson", 0x922B21),
    ("Orange", 0xA04000),
    ("Amber", 0x7D6608),
    ("Olive", 0x4A5C18),
    ("Green", 0x196F3D),
    ("Teal", 0x006B6B),
    ("Aqua", 0x0E6B8C),
    ("Blue", 0x1565C0),
    ("Navy", 0x1A5276),
    ("Indigo", 0x283593),
    ("Purple", 0x6A1B9A),
    ("Magenta", 0xAD1457),
    ("Rose", 0x880E4F),
    ("Brown", 0x7B3F00),
    ("Charcoal", 0x3E4B5E),
];

impl Default for WorkspaceColors {
    fn default() -> Self {
        Self {
            indicator_style: IndicatorStyle::LeftRail,
            selection_color: None,
            unselected_fill_opacity: 0.7,
            colors: WORKSPACE_COLORS
                .iter()
                .map(|(n, c)| (n.to_string(), *c))
                .collect(),
        }
    }
}

impl WorkspaceColors {
    /// A palette name (any case) or `#rrggbb`.
    pub fn parse(&self, s: &str) -> Option<u32> {
        self.colors
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(s))
            .map(|(_, c)| *c)
            .or_else(|| {
                s.starts_with('#')
                    .then(|| crate::workspace::parse_color(s))
                    .flatten()
            })
    }
}

/// How a workspace's color marks its row.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum IndicatorStyle {
    /// A bar down the row's left edge.
    #[default]
    LeftRail,
    /// The whole row, faded unless it's selected.
    SolidFill,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceGroups {
    /// Where a new workspace goes inside a group (⌘N from one of its
    /// workspaces, or its header's +): after the selected one, second
    /// (after the group's first), or last.
    pub new_workspace_placement: Placement,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Automation {
    /// Name a workspace from its agent's first prompt, summarised by that
    /// agent's own CLI (`claude -p`, `codex exec`). A name you give it wins.
    pub workspace_auto_naming: bool,
    /// The agent CLI that cleans up a workspace folder (right-click › Clean
    /// Up and Delete Folder…), run with the instruction as its one argument.
    /// None is `claude`.
    pub workspace_cleanup_agent: Option<String>,
}

/// The keys bound to an action or command.
#[derive(Clone, Debug, PartialEq)]
pub struct Shortcut {
    /// Each one a keystroke, or a chord of several.
    pub keys: Vec<Vec<Keystroke>>,
    /// The key context it's bound in, like `Terminal`; by default the
    /// action's own contexts (app-wide for a command).
    pub when: Option<String>,
}

/// A palette command from the config file or a project's. It types `run`
/// into the focused terminal and presses return, opens `url` in a new
/// browser tab, or builds `workspace` (`crate::layout`). cmux's `name` and
/// `command` work for `title` and `run`, so a `cmux.json` reads as is.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Command {
    #[serde(alias = "name")]
    pub title: String,
    /// Shown under the title in the palette.
    #[serde(default)]
    pub description: Option<String>,
    /// More words the palette finds it by.
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default, alias = "command")]
    pub run: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub workspace: Option<WorkspaceSpec>,
    /// What a workspace command does when a workspace of its name is open.
    #[serde(default)]
    pub restart: Restart,
    /// Ask before every run.
    #[serde(default)]
    pub confirm: bool,
    /// Run it in a new terminal tab instead of the focused terminal.
    #[serde(default)]
    pub new_tab: bool,
    #[serde(default, deserialize_with = "shortcut")]
    pub shortcut: Option<Shortcut>,
}

/// cmux's `restart`: what to do when the command's workspace is open.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Restart {
    /// Open another one.
    #[default]
    New,
    /// Switch to it.
    Ignore,
    /// Close it and build it again.
    Recreate,
    /// Ask, then recreate.
    Confirm,
}

impl Command {
    /// The workspace name a workspace command opens.
    pub fn workspace_name(&self) -> Option<&str> {
        let w = self.workspace.as_ref()?;
        Some(w.name.as_deref().unwrap_or(&self.title))
    }

    /// What running it does, for a confirmation.
    pub fn describe(&self) -> String {
        if let Some(run) = &self.run {
            return format!("Types into a terminal:\n{run}");
        }
        if let Some(url) = &self.url {
            return format!("Opens {url}");
        }
        let Some(w) = &self.workspace else {
            return String::new();
        };
        let mut lines = vec![format!(
            "Opens the workspace “{}”.",
            self.workspace_name().unwrap_or_default()
        )];
        let runs = w.commands();
        if !runs.is_empty() {
            lines.push(format!("Runs:\n{}", runs.join("\n")));
        }
        lines.join("\n")
    }

    fn check(&self) -> Result<(), String> {
        let kinds = [
            self.run.is_some(),
            self.url.is_some(),
            self.workspace.is_some(),
        ];
        if kinds.iter().filter(|k| **k).count() != 1 {
            return Err(format!(
                "command {:?} needs one of \"run\", \"url\" or \"workspace\"",
                self.title
            ));
        }
        if let Some(w) = &self.workspace {
            w.check()
                .map_err(|e| format!("command {:?}: {e}", self.title))?;
        }
        Ok(())
    }
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

fn opt_color<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Rgba>, D::Error> {
    #[derive(Deserialize)]
    struct C(#[serde(deserialize_with = "color")] Rgba);
    Ok(Option::<C>::deserialize(d)?.map(|c| c.0))
}

fn font_size<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = f32::deserialize(d)?;
    if (8. ..=24.).contains(&v) {
        Ok(v)
    } else {
        Err(serde::de::Error::custom(format!(
            "fontSize must be 8–24 points, got {v}"
        )))
    }
}

fn unit<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = f32::deserialize(d)?;
    if (0. ..=1.).contains(&v) {
        Ok(v)
    } else {
        Err(serde::de::Error::custom(format!(
            "an opacity is 0–1, got {v}"
        )))
    }
}

/// `{ "Red": "#C0392B", … }`, sorted by name.
fn named_colors<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<(String, u32)>, D::Error> {
    let map = serde_json::Map::<String, serde_json::Value>::deserialize(d)?;
    let mut colors = map
        .into_iter()
        .map(|(name, v)| {
            v.as_str()
                .and_then(crate::workspace::parse_color)
                .map(|c| (name.clone(), c))
                .ok_or_else(|| serde::de::Error::custom(format!("color {name:?} isn't #rrggbb")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    colors.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(colors)
}

/// Whether browser tabs open as `terminal-browser`.
pub fn terminal_browser(cx: &gpui::App) -> bool {
    cx.try_global::<Settings>()
        .is_some_and(|s| s.browser.engine == BrowserEngine::TerminalBrowser)
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

/// Set one key of the config file, as the Settings window does: `key` is
/// its path (`["sidebar", "fontSize"]`), and `None` removes it so the
/// default applies. The rest of the file stays as it was, in its order;
/// objects left empty are removed. The result must parse, or nothing is
/// written. Returns the new settings.
pub fn edit(key: &[&str], value: Option<serde_json::Value>) -> Result<Settings, String> {
    edit_file(&path(), key, value)
}

pub fn edit_file(
    path: &Path,
    key: &[&str],
    value: Option<serde_json::Value>,
) -> Result<Settings, String> {
    use serde_json::{Map, Value};
    let at = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(at(&e)),
    };
    let mut root = if text.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&text).map_err(|e| at(&e))?
    };
    let Some((last, parents)) = key.split_last() else {
        return Err("no key to set".into());
    };
    fn set(obj: &mut Map<String, Value>, parents: &[&str], last: &str, value: Option<Value>) {
        let Some((first, rest)) = parents.split_first() else {
            match value {
                Some(v) => {
                    obj.insert(last.to_string(), v);
                }
                None => {
                    obj.shift_remove(last);
                }
            }
            return;
        };
        if value.is_none() && !obj.get(*first).is_some_and(Value::is_object) {
            return;
        }
        let child = obj
            .entry(first.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !child.is_object() {
            *child = Value::Object(Map::new());
        }
        let Value::Object(child) = child else {
            unreachable!()
        };
        set(child, rest, last, value);
        if child.is_empty() {
            obj.shift_remove(*first);
        }
    }
    let Value::Object(obj) = &mut root else {
        return Err(at(&"the file isn't a JSON object"));
    };
    set(obj, parents, last, value);
    let mut out = serde_json::to_string_pretty(&root).map_err(|e| at(&e))?;
    out.push('\n');
    let settings = parse(&out).map_err(|e| at(&e))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| at(&e))?;
    }
    // Write next to it and rename, so a crash never leaves half a file. A
    // symlinked file (dotfiles) is written where it points, keeping the link.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let tmp = target.with_extension("json.tmp");
    std::fs::write(&tmp, out)
        .and_then(|()| std::fs::rename(&tmp, &target))
        .map_err(|e| at(&e))?;
    Ok(settings)
}

fn parse(text: &str) -> Result<Settings, serde_json::Error> {
    // An empty file is a valid "no settings".
    if text.trim().is_empty() {
        return Ok(Settings::default());
    }
    let s: Settings = serde_json::from_str(text)?;
    for c in &s.commands {
        c.check().map_err(serde::de::Error::custom)?;
    }
    Ok(s)
}

/// A project's own commands file, looked for from a directory upward:
/// `.abromux/config.json`, or cmux's `.cmux/cmux.json` or `cmux.json`.
/// Only `"commands"` is read from it; other keys (cmux's `actions`, `ui`)
/// are ignored.
pub const PROJECT_FILES: [&str; 3] = [".abromux/config.json", ".cmux/cmux.json", "cmux.json"];

#[derive(Deserialize)]
struct ProjectFile {
    #[serde(default)]
    commands: Vec<serde_json::Value>,
}

/// The nearest project file at or above `dir`.
pub fn find_project(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .flat_map(|d| PROJECT_FILES.iter().map(move |f| d.join(f)))
        .find(|p| p.is_file())
}

/// The directory a project file belongs to: the one holding `.abromux/`,
/// `.cmux/` or `cmux.json`.
pub fn project_root(file: &Path) -> &Path {
    let dir = file.parent().unwrap_or(file);
    match dir.file_name().and_then(|n| n.to_str()) {
        Some(".abromux" | ".cmux") => dir.parent().unwrap_or(dir),
        _ => dir,
    }
}

/// A project's commands, each with its JSON as written (what trusting it
/// covers: a changed command asks again).
pub fn read_project(path: &Path) -> Result<Vec<(Command, String)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let at = |e: serde_json::Error| format!("{}: {e}", path.display());
    let file: ProjectFile = serde_json::from_str(&text).map_err(at)?;
    file.commands
        .into_iter()
        .map(|raw| {
            let c: Command = serde_json::from_value(raw.clone()).map_err(at)?;
            c.check().map_err(|e| format!("{}: {e}", path.display()))?;
            Ok((c, raw.to_string()))
        })
        .collect()
}

/// Project commands the user has said may run, as `path\njson` lines, in
/// the app's support folder.
fn trust_path() -> PathBuf {
    crate::channel::support_dir().join("trusted-commands.json")
}

fn trust_key(file: &Path, raw: &str) -> String {
    format!("{}\n{raw}", file.display())
}

fn trusted_keys() -> Vec<String> {
    std::fs::read_to_string(trust_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Whether the project command `raw` from `file` was trusted as it is now.
pub fn is_trusted(file: &Path, raw: &str) -> bool {
    trusted_keys().contains(&trust_key(file, raw))
}

/// Remember that the project command `raw` from `file` may run.
pub fn trust(file: &Path, raw: &str) {
    let mut keys = trusted_keys();
    let key = trust_key(file, raw);
    if keys.contains(&key) {
        return;
    }
    keys.push(key);
    let path = trust_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(&keys) {
        let _ = std::fs::write(path, json);
    }
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
    fn persistence() {
        assert_eq!(Settings::default().persistence, Persistence::Abromux);
        let s = parse(r#"{"persistence": "abromux"}"#).unwrap();
        assert_eq!(s.persistence, Persistence::Abromux);
        let s = parse(r#"{"persistence": "cmux-like"}"#).unwrap();
        assert_eq!(s.persistence, Persistence::CmuxLike);
        for gone in ["abromux-hold", "herdr-core"] {
            assert!(parse(&format!(r#"{{"persistence": "{gone}"}}"#)).is_err());
        }
    }

    #[test]
    fn setting_persistence_keeps_the_rest_of_the_file() {
        let old = Persistence::CmuxLike;
        assert_eq!(
            with_persistence("", old).unwrap(),
            "{\n  \"persistence\": \"cmux-like\"\n}\n"
        );
        assert_eq!(
            with_persistence("{}", old).unwrap(),
            "{\n  \"persistence\": \"cmux-like\"}"
        );
        // Added first; key order, spacing and nested strings stay.
        let file = "{\n    \"panes\": { \"resizeStep\": 30 },\n    \"commands\": [{ \"title\": \"persistence\", \"run\": \"echo \\\"persistence\\\"\" }]\n}\n";
        let new = with_persistence(file, old).unwrap();
        assert_eq!(
            new,
            format!("{{\n  \"persistence\": \"cmux-like\",{}", &file[1..])
        );
        // Replaced in place, and back again.
        let back = with_persistence(&new, Persistence::Abromux).unwrap();
        assert_eq!(back, new.replace("\"cmux-like\"", "\"abromux\""));
        assert_eq!(parse(&back).unwrap().panes.resize_step, 30.);
        // A broken file is left alone.
        assert!(with_persistence("{ \"panes\": ", old).is_err());
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
        let e = parse(r#"{"commands": [{ "title": "x", "run": "a", "workspace": {} }]}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("needs one of"), "{e}");
    }

    #[test]
    fn cmux_commands_and_project_files() {
        let dir = std::env::temp_dir().join(format!("abromux-project-{}", std::process::id()));
        let sub = dir.join("app/src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(dir.join(".cmux")).unwrap();
        let file = dir.join(".cmux/cmux.json");
        std::fs::write(
            &file,
            r#"{
              "actions": {},
              "commands": [
                { "name": "Build", "command": "make", "keywords": ["compile"], "confirm": true },
                { "name": "Web", "restart": "recreate", "workspace": { "layout": { "pane": { "surfaces": [{}] } } } }
              ]
            }"#,
        )
        .unwrap();
        assert_eq!(find_project(&sub).as_deref(), Some(file.as_path()));
        assert_eq!(project_root(&file), dir);
        let cmds = read_project(&file).unwrap();
        assert_eq!(cmds[0].0.run.as_deref(), Some("make"));
        assert!(cmds[0].0.confirm && cmds[0].1.contains("\"make\""));
        assert_eq!(cmds[1].0.restart, Restart::Recreate);
        assert_eq!(cmds[1].0.workspace_name(), Some("Web"));
        // `.abromux/config.json` is closer, so it wins.
        std::fs::create_dir_all(sub.join(".abromux")).unwrap();
        std::fs::write(
            sub.join(".abromux/config.json"),
            r#"{"commands": [{"name": "x"}]}"#,
        )
        .unwrap();
        assert!(
            read_project(&find_project(&sub).unwrap())
                .unwrap_err()
                .contains("needs one of")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sidebar_and_workspace_keys() {
        let s = parse(
            r##"{
              "app": { "workspaceInheritWorkingDirectory": false, "newWorkspacePlacement": "afterCurrent",
                       "keepWorkspaceOpenWhenClosingLastSurface": true },
              "sidebar": { "fontSize": 14, "hideAllDetails": true, "selectionColor": "#336699" },
              "sidebarAppearance": { "tintColor": "#000", "tintOpacity": 0.3 },
              "workspaceColors": { "indicatorStyle": "solidFill", "colors": { "Ocean": "#0066aa" } },
              "workspaceGroups": { "newWorkspacePlacement": "top" },
              "automation": { "workspaceAutoNaming": true }
            }"##,
        )
        .unwrap();
        assert!(!s.app.workspace_inherit_working_directory);
        assert!(s.app.keep_workspace_open_when_closing_last_surface);
        assert!(!AppSettings::default().keep_workspace_open_when_closing_last_surface);
        assert_eq!(s.app.new_workspace_placement, Placement::AfterCurrent);
        assert_eq!(s.sidebar.font_size, 14.);
        assert!(s.sidebar.hide_all_details && s.sidebar.show_log);
        assert_eq!(s.sidebar.selection_color, Some(rgb(0x336699)));
        assert_eq!(s.sidebar_appearance.tint_opacity, 0.3);
        assert_eq!(
            s.workspace_colors.indicator_style,
            IndicatorStyle::SolidFill
        );
        assert_eq!(s.workspace_colors.parse("ocean"), Some(0x0066aa));
        assert_eq!(
            s.workspace_colors.parse("Red"),
            None,
            "a palette replaces cmux's"
        );
        assert_eq!(s.workspace_colors.parse("#ff0000"), Some(0xff0000));
        assert_eq!(s.workspace_groups.new_workspace_placement, Placement::Top);
        assert!(s.automation.workspace_auto_naming);
        let d = Settings::default();
        assert_eq!(d.workspace_colors.parse("amber"), Some(0x7D6608));
        assert_eq!(d.app.new_workspace_placement, Placement::End);
        let err = |text: &str| parse(text).unwrap_err().to_string();
        assert!(err(r#"{"sidebar": {"fontSize": 50}}"#).contains("8–24"));
        assert!(
            err(r#"{"workspaceColors": {"indicatorStyle": "wash"}}"#).contains("unknown variant")
        );
        assert!(err(r#"{"workspaceColors": {"colors": {"X": "blue"}}}"#).contains("isn't #rrggbb"));
    }

    #[test]
    fn browser_engine() {
        assert_eq!(Settings::default().browser.engine, BrowserEngine::BuiltIn);
        for v in ["terminalBrowser", "terminal-browser"] {
            let s = parse(&format!(r#"{{"browser": {{"engine": "{v}"}}}}"#)).unwrap();
            assert_eq!(s.browser.engine, BrowserEngine::TerminalBrowser);
        }
        let s = parse(r#"{"browser": {"engine": "builtIn"}}"#).unwrap();
        assert_eq!(s.browser.engine, BrowserEngine::BuiltIn);
        let e = parse(r#"{"browser": {"engine": "chrome"}}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown variant"), "{e}");
    }

    #[test]
    fn missing_file_is_defaults() {
        let p = Path::new("/nonexistent/abromux/config.json");
        assert_eq!(read(p).unwrap(), Settings::default());
    }

    #[test]
    fn edit_keeps_the_rest_of_the_file() {
        use serde_json::json;
        let dir = std::env::temp_dir().join(format!("abromux-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("sub/config.json");

        // A missing file and folder are created.
        let s = edit_file(&p, &["sidebar", "fontSize"], Some(json!(14))).unwrap();
        assert_eq!(s.sidebar.font_size, 14.);

        std::fs::write(
            &p,
            r#"{ "panes": { "resizeStep": 30 }, "commands": [{ "title": "t", "run": "ls" }],
                "sidebar": { "showLog": false, "fontSize": 14 } }"#,
        )
        .unwrap();
        let s = edit_file(&p, &["app", "newWorkspacePlacement"], Some(json!("top"))).unwrap();
        assert_eq!(s.app.new_workspace_placement, Placement::Top);
        assert_eq!(s.panes.resize_step, 30.);
        assert_eq!(s.commands.len(), 1);
        let keys = |p: &Path| -> Vec<String> {
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
            v.as_object().unwrap().keys().cloned().collect()
        };
        assert_eq!(
            keys(&p),
            ["panes", "commands", "sidebar", "app"],
            "order kept"
        );

        // Removing the last key of an object removes the object.
        edit_file(&p, &["sidebar", "showLog"], None).unwrap();
        edit_file(&p, &["sidebar", "fontSize"], None).unwrap();
        assert_eq!(keys(&p), ["panes", "commands", "app"]);
        // Removing what isn't there is fine.
        edit_file(&p, &["automation", "workspaceAutoNaming"], None).unwrap();

        // A value the settings reject isn't written.
        let before = std::fs::read_to_string(&p).unwrap();
        let e = edit_file(&p, &["sidebar", "fontSize"], Some(json!(99))).unwrap_err();
        assert!(e.contains("fontSize must be 8–24"), "{e}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);

        // Nor is anything over a file that isn't JSON.
        std::fs::write(&p, "{ oops").unwrap();
        assert!(edit_file(&p, &["sidebar", "showLog"], Some(json!(true))).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ oops");

        // A symlinked file stays a symlink.
        let real = dir.join("real.json");
        std::fs::write(&real, "{}").unwrap();
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        edit_file(&link, &["sidebar", "showLog"], Some(json!(false))).unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(std::fs::read_to_string(&real).unwrap().contains("showLog"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
