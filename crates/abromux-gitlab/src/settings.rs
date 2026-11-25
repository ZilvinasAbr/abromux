//! The plugin's settings: `"gitlab": {…}` in abromux's config.json, and its
//! page in the Settings window, both from [`page`]. abromux checks the
//! file's section against it; [`get`] reads it.

use abromux_plugin::setting::{SettingDef, SettingsPage};
use gpui::App;
use serde::Deserialize;

/// `gitlab` in config.json.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// GitLab in abromux at all: its tabs for merge request links, the
    /// background merge requests cache, and the workspaces'
    /// merge requests in the sidebar. Off: merge request links are web
    /// pages like any other, and nothing asks GitLab anything. Local diffs
    /// (`abromux diff`) work either way.
    pub enabled: bool,
    /// The GitLab the cache uses, e.g. `gitlab.example.com`. None:
    /// the one signed in to in a GitLab tab. Its token is the tab's (the
    /// Keychain). Workspace checkouts whose remote is elsewhere are left out.
    pub host: Option<String>,
    /// How often merge requests are checked in the background. Off: never
    /// (tabs load what they open, and Refresh still checks once).
    pub polling: Polling,
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

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            host: None,
            polling: Polling::default(),
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
pub enum Polling {
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

impl Polling {
    /// What every interval is multiplied by; None when off.
    pub fn factor(self) -> Option<f64> {
        match self {
            Polling::Off => None,
            Polling::Low => Some(4.),
            Polling::Medium => Some(1.),
            Polling::High => Some(0.5),
        }
    }
}

/// The settings as config.json has them (checked by abromux against
/// [`page`]; anything it can't read is the default).
pub fn get(cx: &App) -> Settings {
    serde_json::from_value(abromux_plugin::setting::section(crate::PLUGIN.id, cx))
        .unwrap_or_default()
}

/// Whether GitLab is turned on (`gitlab.enabled`).
pub fn enabled(cx: &App) -> bool {
    get(cx).enabled
}

/// The Settings window's GitLab page, and what `"gitlab"` may hold.
pub fn page() -> SettingsPage {
    let d = Settings::default();
    SettingsPage {
        title: "GitLab",
        icon: "icons/git-pull-request.svg",
        description: Some(
            "Merge requests kept cached and checked in the background, so they open \
             at once in a GitLab tab (⇧⌘M finds one).",
        ),
        settings: vec![
            SettingDef::boolean("enabled", "Use GitLab", d.enabled)
                .group("GitLab")
                .description(
                    "Merge request links open in GitLab tabs, merge requests are checked \
                     in the background, and workspaces list their merge requests. \
                     Off: those links are web pages like any other and nothing asks GitLab \
                     anything. Local diffs (abromux diff) work either way.",
                ),
            SettingDef::text("host", "Host", "the GitLab tab's")
                .group("Connection")
                .description(
                    "Your GitLab, e.g. gitlab.example.com. Empty: the one you signed in \
                     to in a GitLab tab, with its token. Workspace checkouts whose \
                     remote is elsewhere are left out.",
                ),
            SettingDef::boolean(
                "includeReviewRequests",
                "Review requests",
                d.include_review_requests,
            )
            .group("Connection")
            .description("\"Mine\" also lists merge requests you're asked to review."),
            SettingDef::boolean("includeAssigned", "Assigned to me", d.include_assigned)
                .group("Connection")
                .description("\"Mine\" also lists merge requests assigned to you."),
            SettingDef::number(
                "recentLimit",
                "Recently opened",
                d.recent_limit.into(),
                (5., 500., 5.),
            )
            .group("Cache")
            .description(
                "How many merge requests you opened stay cached. Merged and \
                 closed ones are dropped at once.",
            ),
            SettingDef::number(
                "cacheSizeMb",
                "Disk space",
                d.cache_size_mb.into(),
                (50., 20000., 50.),
            )
            .unit("MB")
            .group("Cache")
            .description(
                "For cached merge requests. Past it, the longest-unopened ones' \
                 go first; they stay listed.",
            ),
            SettingDef::number(
                "maxFileDiffKb",
                "Largest file diff",
                d.max_file_diff_kb.into(),
                (64., 65536., 64.),
            )
            .unit("KB")
            .group("Cache")
            .description("Bigger files' diffs are fetched when they're shown."),
            SettingDef::number(
                "maxMrDiffMb",
                "Largest merge request diff",
                d.max_mr_diff_mb.into(),
                (1., 500., 1.),
            )
            .unit("MB")
            .group("Cache")
            .description("Past it, files' diffs are fetched when they're shown."),
            SettingDef::number(
                "maxMrDiffFiles",
                "Most files per diff",
                d.max_mr_diff_files.into(),
                (100., 50000., 100.),
            )
            .group("Cache")
            .description("…or after this many files."),
            SettingDef::choice(
                "polling",
                "Polling level",
                &[
                    ("off", "Off"),
                    ("low", "Low"),
                    ("medium", "Medium"),
                    ("high", "High"),
                ],
                "medium",
            )
            .group("Checking for Changes")
            .description("How often merge requests are checked in the background."),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Each declared default is what the typed settings default to, so the
    /// page and [`get`] agree.
    #[test]
    fn defaults_agree() {
        let p = page();
        let filled = p.filled(&json!({}));
        let s: Settings = serde_json::from_value(filled).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn polling() {
        let p = page();
        for (v, want) in [
            ("off", Polling::Off),
            ("low", Polling::Low),
            ("medium", Polling::Medium),
            ("high", Polling::High),
        ] {
            let section = json!({ "polling": v });
            assert!(p.check("gitlab", &section).is_ok());
            let s: Settings = serde_json::from_value(section).unwrap();
            assert_eq!(s.polling, want);
        }
        assert_eq!(Polling::Off.factor(), None);
        assert!(p.check("gitlab", &json!({"polling": "fast"})).is_err());
        assert!(p.check("gitlab", &json!({"onScreenSeconds": 20})).is_err());
    }
}
