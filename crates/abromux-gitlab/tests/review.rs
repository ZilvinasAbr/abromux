//! The review model on dev/gitlab/seed.py's merge requests: offline from the
//! fixtures (dev/gitlab/record-fixtures.sh), or live against the local
//! GitLab with `ABROMUX_GITLAB_LIVE=1 cargo test -p abromux-gitlab` after
//! dev/gitlab/up.sh. Both must pass the same checks.

use abromux_gitlab::{
    Compare,
    api::{Client, Curl, Response, Scope, Transport},
    load_compare, load_review,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

const LIVE_HOST: &str = "http://localhost:8929";
const LIVE_TOKEN: &str = "glpat-abromux-dev-root-token";

/// Answers from tests/fixtures, by the API path's shape.
struct Fixtures;

impl Transport for Fixtures {
    fn get(&self, url: &str, _: &str) -> anyhow::Result<Response> {
        let path = url
            .split("/api/v4/")
            .nth(1)
            .unwrap()
            .split('?')
            .next()
            .unwrap();
        let parts: Vec<&str> = path.split('/').collect();
        let name = match parts.as_slice() {
            ["projects", _, "members", "all"] => "members.json".into(),
            ["projects", _, "labels"] => "labels.json".into(),
            ["projects", _, "repository", "compare"] => "compare.json".into(),
            ["projects", _, "pipelines"] => "merge_pipelines.json".into(),
            ["projects", _, "repository", "commits", sha, "diff"] => {
                format!("commit.{sha}.diff.json")
            }
            ["projects", _, "merge_requests", iid, "versions", v] => {
                format!("mr{iid}.versions.{v}.json")
            }
            ["projects", _, "merge_requests", iid] => format!("mr{iid}.json"),
            ["projects", _, "merge_requests", iid, what] => format!("mr{iid}.{what}.json"),
            [one] => format!("{one}.json"),
            _ => anyhow::bail!("no fixture for {url}"),
        };
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        Ok(Response {
            status: 200,
            next_page: None,
            total: None,
            body: std::fs::read(&file)?,
            ..Default::default()
        })
    }
}

fn clients() -> Vec<(&'static str, Client)> {
    let mut c = vec![("fixtures", Client::new(LIVE_HOST, "x", Arc::new(Fixtures)))];
    if std::env::var("ABROMUX_GITLAB_LIVE").is_ok() {
        c.push(("live", Client::new(LIVE_HOST, LIVE_TOKEN, Arc::new(Curl))));
    }
    c
}

/// The seeded MR with this title, as (project id, iid).
fn find(c: &Client, title: &str) -> (u64, u64) {
    let mrs = c.merge_requests(Scope::All).unwrap();
    let mr = mrs
        .iter()
        .find(|m| m.title == title)
        .unwrap_or_else(|| panic!("no MR {title:?}"));
    (mr.project_id, mr.iid)
}

fn file<'a>(s: &'a Value, path: &str) -> &'a Value {
    s["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == path)
        .unwrap()
}

#[test]
fn rate_limiter_mr() {
    for (name, c) in clients() {
        let (p, iid) = find(&c, "Add rate limiter");
        let review = load_review(&c, p, iid).unwrap();
        // The role badges: alice made the project, bob was added as a developer.
        let role = |who: &str| {
            let u = review.general.iter().chain(
                review
                    .files
                    .iter()
                    .flat_map(|f| f.threads.iter().map(|(_, _, d)| d)),
            );
            let id = u
                .flat_map(|d| &d.notes)
                .find(|n| n.author.username == who)
                .unwrap()
                .author
                .id;
            review.roles.get(&id).copied()
        };
        assert_eq!(
            (role("alice"), role("bob")),
            (Some("Owner"), Some("Developer")),
            "{name}"
        );
        // Labels come with their colors.
        let labels: Vec<(&str, &str)> = review
            .mr
            .labels
            .iter()
            .map(|l| (l.name.as_str(), l.color.as_str()))
            .collect();
        assert_eq!(
            labels,
            [("backend", "#1068bf"), ("priority::high", "#e9be74")],
            "{name}"
        );
        // CI ran on the head; with no runner it waits, and after an hour
        // GitLab fails it as stuck.
        let pipeline = review.mr.head_pipeline.as_ref().expect("a pipeline");
        assert_eq!(pipeline.git_ref, "rate-limiter", "{name}");
        assert!(
            ["pending", "failed"].contains(&pipeline.label().0),
            "{name}: {}",
            pipeline.status
        );
        // main moved on after the branch was cut (seed.py's "Document LIMIT"…),
        // so the merge widget offers a rebase.
        assert_eq!(review.mr.diverged_commits_count, Some(2), "{name}");
        // bob approved: the sidebar's green check comes from GitLab's reviewer state.
        let bob = review
            .mr
            .reviewers
            .iter()
            .find(|u| u.username == "bob")
            .expect("bob reviews")
            .id;
        assert_eq!(
            review.review_states.get(&bob).map(String::as_str),
            Some("approved"),
            "{name}"
        );
        let s = review.summary();
        eprintln!("{name}: {s:#}");
        assert_eq!(s["ref"], "alice/review-fixtures!1", "{name}");
        assert_eq!(s["approved_by"], json!(["bob"]), "{name}");
        assert_eq!(s["unresolved"], 3, "{name}");
        assert_eq!(s["commits"], 2, "{name}");
        // GitLab's own notes ("requested review", "approved") are left out.
        let general: Vec<_> = s["general"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| &t["body"])
            .collect();
        assert_eq!(
            general,
            [&json!("Looks good overall, two nits. :thumbsup:")],
            "{name}"
        );
        assert_eq!(s["outdated"], json!([]), "{name}");

        let paths: Vec<_> = s["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths.len(), 7, "{name}: {paths:?}");

        let lim = file(&s, "src/limiter.rs");
        assert_eq!(
            (&lim["status"], &lim["added"], &lim["removed"]),
            (&json!("added"), &json!(19), &json!(0))
        );
        let t = lim["threads"].as_array().unwrap();
        assert_eq!(t.len(), 2, "{name}");
        assert_eq!(
            (&t[0]["line"]["new"], &t[0]["notes"], &t[0]["resolved"]),
            (&json!(6), &json!(3), &json!(false))
        );
        assert_eq!(t[1]["line"]["new"], 17);

        let legacy = file(&s, "src/legacy.rs");
        assert_eq!(legacy["status"], "deleted");
        assert_eq!(
            legacy["threads"][0]["line"],
            json!({ "old": 2, "new": null })
        );
        assert_eq!(legacy["threads"][0]["resolved"], true);

        assert_eq!(file(&s, "docs/NOTES.md")["status"], "renamed");
        assert_eq!(file(&s, "docs/NOTES.md")["old_path"], "docs/notes.txt");
        assert_eq!(file(&s, "docs/NOTES.md")["body"], "unchanged");
        assert_eq!(file(&s, "assets/logo.png")["body"], "binary");
        assert_eq!(file(&s, "src/config.rs")["threads"][0]["line"]["new"], 6);
    }
}

#[test]
fn versions_compare() {
    for (name, c) in clients() {
        let (p, iid) = find(&c, "Add rate limiter");
        let r = load_review(&c, p, iid).unwrap();
        // Pushed twice: the README change came with the second push.
        assert_eq!(r.versions.len(), 2, "{name}");
        let (v2, v1) = (r.versions[0].id, r.versions[1].id);
        let paths = |cmp: Compare| -> Vec<String> {
            let shown = load_compare(&c, &r, cmp).unwrap();
            assert!(
                shown.files.iter().all(|f| f.threads.is_empty()),
                "{name}: threads belong to the latest"
            );
            let mut p: Vec<String> = shown.files.iter().map(|f| f.path().to_string()).collect();
            p.sort();
            p
        };
        let v1_paths = paths(Compare {
            base: None,
            head: Some(v1),
            commit: None,
        });
        assert_eq!(v1_paths.len(), 6, "{name}: {v1_paths:?}");
        assert!(!v1_paths.contains(&"README.md".to_string()), "{name}");
        assert_eq!(
            paths(Compare {
                base: Some(v1),
                head: Some(v2),
                commit: None
            }),
            ["README.md"],
            "{name}"
        );
        // One commit: the newest is the README change alone.
        let one = load_compare(
            &c,
            &r,
            Compare {
                commit: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        let p: Vec<_> = one.files.iter().map(|f| f.path()).collect();
        assert_eq!(p, ["README.md"], "{name}");
        // The comparison's unchanged lines come from version 2's commit.
        let shown = load_compare(
            &c,
            &r,
            Compare {
                base: Some(v1),
                head: Some(v2),
                commit: None,
            },
        )
        .unwrap();
        assert_eq!(
            shown.mr.diff_refs.unwrap().head_sha,
            r.versions[0].head_commit_sha,
            "{name}"
        );
    }
}

#[test]
fn merged_and_closed() {
    for (name, c) in clients() {
        // Not in the open list: by their seeded iids.
        let p = find(&c, "Add rate limiter").0;
        let merged = load_review(&c, p, 5).unwrap();
        assert_eq!(merged.mr.state, "merged", "{name}");
        assert_eq!(
            merged.mr.merged_by.as_ref().map(|u| u.username.as_str()),
            Some("alice"),
            "{name}"
        );
        assert!(merged.mr.merge_commit_sha.is_some(), "{name}");
        // CI ran on main for the merge commit, as the merged widget shows.
        let mp = merged
            .merge_pipeline
            .as_ref()
            .expect("the merge commit's pipeline");
        assert_eq!(
            (mp.git_ref.as_str(), Some(&mp.sha)),
            ("main", merged.mr.merge_commit_sha.as_ref()),
            "{name}"
        );
        assert!(
            merged.system.iter().any(|n| n.body == "merged"),
            "{name}: a merged event"
        );
        let closed = load_review(&c, p, 6).unwrap();
        assert_eq!(closed.mr.state, "closed", "{name}");
        assert!(
            closed.system.iter().any(|n| n.body == "closed"),
            "{name}: a closed event"
        );
    }
}

#[test]
fn big_and_empty_mrs() {
    for (name, c) in clients() {
        let (p, iid) = find(&c, "Draft: big refactor");
        let s = load_review(&c, p, iid).unwrap().summary();
        assert_eq!(s["draft"], true, "{name}");
        let big = file(&s, "src/big.rs");
        assert_eq!(
            (&big["added"], &big["removed"]),
            (&json!(3000), &json!(3000)),
            "{name}"
        );

        let (p, iid) = find(&c, "Fix typo");
        let s = load_review(&c, p, iid).unwrap().summary();
        assert_eq!(s["unresolved"], 0, "{name}");
        assert_eq!(s["general"], json!([]), "{name}");
        assert_eq!(file(&s, "TYPO.md")["added"], 1, "{name}");

        let (p, iid) = find(&c, "Warmer icon");
        let s = load_review(&c, p, iid).unwrap().summary();
        assert_eq!(
            (
                &file(&s, "assets/icon.png")["status"],
                &file(&s, "assets/icon.png")["body"]
            ),
            (&json!("modified"), &json!("binary")),
            "{name}"
        );
    }
}
