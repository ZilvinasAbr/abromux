//! A merge request past GitLab's limits for one diff: the seeded !9
//! "Vendor everything" (1,203 files, ~70,000 lines). Live only
//! (`ABROMUX_GITLAB_LIVE=1`, after dev/gitlab/up.sh and seed.py).

use abromux_gitlab::{
    api::{Client, Curl, Scope},
    diff::Body,
    load_review,
};
use std::sync::Arc;

#[test]
fn every_file_of_a_huge_merge_request() {
    if std::env::var("ABROMUX_GITLAB_LIVE").is_err() {
        eprintln!("skipped: set ABROMUX_GITLAB_LIVE=1 to read the local GitLab");
        return;
    }
    let c = Client::new("http://localhost:8929", "glpat-abromux-dev-bob-token", Arc::new(Curl));
    let mr = c.merge_requests(Scope::All).unwrap().into_iter().find(|m| m.title == "Vendor everything").expect("!9");
    let r = load_review(&c, mr.project_id, mr.iid).unwrap();
    // GitLab's diff stops at 1000; GraphQL lists the rest, with their counts.
    assert_eq!(r.files.len(), 1203);
    let (added, removed) = r.files.iter().fold((0, 0), |(a, d), f| {
        let (x, y) = f.counts();
        (a + x, d + y)
    });
    assert_eq!((added, removed), (70001, 0));
    let pending: Vec<_> = r.files.iter().filter(|f| *f.body() == Body::Pending).collect();
    assert_eq!(pending.len(), 203);
    // One of them fetched as the tab does: both sides, diffed here.
    let f = r.files.iter().find(|f| f.path() == "huge/generated.rs").unwrap();
    let refs = r.mr.diff_refs.clone().unwrap();
    let old = c.raw_maybe(mr.project_id, &f.meta.old_path, &refs.base_sha).unwrap();
    let new = c.raw_maybe(mr.project_id, &f.meta.new_path, &refs.head_sha).unwrap();
    assert!(old.is_none(), "added in this merge request");
    let body = Body::local(old.as_deref(), new.as_deref(), f.path());
    assert_eq!(body.counts(), (40000, 0));
}

/// curl's batch (one process, parallel transfers over reused connections)
/// answers as one request at a time does, statuses and headers included.
#[test]
fn a_batch_answers_as_single_requests_do() {
    if std::env::var("ABROMUX_GITLAB_LIVE").is_err() {
        return;
    }
    use abromux_gitlab::api::Transport;
    let base = "http://localhost:8929/api/v4";
    let token = "glpat-abromux-dev-bob-token";
    let urls: Vec<String> = [
        "version",
        "user",
        "projects/alice%2Freview-fixtures/merge_requests?state=all&per_page=2",
        "projects/alice%2Freview-fixtures/merge_requests/1/discussions?per_page=100&page=1",
        "projects/no%2Fsuch-project",
    ]
    .iter()
    .map(|p| format!("{base}/{p}"))
    .collect();
    let batch = Curl.get_many(&urls, token);
    assert_eq!(batch.len(), urls.len());
    for (url, got) in urls.iter().zip(batch) {
        let got = got.unwrap();
        let one = Curl.get(url, token).unwrap();
        assert_eq!((got.status, got.next_page, got.total), (one.status, one.next_page, one.total), "{url}");
        assert_eq!(got.body, one.body, "{url}");
    }
}
