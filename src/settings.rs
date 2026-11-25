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
//!   "browser": { "engine": "terminalBrowser", "terminalBrowserPanes": true },
//!   "editor": { "program": "tode" },
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
    theme::{Appearance, pal},
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
    pub editor: EditorSettings,
    pub notifications: NotificationSettings,
    pub gitlab: GitlabSettings,
    pub jira: JiraSettings,
    pub claude: ClaudeSettings,
    pub remote: RemoteSettings,
    /// Values of the settings init.lua declares (`abromux.setting`), by key.
    pub scripts: BTreeMap<String, serde_json::Value>,
    /// Those settings, from init.lua: not in the file.
    #[serde(skip)]
    pub script_settings: Vec<crate::lua::SettingDef>,
}

impl Settings {
    /// Script setting `key`'s value: the file's, if it's one the setting
    /// takes, else its default. None for a key init.lua doesn't declare.
    pub fn script_value(&self, key: &str) -> Option<serde_json::Value> {
        let def = self.script_settings.iter().find(|d| d.key == key)?;
        Some(
            self.scripts
                .get(key)
                .filter(|v| def.accepts(v))
                .cloned()
                .unwrap_or_else(|| def.default.clone()),
        )
    }
}

impl Global for Settings {}

/// Claude tabs (`crate::claude_chat`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ClaudeSettings {
    /// The Claude Code config folder Claude tabs use (its account, settings
    /// and transcripts), as `CLAUDE_CONFIG_DIR`. None: what the login shell
    /// sets, else `~/.claude`.
    pub config_dir: Option<String>,
    /// The permission mode a Claude tab starts in when none is asked for
    /// (`ask`, `accept-edits`, `plan`, `auto`, `dont-ask`). None: auto.
    pub default_mode: Option<String>,
}

/// The config folder Claude tabs use, if the config file names one.
pub fn claude_config_dir(cx: &gpui::App) -> Option<PathBuf> {
    let dir = cx
        .try_global::<Settings>()?
        .claude
        .config_dir
        .as_deref()?
        .trim();
    (!dir.is_empty()).then(|| abromux_core::paths::expand_tilde(dir))
}

/// The permission mode a new Claude tab starts in: the config file's
/// `defaultMode`, else auto. Never bypass: that one you pick per tab.
pub fn claude_default_mode(cx: &gpui::App) -> &'static str {
    cx.try_global::<Settings>()
        .and_then(|s| s.claude.default_mode.as_deref())
        .and_then(crate::claude_chat::mode_name)
        .filter(|m| *m != "bypassPermissions")
        .unwrap_or("auto")
}

/// Phone access over Tailscale (`crate::remote`, `abromux remote`,
/// Settings › Phone Access). The release and nightly apps share it, each on
/// its own ports.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct RemoteSettings {
    /// Serve the phone app on this Mac's tailnet address.
    pub enabled: bool,
    /// Tailscale logins let in besides this Mac's own user, who always is.
    pub allow: Vec<String>,
    /// Let phones call every method, browser automation and files too.
    pub all_methods: bool,
    /// Keep the Mac awake (not the display) while an agent is working.
    pub keep_awake: bool,
    /// Notify phones only after this long without keyboard or mouse use on
    /// the Mac (seconds); 0: always.
    pub push_when_idle: u64,
    /// Show a notification's text on the phone, not just its title.
    pub push_preview: bool,
    /// The loopback port the app listens on. None: 7298 (the nightly 7299).
    pub port: Option<u16>,
    /// The tailnet HTTPS port `tailscale serve` uses: 443, 8443 or 10000.
    /// None: 8443 (the nightly 10000).
    pub https_port: Option<u16>,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            allow: Vec::new(),
            all_methods: false,
            keep_awake: true,
            push_when_idle: 60,
            push_preview: true,
            port: None,
            https_port: None,
        }
    }
}

/// Jira, for tracked links (`crate::links`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct JiraSettings {
    /// The Jira a bare ticket key (`abromux link add PAY-1423`) is on, e.g.
    /// `acme.atlassian.net`. None: the one a tracked ticket is on.
    pub host: Option<String>,
}

/// The GitLab merge requests panel and cache (`crate::gitlab`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct GitlabSettings {
    /// GitLab in abromux at all: its tabs for merge request links, the
    /// merge requests panel and its background cache, and the workspaces'
    /// merge requests in the sidebar. Off: merge request links are web
    /// pages like any other, and nothing asks GitLab anything. Local diffs
    /// (`abromux diff`) work either way.
    pub enabled: bool,
    /// The GitLab the panel and cache use, e.g. `gitlab.example.com`. None:
    /// the one signed in to in a GitLab tab. Its token is the tab's (the
    /// Keychain). Workspace checkouts whose remote is elsewhere are left out.
    pub host: Option<String>,
    /// How often merge requests are checked in the background
    /// (`crate::gitlab`'s intervals, scaled together). Off: nothing is (tabs load what they open, and
    /// Refresh still checks once).
    pub polling: GitlabPolling,
    /// "Mine" also holds merge requests you're asked to review.
    pub include_review_requests: bool,
    /// "Mine" also holds merge requests assigned to you.
    pub include_assigned: bool,
    /// Merge requests you opened that stay cached, newest first.
    pub recent_limit: u32,
    /// Disk space for cached merge requests, in MB. Past it, the
    /// longest-unopened ones' go first; they stay listed.
    pub cache_size_mb: u32,
    /// A file whose diff is bigger (KB) is fetched when it's shown.
    pub max_file_diff_kb: u32,
    /// Past this much of a merge request's diff (MB), files are fetched
    /// when they're shown.
    pub max_mr_diff_mb: u32,
    /// …or after this many files.
    pub max_mr_diff_files: u32,
}

impl Default for GitlabSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            host: None,
            polling: GitlabPolling::default(),
            include_review_requests: true,
            include_assigned: false,
            recent_limit: 50,
            cache_size_mb: 500,
            max_file_diff_kb: 1024,
            max_mr_diff_mb: 20,
            max_mr_diff_files: 3000,
        }
    }
}

/// `gitlab.polling`: how often the background checks run.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum GitlabPolling {
    /// Nothing in the background.
    Off,
    /// Every interval four times longer.
    Low,
    /// The intervals as they are.
    #[default]
    Medium,
    /// Every interval halved.
    High,
}

impl GitlabPolling {
    /// Its value in the config file.
    pub fn key(self) -> &'static str {
        match self {
            GitlabPolling::Off => "off",
            GitlabPolling::Low => "low",
            GitlabPolling::Medium => "medium",
            GitlabPolling::High => "high",
        }
    }

    /// What every interval is multiplied by; None when off.
    pub fn factor(self) -> Option<f64> {
        match self {
            GitlabPolling::Off => None,
            GitlabPolling::Low => Some(4.),
            GitlabPolling::Medium => Some(1.),
            GitlabPolling::High => Some(0.5),
        }
    }
}

pub use abromux_core::pending::TurnComplete;
pub use abromux_core::settings::{NotificationHook, Persistence};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct BrowserSettings {
    /// What a browser tab is; None for this OS's default ([`browser_engine`]).
    pub engine: Option<BrowserEngine>,
    /// terminal-browser opens new tabs as abromux splits beside it
    /// (`crate::terminal_browser_panes`).
    pub terminal_browser_panes: bool,
}

/// The built-in WebKit browser (macOS only), a terminal running
/// `terminal-browser`, or the system's default browser (no tab: pages open
/// there).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum BrowserEngine {
    BuiltIn,
    #[serde(alias = "terminal-browser")]
    TerminalBrowser,
    System,
}

/// Files abromux is asked to open (`crate::editor`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct EditorSettings {
    /// The terminal editor a file opens in, in a pane of its own.
    pub program: EditorProgram,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum EditorProgram {
    #[default]
    Nvim,
    Tode,
}

impl EditorProgram {
    /// Its value in the config file, which is also the command's name.
    pub fn key(self) -> &'static str {
        match self {
            EditorProgram::Nvim => "nvim",
            EditorProgram::Tode => "tode",
        }
    }
}

/// cmux's `app` keys that abromux has.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct AppSettings {
    /// Light, dark, or following macOS (`crate::theme`).
    pub appearance: Appearance,
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
    /// Move a workspace to the top when it gets a notification you aren't
    /// looking at: cmux's `true`/`"notifications"`, or `false`/`"off"`.
    /// cmux's `"agentActivity"` is taken as `"notifications"`.
    #[serde(deserialize_with = "reorder")]
    pub reorder_on_notification: bool,
}

fn reorder<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        Flag(bool),
        Mode(String),
    }
    match V::deserialize(d)? {
        V::Flag(b) => Ok(b),
        V::Mode(m) => match m.as_str() {
            "off" => Ok(false),
            "notifications" | "agentActivity" => Ok(true),
            other => Err(serde::de::Error::custom(format!(
                "reorderOnNotification: expected true, false, \"off\", \"notifications\" or \"agentActivity\", got \"{other}\""
            ))),
        },
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            appearance: Appearance::System,
            workspace_inherit_working_directory: true,
            new_workspace_placement: Placement::End,
            automatically_check_for_updates: true,
            workspace_folders_directory: None,
            keep_workspace_open_when_closing_last_surface: false,
            reorder_on_notification: true,
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
    /// The selected workspace's tracked links under its row (`crate::links`).
    pub show_links: bool,
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
            show_links: true,
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
    /// Sections by agent status, from 0.28: removed, but still accepted so
    /// a config that set it isn't refused.
    #[serde(rename = "groupByAgentStatus")]
    _group_by_agent_status: serde::de::IgnoredAny,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Automation {
    /// Name a workspace from its agent's prompts, summarised by that agent's
    /// own CLI (`claude -p`, `codex exec`), renaming it as the work moves on.
    /// A name you give it wins and stops that. On by default. (A ⇧⌘I
    /// workspace left on its made-up name is named this way even when off.)
    pub workspace_auto_naming: bool,
    /// The agent CLI that cleans up a workspace folder (right-click › Clean
    /// Up and Delete Folder…), run with the instruction as its one argument.
    /// None is `claude`.
    pub workspace_cleanup_agent: Option<String>,
}

impl Default for Automation {
    fn default() -> Self {
        Self {
            workspace_auto_naming: true,
            workspace_cleanup_agent: None,
        }
    }
}

/// cmux's `notifications` keys.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct NotificationSettings {
    /// Commands each notification passes through, in order
    /// (`crate::notify_hook`).
    pub hooks: Vec<NotificationHook>,
    /// The bell in the macOS menu bar, or the tray on Linux (off by default
    /// there).
    pub show_in_menu_bar: bool,
    /// An agent (or a program, OSC 7501) waiting for you: a permission
    /// prompt or a question.
    pub needs_input: Alert,
    /// An agent finishing its turn, or a program done.
    pub done: Alert,
    /// A program that failed (OSC 7501, OSC 9;4).
    pub error: Alert,
    /// When a coding agent finishing a turn notifies
    /// (`abromux_core::pending`).
    pub agent_turn_complete: TurnComplete,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        NotificationSettings {
            hooks: vec![],
            // The tray on Linux: off, as not every bar has one.
            show_in_menu_bar: abromux_platform::MACOS,
            needs_input: Alert::Banner,
            // An agent finishes every turn: worth a dot, not a banner each.
            done: Alert::Unread,
            error: Alert::Banner,
            agent_turn_complete: TurnComplete::WhenIdle,
        }
    }
}

impl NotificationSettings {
    pub fn alert(&self, kind: crate::notification::Kind) -> Alert {
        use crate::notification::Kind;
        match kind {
            Kind::NeedsInput => self.needs_input,
            Kind::Done => self.done,
            Kind::Error => self.error,
            Kind::Message | Kind::Bell => Alert::Banner,
        }
    }
}

/// How much a kind of notification asks for.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Alert {
    /// Unread, and a desktop banner while the app is in the background.
    Banner,
    /// Unread (the pane's ring, the dots, the sidebar line), no banner.
    Unread,
    /// Not even recorded.
    Off,
}

impl Alert {
    pub fn as_str(self) -> &'static str {
        match self {
            Alert::Banner => "banner",
            Alert::Unread => "unread",
            Alert::Off => "off",
        }
    }
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
    /// A Lua script to run with `abromux do` (`~/…`, absolute, or relative
    /// to the config file's folder, or the project's).
    #[serde(default)]
    pub script: Option<String>,
    /// With `script`: run this action of it (`abromux do <script> --action`).
    #[serde(default)]
    pub action: Option<String>,
    /// With `script`: its arguments (`abromux.args`).
    #[serde(default)]
    pub args: Option<serde_json::Map<String, serde_json::Value>>,
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
        if let Some(script) = &self.script {
            return match &self.action {
                Some(a) => format!("Runs the Lua action “{a}” of {script}"),
                None => format!("Runs the Lua script {script}"),
            };
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
            self.script.is_some(),
        ];
        if kinds.iter().filter(|k| **k).count() != 1 {
            return Err(format!(
                "command {:?} needs one of \"run\", \"url\", \"workspace\" or \"script\"",
                self.title
            ));
        }
        if self.script.is_none() && (self.action.is_some() || self.args.is_some()) {
            return Err(format!(
                "command {:?}: \"action\" and \"args\" go with \"script\"",
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
    /// The line between split panes; None for the theme's border colour
    /// ([`Panes::divider`]).
    #[serde(deserialize_with = "opt_color")]
    pub divider_color: Option<Rgba>,
    /// The border around the focused pane when a workspace has splits.
    #[serde(deserialize_with = "color")]
    pub focus_border_color: Rgba,
    /// Where a terminal's ⌘-clicked link (a web page or Markdown file)
    /// opens.
    pub open_links_in: OpenLinksIn,
}

/// `panes.openLinksIn`.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum OpenLinksIn {
    /// A pane to the right of the terminal (a new tab there if it already
    /// shows that kind of tab).
    NewPane,
    /// A new tab in the terminal's own pane.
    #[default]
    SamePane,
}

impl OpenLinksIn {
    /// Its value in the config file.
    pub fn key(self) -> &'static str {
        match self {
            OpenLinksIn::NewPane => "newPane",
            OpenLinksIn::SamePane => "samePane",
        }
    }
}

impl Panes {
    /// The line between split panes, in the current theme unless set.
    pub fn divider(&self) -> Rgba {
        self.divider_color.unwrap_or_else(|| rgb(pal().border))
    }
}

impl Default for Panes {
    fn default() -> Self {
        Self {
            resize_step: 20.,
            divider_color: None,
            focus_border_color: rgba(pal().accent << 8 | 0xb0),
            open_links_in: OpenLinksIn::SamePane,
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
    abromux_plugin::color::parse(&s).ok_or_else(|| {
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

/// What browser tabs are: the config file's `browser.engine`, else this
/// OS's default, the built-in browser on macOS, and on Linux (which has
/// none) terminal-browser if it's installed, else the default browser.
pub fn browser_engine(cx: &gpui::App) -> BrowserEngine {
    let set = cx.try_global::<Settings>().and_then(|s| s.browser.engine);
    set.unwrap_or_else(default_engine)
}

/// This OS's browser engine when the config file names none.
pub fn default_engine() -> BrowserEngine {
    if abromux_platform::MACOS {
        BrowserEngine::BuiltIn
    } else if crate::terminal_browser::installed() {
        BrowserEngine::TerminalBrowser
    } else {
        BrowserEngine::System
    }
}

/// Whether browser tabs open as `terminal-browser`.
pub fn terminal_browser(cx: &gpui::App) -> bool {
    browser_engine(cx) == BrowserEngine::TerminalBrowser
}

/// Whether pages open in the system's default browser instead of a tab.
pub fn system_browser(cx: &gpui::App) -> bool {
    browser_engine(cx) == BrowserEngine::System
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
    crate::paths::xdg_config().join("abromux/config.json")
}

/// The config file's path, after making an empty one if there's none, so
/// it can be opened in an editor.
pub fn ensure_file() -> PathBuf {
    let path = path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, "{\n}\n");
    }
    path
}

/// The settings in the config file, or the defaults if there is none.
pub fn load() -> Result<Settings, String> {
    read(&path())
}

/// `load`, plus the actions init.lua (next to the file) defines, as
/// commands after the file's own: what the app uses.
pub fn load_app() -> Result<Settings, String> {
    let mut s = load()?;
    let init = crate::lua::init_path();
    let declared = crate::lua::collect_all(&init)?;
    s.commands.extend(commands_of(&init, declared.actions)?);
    s.script_settings = declared.settings;
    Ok(s)
}

/// `s` with init.lua's commands and settings as its last load had them,
/// none if it failed: for a change made in the app (the Settings window,
/// `settings.set`), which shouldn't fail over init.lua.
pub fn with_lua(mut s: Settings) -> Settings {
    let init = crate::lua::init_path();
    if let Ok(d) = crate::lua::collect_all_cached(&init) {
        s.commands
            .extend(commands_of(&init, d.actions).unwrap_or_default());
        s.script_settings = d.settings;
    }
    s
}

/// What's wrong with the config file or init.lua, if anything, for the
/// Settings window. It asks often (each change, each time it comes to the
/// front), so init.lua only runs again once it, or a file it `require`s,
/// has changed.
pub fn app_error() -> Option<String> {
    let init = crate::lua::init_path();
    load()
        .and_then(|_| commands_of(&init, crate::lua::collect_cached(&init)?))
        .err()
}

/// The actions of `init`, each a palette command that runs it.
#[cfg(test)]
pub fn lua_commands(init: &Path) -> Result<Vec<Command>, String> {
    commands_of(init, crate::lua::collect(init)?)
}

fn commands_of(init: &Path, actions: Vec<crate::lua::ActionDef>) -> Result<Vec<Command>, String> {
    let at = |e: serde_json::Error| format!("{}: {e}", init.display());
    actions
        .into_iter()
        .filter(|a| a.palette)
        .map(|a| {
            serde_json::from_value::<Command>(serde_json::json!({
                "title": a.title,
                "description": a.description,
                "keywords": a.keywords,
                "script": init.to_string_lossy(),
                "action": a.name,
                "shortcut": a.key,
            }))
            .map_err(at)
        })
        .collect()
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
    if !abromux_platform::MACOS && s.browser.engine == Some(BrowserEngine::BuiltIn) {
        return Err(serde::de::Error::custom(
            "browser.engine \"builtIn\" (the WebKit browser) is macOS-only; \
             use \"terminalBrowser\" or \"system\" (the default browser)",
        ));
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
    fn script_values() {
        use crate::lua::{SettingDef, SettingKind};
        let mut s = parse(r#"{"scripts": {"level": "off", "loud": "yes"}}"#).unwrap();
        s.script_settings = vec![
            SettingDef {
                key: "level".into(),
                title: "Level".into(),
                description: None,
                group: None,
                kind: SettingKind::Choice(vec![
                    ("off".into(), "Off".into()),
                    ("on".into(), "On".into()),
                ]),
                default: serde_json::json!("on"),
            },
            SettingDef {
                key: "loud".into(),
                title: "Loud".into(),
                description: None,
                group: None,
                kind: SettingKind::Boolean,
                default: serde_json::json!(false),
            },
        ];
        assert_eq!(s.script_value("level"), Some(serde_json::json!("off")));
        // Not a value it takes: the default.
        assert_eq!(s.script_value("loud"), Some(serde_json::json!(false)));
        assert_eq!(s.script_value("nope"), None);
    }

    #[test]
    fn defaults() {
        assert_eq!(parse("").unwrap(), Settings::default());
        assert_eq!(parse("{}").unwrap(), Settings::default());
        assert_eq!(parse(r#"{"panes": {}}"#).unwrap(), Settings::default());
    }

    #[test]
    fn gitlab_polling() {
        assert_eq!(Settings::default().gitlab.polling, GitlabPolling::Medium);
        for (v, want) in [
            ("off", GitlabPolling::Off),
            ("low", GitlabPolling::Low),
            ("medium", GitlabPolling::Medium),
            ("high", GitlabPolling::High),
        ] {
            let s = parse(&format!(r#"{{"gitlab": {{"polling": "{v}"}}}}"#)).unwrap();
            assert_eq!(s.gitlab.polling, want);
            assert_eq!(want.key(), v);
        }
        assert_eq!(GitlabPolling::Off.factor(), None);
        assert!(parse(r#"{"gitlab": {"polling": "fast"}}"#).is_err());
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
              "panes": { "resizeStep": 35.5, "dividerColor": "#ff0000", "focusBorderColor": "#00ff0080",
                         "openLinksIn": "newPane" }
            }"##,
        )
        .unwrap();
        assert_eq!(s.panes.resize_step, 35.5);
        assert_eq!(s.panes.divider_color, Some(rgb(0xff0000)));
        assert_eq!(s.panes.focus_border_color, rgba(0x00ff0080));
        assert_eq!(s.panes.open_links_in, OpenLinksIn::NewPane);
        assert_eq!(Panes::default().open_links_in, OpenLinksIn::SamePane);
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
    fn script_commands() {
        let s = parse(
            r#"{ "commands": [
                { "title": "Ticket", "script": "~/scripts/ticket.lua", "args": { "board": "PAY" } },
                { "title": "Tests", "script": "init.lua", "action": "tests", "shortcut": "cmd-alt-t" }
            ] }"#,
        )
        .unwrap();
        assert_eq!(s.commands[0].args.as_ref().unwrap()["board"], "PAY");
        assert!(s.commands[1].describe().contains("action “tests”"));
        assert!(s.commands[1].shortcut.is_some());
        // One kind per command, and action/args only with a script.
        assert!(
            parse(r#"{ "commands": [{ "title": "X", "script": "a.lua", "run": "ls" }] }"#).is_err()
        );
        assert!(
            parse(r#"{ "commands": [{ "title": "X", "run": "ls", "action": "a" }] }"#).is_err()
        );

        let dir = std::env::temp_dir().join(format!("abromux-lua-cmds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let init = dir.join("init.lua");
        std::fs::write(
            &init,
            r#"abromux.action{ name = "tests", title = "Run Tests", key = "cmd-alt-t", run = function() end }"#,
        )
        .unwrap();
        let cmds = lua_commands(&init).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].title, "Run Tests");
        assert_eq!(cmds[0].action.as_deref(), Some("tests"));
        assert!(cmds[0].shortcut.is_some());
        std::fs::write(
            &init,
            r#"abromux.action{ name = "x", title = "X", key = 42, run = print }"#,
        )
        .unwrap();
        assert!(lua_commands(&init).is_err());
        let _ = std::fs::remove_dir_all(&dir);
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
    fn notification_kinds() {
        use crate::notification::Kind;
        let d = NotificationSettings::default();
        assert_eq!(d.alert(Kind::NeedsInput), Alert::Banner);
        assert_eq!(d.alert(Kind::Done), Alert::Unread);
        assert_eq!(d.alert(Kind::Message), Alert::Banner);
        let s = parse(r#"{"notifications": {"done": "off", "needsInput": "unread"}}"#).unwrap();
        assert_eq!(s.notifications.alert(Kind::Done), Alert::Off);
        assert_eq!(s.notifications.alert(Kind::NeedsInput), Alert::Unread);
        assert_eq!(s.notifications.alert(Kind::Error), Alert::Banner);
        assert!(parse(r#"{"notifications": {"done": "loud"}}"#).is_err());
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
              "workspaceGroups": { "newWorkspacePlacement": "top", "groupByAgentStatus": true },
              "automation": { "workspaceAutoNaming": false }
            }"##,
        )
        .unwrap();
        assert!(!s.app.workspace_inherit_working_directory);
        assert!(s.app.keep_workspace_open_when_closing_last_surface);
        assert!(!AppSettings::default().keep_workspace_open_when_closing_last_surface);
        assert!(
            AppSettings::default().reorder_on_notification,
            "on, as in cmux"
        );
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
        assert!(!s.automation.workspace_auto_naming);
        let d = Settings::default();
        assert!(d.automation.workspace_auto_naming, "on unless turned off");
        assert_eq!(d.workspace_colors.parse("amber"), Some(0x7D6608));
        assert_eq!(d.app.new_workspace_placement, Placement::End);
        let err = |text: &str| parse(text).unwrap_err().to_string();
        assert!(err(r#"{"sidebar": {"fontSize": 50}}"#).contains("8–24"));
        assert!(
            err(r#"{"workspaceColors": {"indicatorStyle": "wash"}}"#).contains("unknown variant")
        );
        assert!(err(r#"{"workspaceColors": {"colors": {"X": "blue"}}}"#).contains("isn't #rrggbb"));
        // cmux takes a flag or a mode.
        let reorder = |v: &str| {
            parse(&format!(r#"{{"app": {{"reorderOnNotification": {v}}}}}"#))
                .map(|s| s.app.reorder_on_notification)
        };
        assert!(!reorder("false").unwrap());
        assert!(!reorder(r#""off""#).unwrap());
        assert!(reorder(r#""notifications""#).unwrap());
        assert!(reorder(r#""agentActivity""#).unwrap());
        assert!(reorder(r#""sometimes""#).is_err());
        let turn = |v: &str| {
            parse(&format!(
                r#"{{"notifications": {{"agentTurnComplete": "{v}"}}}}"#
            ))
            .map(|s| s.notifications.agent_turn_complete)
        };
        assert_eq!(turn("whenIdle").unwrap(), TurnComplete::WhenIdle);
        assert_eq!(turn("always").unwrap(), TurnComplete::Always);
        assert_eq!(turn("never").unwrap(), TurnComplete::Never);
        assert!(turn("sometimes").is_err());
        assert_eq!(
            NotificationSettings::default().agent_turn_complete,
            TurnComplete::WhenIdle,
            "as in cmux"
        );
        let appearance = |v: &str| {
            parse(&format!(r#"{{"app": {{"appearance": "{v}"}}}}"#)).map(|s| s.app.appearance)
        };
        assert_eq!(appearance("light").unwrap(), Appearance::Light);
        assert_eq!(appearance("dark").unwrap(), Appearance::Dark);
        assert_eq!(appearance("system").unwrap(), Appearance::System);
        assert_eq!(AppSettings::default().appearance, Appearance::System);
        assert!(appearance("sepia").is_err());
    }

    #[test]
    fn browser_engine() {
        assert_eq!(Settings::default().browser.engine, None);
        for v in ["terminalBrowser", "terminal-browser"] {
            let s = parse(&format!(r#"{{"browser": {{"engine": "{v}"}}}}"#)).unwrap();
            assert_eq!(s.browser.engine, Some(BrowserEngine::TerminalBrowser));
        }
        let s = parse(r#"{"browser": {"engine": "system"}}"#).unwrap();
        assert_eq!(s.browser.engine, Some(BrowserEngine::System));
        // The WebKit browser is macOS's; Linux says so.
        let builtin = parse(r#"{"browser": {"engine": "builtIn"}}"#);
        if abromux_platform::MACOS {
            assert_eq!(
                builtin.unwrap().browser.engine,
                Some(BrowserEngine::BuiltIn)
            );
        } else {
            let e = builtin.unwrap_err().to_string();
            assert!(e.contains("macOS-only"), "{e}");
            assert_ne!(default_engine(), BrowserEngine::BuiltIn);
        }
        assert!(!Settings::default().browser.terminal_browser_panes);
        let s = parse(r#"{"browser": {"terminalBrowserPanes": true}}"#).unwrap();
        assert!(s.browser.terminal_browser_panes);
        let e = parse(r#"{"browser": {"engine": "chrome"}}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown variant"), "{e}");
    }

    #[test]
    fn editor_program() {
        assert_eq!(Settings::default().editor.program, EditorProgram::Nvim);
        let s = parse(r#"{"editor": {"program": "tode"}}"#).unwrap();
        assert_eq!(s.editor.program, EditorProgram::Tode);
        assert!(parse(r#"{"editor": {"program": "emacs"}}"#).is_err());
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
