//! GitLab in the app. The merge requests cache is the plugin's
//! (`abromux_gitlab::mrs`, re-exported here); what's left is what only the
//! app knows: which merge requests its tabs show, opening one (`views`),
//! and where a workspace's checkouts are (`places`).

pub mod views;

pub use abromux_gitlab::mrs::*;

use remote::Place;
use std::path::{Path, PathBuf};

/// A checkout in a workspace folder, as `.abromux/workspace.json` lists it
/// (the workspace folders script writes it).
#[derive(serde::Deserialize)]
struct Checkout {
    path: String,
    #[serde(default)]
    branch: Option<String>,
}

fn checkouts(home: &Path) -> Vec<Checkout> {
    #[derive(serde::Deserialize)]
    struct Meta {
        #[serde(default)]
        checkouts: Vec<Checkout>,
    }
    std::fs::read_to_string(home.join(".abromux/workspace.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Meta>(&t).ok())
        .map(|m| m.checkouts)
        .unwrap_or_default()
}

/// The places of a workspace: its folder's checkouts, and terminal cwds.
pub fn places(home: Option<&Path>, cwds: &[String]) -> Vec<Place> {
    let mut out = vec![];
    if let Some(home) = home {
        for c in checkouts(home) {
            out.push(Place {
                dir: home.join(&c.path),
                branch: c.branch,
            });
        }
    }
    for c in cwds {
        out.push(Place {
            dir: PathBuf::from(c),
            branch: None,
        });
    }
    out.sort();
    out.dedup();
    out
}
