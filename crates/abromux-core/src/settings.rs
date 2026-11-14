//! The settings model: what config.json can say, parsed. (Growing: the
//! rest is still in the app's settings.rs.)

use serde::Deserialize;

/// How terminals come back after abromux quits. Applies to terminals opened
/// after it's set; ones already running in the holder are taken back
/// whatever it says.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Persistence {
    /// `"abromux"`: shells run in abromux's holder (`abromux_term::hold`) and keep
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
