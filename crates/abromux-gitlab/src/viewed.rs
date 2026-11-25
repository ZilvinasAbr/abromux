//! Files marked Viewed, kept per merge request across tabs and restarts, as
//! GitLab's web UI keeps them in the browser.
//!
//! GitLab ties a mark to the file's content: a file changed by a later push
//! is not viewed any more, the rest stay viewed. Here each mark keeps the
//! head commit it was made at; when the merge request's head has moved,
//! the files changed between the two commits lose their marks
//! ([`GitLabView`](crate::view::GitLabView) asks GitLab's compare for them).

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

/// Merge requests remembered; the least lately touched go first.
const KEEP: usize = 500;

static FILE: OnceLock<PathBuf> = OnceLock::new();
static MARKS: Mutex<Option<Marks>> = Mutex::new(None);

/// Keep the marks in `file` (abromux: `gitlab-viewed.json` in its support
/// folder). Without it they last only while the app runs.
pub fn init(file: PathBuf) {
    let _ = FILE.set(file);
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mr {
    /// Viewed files by path: the head commit each was marked at.
    pub files: BTreeMap<String, String>,
    /// When it was last changed (unix seconds), to forget the oldest.
    pub at: i64,
}

impl Mr {
    /// Marks made at a head other than `head`: those commits, each once.
    pub fn stale(&self, head: &str) -> Vec<String> {
        let mut shas: Vec<String> = self
            .files
            .values()
            .filter(|s| s.as_str() != head)
            .cloned()
            .collect();
        shas.sort();
        shas.dedup();
        shas
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Marks {
    /// By [`key`].
    mrs: BTreeMap<String, Mr>,
}

pub fn key(host: &str, project: u64, iid: u64) -> String {
    format!("{} {project} {iid}", crate::cache::host_key(host))
}

fn with<R>(f: impl FnOnce(&mut Marks) -> R) -> R {
    let mut g = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    let marks = g.get_or_insert_with(|| {
        FILE.get()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    });
    f(marks)
}

fn save(marks: &Marks) {
    let Some(path) = FILE.get() else { return };
    let Ok(json) = serde_json::to_vec(marks) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written whole and moved into place: a crash never leaves half a file.
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// The marks of a merge request, if any were ever made.
pub fn get(host: &str, project: u64, iid: u64) -> Option<Mr> {
    with(|m| m.mrs.get(&key(host, project, iid)).cloned())
}

/// The merge request's viewed files are `paths` now, at head `head`: new
/// marks are made at `head`, those kept keep the commit they were made at.
pub fn set(host: &str, project: u64, iid: u64, head: &str, paths: &[String]) {
    with(|m| {
        let k = key(host, project, iid);
        let old = m.mrs.remove(&k).unwrap_or_default();
        let files: BTreeMap<String, String> = paths
            .iter()
            .map(|p| {
                let sha = old
                    .files
                    .get(p)
                    .cloned()
                    .unwrap_or_else(|| head.to_string());
                (p.clone(), sha)
            })
            .collect();
        let at = crate::time::now();
        m.mrs.insert(k, Mr { files, at });
        while m.mrs.len() > KEEP {
            let Some(oldest) = m
                .mrs
                .iter()
                .min_by_key(|(_, v)| v.at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            m.mrs.remove(&oldest);
        }
        save(m);
    });
}

/// GitLab said what changed between `from` and `head`: marks made at
/// `from` on those files are gone, the rest count as made at `head`.
/// Returns the viewed paths now.
pub fn moved(
    host: &str,
    project: u64,
    iid: u64,
    from: &str,
    head: &str,
    changed: &[String],
) -> Vec<String> {
    with(|m| {
        let Some(mr) = m.mrs.get_mut(&key(host, project, iid)) else {
            return vec![];
        };
        mr.files
            .retain(|p, sha| sha != from || !changed.contains(p));
        for sha in mr.files.values_mut() {
            if sha == from {
                *sha = head.to_string();
            }
        }
        mr.at = crate::time::now();
        let paths = mr.files.keys().cloned().collect();
        save(m);
        paths
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_follow_the_head_they_were_made_at() {
        let (h, p, i) = ("viewed-test.example.com", 1, 2);
        set(h, p, i, "aaa", &["a.rs".into(), "b.rs".into()]);
        assert!(get(h, p, i).unwrap().stale("aaa").is_empty());
        // Marked later, at another head: the first two keep theirs.
        set(
            h,
            p,
            i,
            "bbb",
            &["a.rs".into(), "b.rs".into(), "c.rs".into()],
        );
        let mr = get(h, p, i).unwrap();
        assert_eq!(mr.files["a.rs"], "aaa");
        assert_eq!(mr.files["c.rs"], "bbb");
        assert_eq!(mr.stale("bbb"), vec!["aaa".to_string()]);
        // a.rs changed between aaa and bbb: only it is unmarked.
        let now = moved(h, p, i, "aaa", "bbb", &["a.rs".into()]);
        assert_eq!(now, vec!["b.rs".to_string(), "c.rs".to_string()]);
        assert!(get(h, p, i).unwrap().stale("bbb").is_empty());
        // Unmarking all keeps an empty entry, so a stale tab can't bring
        // its old marks back.
        set(h, p, i, "bbb", &[]);
        assert!(get(h, p, i).unwrap().files.is_empty());
    }
}
