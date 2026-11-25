//! What GitLab's page does beyond reviewing, against the live GitLab (as
//! bob, on a merge request this test makes and removes): applying a
//! suggestion and a batch of them, to-dos, notifications, locking the
//! discussion, and editing and deleting a pending review comment. Live only
//! (`ABROMUX_GITLAB_LIVE=1`, after dev/gitlab/up.sh).

use abromux_gitlab::{
    api::{Client, Curl},
    load_review,
};
use std::{process::Command, sync::Arc};

const HOST: &str = "http://localhost:8929";
const BOB: &str = "glpat-abromux-dev-bob-token";

fn live() -> bool {
    if std::env::var("ABROMUX_GITLAB_LIVE").is_err() {
        eprintln!("skipped: set ABROMUX_GITLAB_LIVE=1 to write to the local GitLab");
        return false;
    }
    true
}

fn root(method: &str, path: &str, body: serde_json::Value) -> serde_json::Value {
    let out = Command::new("curl")
        .args(["-sf", "-X", method])
        .args(["-H", "PRIVATE-TOKEN: glpat-abromux-dev-root-token"])
        .args(["-H", "Content-Type: application/json", "--data"])
        .arg(body.to_string())
        .arg(format!("{HOST}/api/v4/{path}"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{method} {path} failed");
    serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null)
}

/// The branch goes when the test does, and with it its merge request's diff.
struct Cleanup(u64, String, Option<u64>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(iid) = self.2 {
            let _ = Command::new("curl")
                .args([
                    "-s",
                    "-X",
                    "DELETE",
                    "-H",
                    "PRIVATE-TOKEN: glpat-abromux-dev-root-token",
                ])
                .arg(format!(
                    "{HOST}/api/v4/projects/{}/merge_requests/{iid}",
                    self.0
                ))
                .output();
        }
        let _ = Command::new("curl")
            .args([
                "-s",
                "-X",
                "DELETE",
                "-H",
                "PRIVATE-TOKEN: glpat-abromux-dev-root-token",
            ])
            .arg(format!(
                "{HOST}/api/v4/projects/{}/repository/branches/{}",
                self.0,
                self.1.replace('/', "%2F")
            ))
            .output();
    }
}

#[test]
fn suggestions_todos_notifications_lock_and_drafts() {
    if !live() {
        return;
    }
    let c = Client::new(HOST, BOB, Arc::new(Curl));
    let p = c.project_id("alice/review-fixtures").unwrap();
    let branch = format!("actions-test-{}", std::process::id());
    let mut cleanup = Cleanup(p, branch.clone(), None);
    c.create_branch(p, &branch, "main").unwrap();
    root(
        "POST",
        &format!("projects/{p}/repository/commits"),
        serde_json::json!({ "branch": branch, "commit_message": "Add notes",
            "actions": [
                { "action": "create", "file_path": format!("{branch}.txt"),
                  "content": "one\ntwo\nthree\nfour\n" },
                { "action": "create", "file_path": format!("{branch}-b.txt"),
                  "content": "five\n" }] }),
    );
    let mr = c.create_mr(p, &branch, "main", "Actions test", "").unwrap();
    cleanup.2 = Some(mr.iid);
    let iid = mr.iid;
    // GitLab works out the diff in the background.
    let mut r = load_review(&c, p, iid).unwrap();
    for _ in 0..30 {
        if !r.files.is_empty() && r.mr.diff_refs.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        r = load_review(&c, p, iid).unwrap();
    }
    let refs = r.mr.diff_refs.clone().unwrap();
    let file = format!("{branch}.txt");
    let other = format!("{branch}-b.txt");
    let _ = &other;
    for (f, line, to) in [(&other, 1, "FIVE"), (&file, 2, "TWO"), (&file, 4, "FOUR")] {
        c.line_comment(
            p,
            iid,
            &refs,
            f,
            f,
            None,
            Some(line),
            &format!("Shout:\n\n```suggestion:-0+0\n{to}\n```"),
        )
        .unwrap();
    }
    let r = load_review(&c, p, iid).unwrap();
    let mut ids: Vec<u64> = r
        .files
        .iter()
        .flat_map(|f| f.threads.iter().map(|(_, _, d)| d))
        .flat_map(|d| d.notes.iter().flat_map(|n| n.suggestions.iter()))
        .map(|s| {
            assert!(s.appliable && !s.applied);
            s.id
        })
        .collect();
    assert_eq!(ids.len(), 3, "three suggestions, with their ids");
    // The other file's one first.
    ids.sort();
    // Two as a batch, in one commit with the message typed, then the third
    // alone.
    c.apply_suggestions(&ids[1..], Some("Apply two suggestions"))
        .unwrap();
    let raw = c.raw_file(p, &file, &branch).unwrap();
    assert_eq!(raw, "one\nTWO\nthree\nFOUR\n");
    let head = root(
        "GET",
        &format!("projects/{p}/repository/commits/{branch}"),
        serde_json::json!({}),
    );
    assert_eq!(head["title"], "Apply two suggestions", "the typed message");
    // Straight after that commit GitLab refuses for a moment ("A file has
    // been changed"); the client waits it out.
    c.apply_suggestions(&ids[..1], None).unwrap();
    assert_eq!(c.raw_file(p, &other, &branch).unwrap(), "FIVE\n");
    let r = load_review(&c, p, iid).unwrap();
    let applied = r
        .files
        .iter()
        .flat_map(|f| f.threads.iter().map(|(_, _, d)| d).chain(f.outdated.iter()))
        .chain(r.outdated.iter())
        .flat_map(|d| d.notes.iter().flat_map(|n| n.suggestions.iter()))
        .filter(|s| s.applied)
        .count();
    assert_eq!(applied, 3, "all applied");

    // To-do: added, seen with the review, then done.
    assert!(r.todos.is_empty());
    c.add_todo(p, iid).unwrap();
    let r = load_review(&c, p, iid).unwrap();
    assert_eq!(r.todos.len(), 1, "the to-do shows");
    c.todo_done(r.todos[0]).unwrap();
    assert!(load_review(&c, p, iid).unwrap().todos.is_empty());

    // Notifications: off, then on again (and asking twice is fine).
    c.subscribe(p, iid, false).unwrap();
    c.subscribe(p, iid, false).unwrap();
    assert_eq!(c.merge_request(p, iid).unwrap().subscribed, Some(false));
    c.subscribe(p, iid, true).unwrap();
    assert_eq!(c.merge_request(p, iid).unwrap().subscribed, Some(true));

    // Lock discussion, and unlock.
    c.update(p, iid, serde_json::json!({ "discussion_locked": true }))
        .unwrap();
    assert_eq!(
        c.merge_request(p, iid).unwrap().discussion_locked,
        Some(true)
    );
    c.update(p, iid, serde_json::json!({ "discussion_locked": false }))
        .unwrap();
    assert_eq!(
        c.merge_request(p, iid).unwrap().discussion_locked,
        Some(false)
    );

    // A pending comment: written, changed, taken out.
    let d = c.draft_comment(p, iid, "pending", None, false).unwrap();
    c.edit_draft(p, iid, d.id, "pending, changed").unwrap();
    let drafts = c.draft_notes(p, iid).unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].note, "pending, changed");
    c.delete_draft(p, iid, d.id).unwrap();
    assert!(c.draft_notes(p, iid).unwrap().is_empty());

    // A review: a reply into a thread and a general comment held, then
    // submitted with a summary, requesting changes.
    let r = load_review(&c, p, iid).unwrap();
    let thread = r
        .files
        .iter()
        .flat_map(|f| f.threads.iter().map(|(_, _, d)| d).chain(f.outdated.iter()))
        .chain(r.outdated.iter())
        .next()
        .unwrap()
        .id
        .clone();
    let me = c.current_user().unwrap();
    c.update(p, iid, serde_json::json!({ "reviewer_ids": [me.id] }))
        .unwrap();
    c.draft_comment(p, iid, "held reply", Some(&thread), false)
        .unwrap();
    c.draft_comment(p, iid, "held comment", None, false)
        .unwrap();
    let held = c.draft_notes(p, iid).unwrap();
    assert_eq!(held.len(), 2);
    assert!(
        held.iter()
            .any(|d| d.discussion_id.as_deref() == Some(thread.as_str()))
    );
    c.publish_review(p, iid, Some("The summary"), Some("requested_changes"))
        .unwrap();
    let state = c
        .reviewers(p, iid)
        .unwrap()
        .into_iter()
        .find(|x| x.user.id == me.id)
        .map(|x| x.state);
    assert_eq!(state.as_deref(), Some("requested_changes"));
    let mut published = false;
    for _ in 0..10 {
        let d = c.discussions(p, iid).unwrap();
        let bodies: Vec<&str> = d
            .iter()
            .flat_map(|d| &d.notes)
            .map(|n| n.body.as_str())
            .collect();
        if ["held reply", "held comment", "The summary"]
            .iter()
            .all(|b| bodies.contains(b))
        {
            let t = d.iter().find(|x| x.id == thread).unwrap();
            assert!(
                t.notes.iter().any(|n| n.body == "held reply"),
                "the reply in its thread"
            );
            published = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert!(published, "the review's comments and summary");
    assert!(c.draft_notes(p, iid).unwrap().is_empty());

    // Internal notes: a comment (REST) and a thread (GraphQL), seen as such.
    c.internal_comment(p, iid, "internal comment").unwrap();
    c.internal_thread(c.merge_request(p, iid).unwrap().id, "internal thread")
        .unwrap();
    let d = c.discussions(p, iid).unwrap();
    let find = |b: &str| d.iter().find(|x| x.notes[0].body == b).unwrap();
    assert!(find("internal comment").notes[0].internal);
    let t = find("internal thread");
    assert!(t.notes[0].internal && t.notes[0].resolvable);

    // Quick actions in a comment run, and leave no comment.
    c.comment(p, iid, "/label ~docs\n/estimate 2h").unwrap();
    let m = c.merge_request(p, iid).unwrap();
    assert!(
        m.labels.iter().any(|l| l.name == "docs"),
        "labelled by /label"
    );
    assert_eq!(m.time_stats.time_estimate, 7200, "estimated by /estimate");
    assert!(
        !c.discussions(p, iid)
            .unwrap()
            .iter()
            .flat_map(|d| &d.notes)
            .any(|n| n.body.starts_with("/label"))
    );
    // Merge checks, as the merge widget lists them.
    let checks = c.merge_checks("alice/review-fixtures", iid).unwrap();
    assert!(
        checks
            .iter()
            .any(|(id, st)| id == "NOT_OPEN" && st == "SUCCESS")
    );
    // Comment templates come back (none, or what the user made).
    c.saved_replies().unwrap();

    // Work items: an issue made and linked as Closes, seen, then unlinked.
    let full = "alice/review-fixtures";
    let issue = c.create_issue(p, "Work item test (temporary)").unwrap();
    c.link_work_item(full, p, iid, issue, "CLOSES").unwrap();
    let items = c.work_items(full, iid).unwrap();
    let it = items.iter().find(|w| w.iid == issue).expect("linked");
    assert_eq!(it.link_type, "CLOSES");
    c.unlink_work_item(full, iid, &it.id).unwrap();
    assert!(
        !c.work_items(full, iid)
            .unwrap()
            .iter()
            .any(|w| w.iid == issue)
    );
    // "Create issue to resolve thread": an issue, and the thread resolved.
    let open = c
        .discussions(p, iid)
        .unwrap()
        .into_iter()
        .find(|d| d.resolvable() && !d.resolved())
        .expect("an open thread");
    let url = c
        .issue_for_thread(p, iid, &open.id, "Actions test")
        .unwrap();
    assert!(
        url.contains("/-/issues/") || url.contains("/-/work_items/"),
        "{url}"
    );
    let after = c.discussions(p, iid).unwrap();
    assert!(
        after.iter().find(|d| d.id == open.id).unwrap().resolved(),
        "resolved by the issue"
    );
    // The issues go with the test.
    let made: u64 = url.rsplit('/').next().unwrap().parse().unwrap();
    for i in [issue, made] {
        root(
            "DELETE",
            &format!("projects/{p}/issues/{i}"),
            serde_json::json!({}),
        );
    }

    // Context commits: one of main's added, listed, removed.
    let main = c.branch_commits(p, "main").unwrap();
    let sha = main[1].id.clone();
    c.context_commits_change(p, iid, std::slice::from_ref(&sha), true)
        .unwrap();
    assert!(
        c.context_commits(p, iid)
            .unwrap()
            .iter()
            .any(|x| x.id == sha)
    );
    c.context_commits_change(p, iid, std::slice::from_ref(&sha), false)
        .unwrap();
    assert!(
        !c.context_commits(p, iid)
            .unwrap()
            .iter()
            .any(|x| x.id == sha)
    );
}
