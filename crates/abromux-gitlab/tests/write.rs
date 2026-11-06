//! Writing to GitLab: commenting, replying, resolving, line threads and
//! approving, as bob on the seeded !3 "Fix typo". Live only
//! (`ABROMUX_GITLAB_LIVE=1`, after dev/gitlab/up.sh). It deletes its notes
//! at the end, so the read tests still find !3 as seeded (GitLab's own
//! notes about resolving and approving stay).

use abromux_gitlab::{
    api::{Client, Curl, Scope},
    load_review,
};
use std::sync::Arc;

#[test]
fn comment_reply_resolve_approve() {
    if std::env::var("ABROMUX_GITLAB_LIVE").is_err() {
        eprintln!("skipped: set ABROMUX_GITLAB_LIVE=1 to write to the local GitLab");
        return;
    }
    let c = Client::new("http://localhost:8929", "glpat-abromux-dev-bob-token", Arc::new(Curl));
    let mr = c.merge_requests(Scope::All).unwrap().into_iter().find(|m| m.title == "Fix typo").expect("!3");
    let (p, iid) = (mr.project_id, mr.iid);

    // A comment on the merge request.
    let note = c.comment(p, iid, "A general comment from the tab.").unwrap();
    assert_eq!(note.author.username, "bob");

    // A thread on the changed line, then a reply, then resolving it.
    let r = load_review(&c, p, iid).unwrap();
    let refs = r.mr.diff_refs.clone().unwrap();
    let d = c.line_comment(p, iid, &refs, "TYPO.md", "TYPO.md", None, Some(1), "Nice catch.").unwrap();
    let reply = c.reply(p, iid, &d.id, "Agreed.").unwrap();
    let resolved = c.resolve(p, iid, &d.id, true).unwrap();
    assert!(resolved.resolved(), "resolved");

    // All of it reads back, the thread on its line.
    let r = load_review(&c, p, iid).unwrap();
    assert!(r.general.iter().any(|g| g.notes[0].body == "A general comment from the tab."));
    let f = r.files.iter().find(|f| f.path() == "TYPO.md").unwrap();
    let (_, _, t) = f.threads.iter().find(|(_, _, t)| t.id == d.id).expect("the thread on its line");
    assert_eq!(t.notes.len(), 2);
    assert!(t.resolved());

    // Unresolve; approve and take it back.
    assert!(!c.resolve(p, iid, &d.id, false).unwrap().resolved());
    c.approve(p, iid, true).unwrap();
    assert!(c.approvals(p, iid).unwrap().approved_by.iter().any(|a| a.user.username == "bob"));
    c.approve(p, iid, false).unwrap();
    assert!(!c.approvals(p, iid).unwrap().approved_by.iter().any(|a| a.user.username == "bob"));

    // Leave !3 as it was.
    for id in [reply.id, d.notes[0].id, note.id] {
        let url = format!("http://localhost:8929/api/v4/projects/{p}/merge_requests/{iid}/notes/{id}");
        let ok = std::process::Command::new("/usr/bin/curl")
            .args(["-sf", "-X", "DELETE", "-H", "PRIVATE-TOKEN: glpat-abromux-dev-bob-token", &url])
            .status()
            .unwrap()
            .success();
        assert!(ok, "deleting note {id}");
    }
}
