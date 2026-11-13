//! Managing a merge request as GitLab's page does: editing a comment and
//! deleting it, marking as draft and ready, editing the title and
//! description, closing and reopening, running, cancelling and retrying a
//! pipeline (as bob on the seeded !3, put back as it was), and merging (as
//! alice, who may, on a merge request this test makes into a branch it
//! makes, all removed after). Live only
//! (`ABROMUX_GITLAB_LIVE=1`, after dev/gitlab/up.sh).

use abromux_gitlab::{
    api::{Client, Curl, MergeOptions, Scope},
    load_review,
};
use std::{process::Command, sync::Arc};

const HOST: &str = "http://localhost:8929";

fn live() -> bool {
    if std::env::var("ABROMUX_GITLAB_LIVE").is_err() {
        eprintln!("skipped: set ABROMUX_GITLAB_LIVE=1 to write to the local GitLab");
        return false;
    }
    true
}

/// A write the client doesn't make: setting up what a test needs.
fn root(method: &str, path: &str, body: serde_json::Value) -> serde_json::Value {
    as_user("glpat-abromux-dev-root-token", method, path, body)
}

fn as_user(token: &str, method: &str, path: &str, body: serde_json::Value) -> serde_json::Value {
    let out = Command::new("curl")
        .args([
            "-sf",
            "-X",
            method,
            "-H",
            &format!("PRIVATE-TOKEN: {token}"),
            "-H",
            "Content-Type: application/json",
        ])
        .args([
            "--data",
            &body.to_string(),
            &format!("{HOST}/api/v4/{path}"),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{method} {path}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null)
}

#[test]
fn edit_draft_close_and_pipelines() {
    if !live() {
        return;
    }
    let c = Client::new(HOST, "glpat-abromux-dev-bob-token", Arc::new(Curl));
    let mr = c
        .merge_requests(Scope::All)
        .unwrap()
        .into_iter()
        .find(|m| m.title == "Fix typo")
        .expect("!3");
    let (p, iid) = (mr.project_id, mr.iid);
    let description = mr.description.clone().unwrap_or_default();

    // A comment of bob's, edited, then deleted.
    let note = c.comment(p, iid, "Typo in the comment.").unwrap();
    assert_eq!(
        c.edit_note(p, iid, note.id, "Fixed the comment.")
            .unwrap()
            .body,
        "Fixed the comment."
    );
    let r = load_review(&c, p, iid).unwrap();
    assert!(
        r.general
            .iter()
            .any(|g| g.notes[0].id == note.id && g.notes[0].body == "Fixed the comment.")
    );
    c.delete_note(p, iid, note.id).unwrap();
    let r = load_review(&c, p, iid).unwrap();
    assert!(
        !r.general
            .iter()
            .any(|g| g.notes.iter().any(|n| n.id == note.id)),
        "deleted"
    );

    // Draft and ready, by the title.
    let t = abromux_gitlab::draft_title(&mr.title, true);
    assert!(
        c.update(p, iid, serde_json::json!({ "title": t }))
            .unwrap()
            .draft
    );
    let t = abromux_gitlab::draft_title(&t, false);
    let back = c.update(p, iid, serde_json::json!({ "title": t })).unwrap();
    assert!(!back.draft);
    assert_eq!(back.title, "Fix typo");

    // Edit, then put back.
    let edited = c
        .update(
            p,
            iid,
            serde_json::json!({ "title": "Fix typo (edited)", "description": "New words." }),
        )
        .unwrap();
    assert_eq!(
        (edited.title.as_str(), edited.description.as_deref()),
        ("Fix typo (edited)", Some("New words."))
    );
    c.update(
        p,
        iid,
        serde_json::json!({ "title": "Fix typo", "description": description }),
    )
    .unwrap();

    // Close and reopen.
    assert_eq!(c.set_open(p, iid, false).unwrap().state, "closed");
    assert_eq!(c.set_open(p, iid, true).unwrap().state, "opened");

    // The latest pipeline (failed or canceled: there's no runner), retried
    // (pending again), then cancelled.
    let id = c.pipelines(p, iid).unwrap()[0].id;
    let status = || {
        c.pipelines(p, iid)
            .unwrap()
            .into_iter()
            .find(|x| x.id == id)
            .unwrap()
            .status
    };
    c.retry_pipeline(p, id).unwrap();
    assert!(abromux_gitlab::pipeline_active(&status()), "{}", status());
    c.cancel_pipeline(p, id).unwrap();
    // Cancelling finishes in the background.
    let canceled = (0..20).any(|_| {
        let s = status();
        std::thread::sleep(std::time::Duration::from_millis(300));
        matches!(s.as_str(), "canceled" | "canceling")
    });
    assert!(canceled, "{}", status());
    // The fixtures' CI has no merge request pipelines: GitLab says so.
    let e = c.run_pipeline(p, iid).unwrap_err();
    assert!(format!("{e:#}").contains("empty"), "{e:#}");
}

#[test]
fn merge_squash_and_delete_branch() {
    if !live() {
        return;
    }
    // alice owns the project, so she may merge into the protected main
    // (root, an admin but no member, may not): a token of hers for this.
    let alice = root("GET", "users?username=alice", serde_json::json!({}))[0]["id"]
        .as_u64()
        .unwrap();
    // GitLab caps a token's life at a year; a week will do (it's revoked at the end).
    let expires = String::from_utf8(
        Command::new("date")
            .args(["-v+7d", "+%F"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let expires = expires.trim();
    let pat = root(
        "POST",
        &format!("users/{alice}/personal_access_tokens"),
        serde_json::json!({ "name": "abromux merge test", "scopes": ["api"], "expires_at": expires }),
    );
    let token = pat["token"].as_str().unwrap().to_string();
    // Whatever happens, take the token and the branches away after.
    struct Cleanup(u64, u64, Vec<String>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for b in &self.2 {
                let _ = std::panic::catch_unwind(|| {
                    root(
                        "DELETE",
                        &format!("projects/{}/repository/branches/{b}", self.1),
                        serde_json::json!({}),
                    )
                });
            }
            let _ = std::panic::catch_unwind(|| {
                root(
                    "DELETE",
                    &format!("personal_access_tokens/{}", self.0),
                    serde_json::json!({}),
                )
            });
        }
    }
    let c = Client::new(HOST, &token, Arc::new(Curl));
    let p = c.project_id("alice/review-fixtures").unwrap();
    let pat_id = pat["id"].as_u64().unwrap();
    // Into a target of its own, so main stays as seeded.
    let branch = format!("merge-test-{}", std::process::id());
    let target = format!("{branch}-target");
    let _cleanup = Cleanup(pat_id, p, vec![branch.clone(), target.clone()]);
    for b in [&target, &branch] {
        root(
            "POST",
            &format!("projects/{p}/repository/branches"),
            serde_json::json!({ "branch": b, "ref": "main" }),
        );
    }
    for i in 0..2 {
        root(
            "POST",
            &format!("projects/{p}/repository/commits"),
            serde_json::json!({ "branch": branch, "commit_message": format!("Step {i}"),
                "actions": [{ "action": "create", "file_path": format!("{branch}-{i}.txt"), "content": "x\n" }] }),
        );
    }
    let mr = root(
        "POST",
        &format!("projects/{p}/merge_requests"),
        serde_json::json!({ "source_branch": branch, "target_branch": target, "title": "Merged by a test" }),
    );
    let iid = mr["iid"].as_u64().unwrap();
    // GitLab checks mergeability in the background.
    let mut m = c.merge_request(p, iid).unwrap();
    for _ in 0..30 {
        if m.detailed_merge_status.as_deref() == Some("mergeable") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        m = c.merge_request(p, iid).unwrap();
    }
    assert!(
        m.user.as_ref().is_some_and(|u| u.can_merge),
        "alice may merge"
    );
    let opts = MergeOptions {
        squash: true,
        remove_source_branch: true,
        auto: false,
        sha: m.sha.clone(),
    };
    let merged = c.merge(p, iid, &opts).unwrap();
    assert_eq!(merged.state, "merged");
    assert!(merged.squash_commit_sha.is_some(), "squashed");
    // The branch goes (in the background).
    let gone = (0..20).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "-H",
                "PRIVATE-TOKEN: glpat-abromux-dev-root-token",
            ])
            .arg(format!(
                "{HOST}/api/v4/projects/{p}/repository/branches/{branch}"
            ))
            .output()
            .is_ok_and(|o| o.stdout == b"404")
    });
    assert!(gone, "source branch deleted");
}
