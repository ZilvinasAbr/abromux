//! The head pipeline's reports, against the live GitLab: the merge request
//! `dev/gitlab/reports.sh` makes, whose pipeline writes a JUnit report with
//! one failure and a Code Quality report with two findings. Live only
//! (`ABROMUX_GITLAB_LIVE=1`), and only once that script has run.

use abromux_gitlab::{
    api::{Client, Curl},
    load_review,
};
use std::sync::Arc;

#[test]
fn loads_test_and_code_quality_reports() {
    if std::env::var("ABROMUX_GITLAB_LIVE").is_err() {
        eprintln!("skipped: set ABROMUX_GITLAB_LIVE=1");
        return;
    }
    let c = Client::new(
        "http://localhost:8929",
        "glpat-abromux-dev-bob-token",
        Arc::new(Curl),
    );
    let p = c.project_id("alice/review-fixtures").unwrap();
    let mrs: Vec<serde_json::Value> = c
        .get(&format!(
            "projects/{p}/merge_requests?source_branch=reports"
        ))
        .unwrap();
    let Some(iid) = mrs.first().and_then(|m| m["iid"].as_u64()) else {
        eprintln!("skipped: run dev/gitlab/reports.sh first");
        return;
    };
    let r = load_review(&c, p, iid).unwrap();
    let t = r.reports.tests.as_ref().expect("a test report");
    assert_eq!((t.total_count, t.failed_count, t.skipped_count), (6, 1, 1));
    let failed: Vec<_> = t
        .test_suites
        .iter()
        .flat_map(|s| &s.test_cases)
        .filter(|c| c.status == "failed")
        .collect();
    assert_eq!(failed[0].name, "blocks_over_limit");
    assert!(
        failed[0]
            .system_output
            .as_deref()
            .unwrap_or_default()
            .contains("panicked")
            || failed[0]
                .stack_trace
                .as_deref()
                .unwrap_or_default()
                .contains("panicked")
    );
    let q = r
        .reports
        .quality
        .as_ref()
        .expect("a Code Quality comparison");
    assert_eq!(q.new.len(), 2);
    assert_eq!(q.new[0].file_path, "src/limiter.rs");
    // A free GitLab: no approval rules, trains or dependencies.
    assert!(r.premium.rules.is_none() && !r.premium.trains_enabled && r.premium.blocks.is_empty());
    assert!(!r.premium.duo, "no GitLab Duo on a free GitLab");

    // Its failed job, retried: a new try of it.
    let pid = r.mr.head_pipeline.as_ref().unwrap().id;
    let jobs = c.jobs(p, pid).unwrap();
    let failed = jobs
        .iter()
        .find(|j| j.status == "failed")
        .expect("a failed job");
    c.retry_job(p, failed.id).unwrap();
    let again = c.jobs(p, pid).unwrap();
    assert!(
        again
            .iter()
            .any(|j| j.name == failed.name && j.id != failed.id),
        "retried"
    );
}
