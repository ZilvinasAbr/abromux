//! Which GitLab projects and branches a workspace has checked out: each
//! checkout in its folder (`crate::folder`) and each terminal's repo, by
//! their `origin` remote and current branch (plus the branch the checkout
//! was made on, in case the agent moved off it before pushing).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `(host, group/project)` of a git remote URL.
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        // ssh://git@host:2222/g/p.git, https://user@host/g/p
        let (auth_host, path) = rest.split_once('/')?;
        let host = auth_host.rsplit('@').next()?;
        let host = host.split(':').next()?;
        (host, path)
    } else {
        // git@host:g/p.git
        let (auth_host, path) = url.split_once(':')?;
        (auth_host.rsplit('@').next()?, path)
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (!host.is_empty() && path.contains('/')).then(|| (host.to_ascii_lowercase(), path.to_string()))
}

/// The host part of the `gitlab.host` setting.
pub fn host_name(setting: &str) -> String {
    let s = setting.trim();
    let s = s.split_once("://").map_or(s, |(_, r)| r);
    s.split(['/', ':']).next().unwrap_or(s).to_ascii_lowercase()
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// One place to look: a directory, and a branch it was made on.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Place {
    pub dir: PathBuf,
    pub branch: Option<String>,
}

/// Projects on `host` and the branches checked out in `places`, by project.
pub fn discover(places: &[Place], host: &str) -> Vec<(String, Vec<String>)> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut seen_tops = std::collections::HashSet::new();
    for p in places {
        let Some(top) = git(&p.dir, &["rev-parse", "--show-toplevel"]) else {
            continue;
        };
        let fresh = seen_tops.insert(top.clone());
        let top = Path::new(&top);
        let Some((h, project)) =
            git(top, &["remote", "get-url", "origin"]).and_then(|u| parse_remote(&u))
        else {
            continue;
        };
        if h != host {
            continue;
        }
        let branches = found.entry(project).or_default();
        let current = fresh
            .then(|| git(top, &["branch", "--show-current"]))
            .flatten();
        for b in current.into_iter().chain(p.branch.clone()) {
            if !branches.contains(&b) {
                branches.push(b);
            }
        }
    }
    found.retain(|_, b| !b.is_empty());
    found.into_iter().collect()
}

/// The places of a workspace: its folder's checkouts, and terminal cwds.
pub fn places(home: Option<&Path>, cwds: &[String]) -> Vec<Place> {
    let mut out = vec![];
    if let Some(home) = home
        && let Some(meta) = crate::folder::read_meta(home)
    {
        for c in meta.checkouts {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remotes() {
        let want = Some(("gitlab.com".to_string(), "g/sub/p".to_string()));
        assert_eq!(parse_remote("git@gitlab.com:g/sub/p.git"), want);
        assert_eq!(parse_remote("https://gitlab.com/g/sub/p.git"), want);
        assert_eq!(parse_remote("https://oauth2:tok@gitlab.com/g/sub/p"), want);
        assert_eq!(parse_remote("ssh://git@GitLab.com:2222/g/sub/p.git/"), want);
        assert_eq!(parse_remote("/local/path"), None);
        assert_eq!(parse_remote("git@host:p.git"), None);
    }

    #[test]
    fn reads_host_setting() {
        assert_eq!(host_name("https://GitLab.example.com/"), "gitlab.example.com");
        assert_eq!(host_name("gitlab.com"), "gitlab.com");
        assert_eq!(host_name("gitlab.x:8443"), "gitlab.x");
    }
}
