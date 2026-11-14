//! What scripts and agents report about a workspace, shown under it in the
//! sidebar like cmux: status pills (`set-status`), a progress bar
//! (`set-progress`) and log entries (`log`).

use std::time::SystemTime;

/// Oldest log entries are dropped past this.
pub const KEEP_LOG: usize = 200;

/// Icons a status may name: the ones bundled with the app.
pub const ICONS: &[&str] = &[
    "terminal",
    "globe",
    "plus",
    "x",
    "split-right",
    "split-down",
    "chevron-left",
    "chevron-right",
    "chevron-down",
    "chevron-up",
    "rotate-cw",
    "panel-left",
    "bell",
    "git-branch",
    "folder",
    "search",
    "circle-x",
    "pencil",
];

pub const RUNNING: u32 = 0x6a9fd8;
pub const SUCCESS: u32 = 0x7fb069;
pub const WARNING: u32 = 0xe0a84f;
pub const ERROR: u32 = 0xe5534b;

pub struct Status<O> {
    pub key: String,
    pub value: String,
    pub icon: Option<String>,
    pub color: Option<u32>,
    /// The terminal it describes (an agent's status), cleared when it closes.
    pub owner: Option<O>,
}

pub struct Progress {
    /// 0.0–1.0.
    pub value: f32,
    pub label: Option<String>,
}

pub struct LogEntry {
    pub level: LogLevel,
    pub source: Option<String>,
    pub message: String,
    pub at: std::time::SystemTime,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LogLevel {
    Info,
    Progress,
    Success,
    Warning,
    Error,
}

impl LogLevel {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "info" => Self::Info,
            "progress" => Self::Progress,
            "success" => Self::Success,
            "warning" | "warn" => Self::Warning,
            "error" => Self::Error,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Progress => "progress",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// JSON for `list-status` and the workspace description.
pub fn status_json<O>(s: &Status<O>) -> serde_json::Value {
    serde_json::json!({
        "key": s.key,
        "value": s.value,
        "icon": s.icon,
        "color": s.color.map(|c| format!("#{c:06x}")),
    })
}

pub fn log_json(l: &LogEntry, now: SystemTime) -> serde_json::Value {
    serde_json::json!({
        "level": l.level.as_str(),
        "source": l.source,
        "message": l.message,
        "age_seconds": now.duration_since(l.at).unwrap_or_default().as_secs(),
    })
}
