//! A local review end to end: a git repository's diff read as a merge
//! request, a comment on a line queueing an agent, the agent's reply.

use abromux_gitlab::{api::Client, diff::Body, load_review, local};
use std::{path::Path, process::Command, sync::Arc};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@x",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&ok.stderr)
    );
}

fn repo() -> tempdir::Dir {
    let d = tempdir::Dir::new();
    git(&d.0, &["init", "-q", "-b", "main"]);
    std::fs::write(d.0.join("a.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    std::fs::write(d.0.join("gone.txt"), "bye\n").unwrap();
    git(&d.0, &["add", "."]);
    git(&d.0, &["commit", "-q", "-m", "first"]);
    git(&d.0, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(d.0.join("a.rs"), "fn a() {}\nfn bee() {}\nfn c() {}\n").unwrap();
    git(&d.0, &["rm", "-q", "gone.txt"]);
    git(&d.0, &["commit", "-q", "-am", "Rename b"]);
    d
}

mod tempdir {
    pub struct Dir(pub std::path::PathBuf);
    impl Dir {
        pub fn new() -> Dir {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p =
                std::env::temp_dir().join(format!("abromux-local-test-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p.canonicalize().unwrap())
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// One store for the process (`init` takes the first): tests run together.
fn init() {
    local::init(
        std::env::temp_dir().join(format!("abromux-local-test-store-{}", std::process::id())),
    );
}

fn client(id: &str) -> Client {
    Client::new(
        &local::host_of(id),
        "",
        Arc::new(local::LocalGitLab::new(id)),
    )
}

#[test]
fn a_branch_diff_reviews_like_a_merge_request() {
    init();
    let r = repo();
    let id = local::create(
        &r.0,
        "main...feature",
        local::Open {
            agent: Some("claude --model x".into()),
            ..Default::default()
        },
    )
    .unwrap();
    // The same diff again: the same review.
    assert_eq!(
        local::create(&r.0, "main...feature", Default::default()).unwrap(),
        id
    );
    let c = client(&id);
    let rv = load_review(&c, local::PROJECT, local::IID).unwrap();
    // One commit: its title is the review's.
    assert_eq!(rv.mr.title, "Rename b");
    assert_eq!(rv.commits.len(), 1);
    let paths: Vec<(&str, &str)> = rv.files.iter().map(|f| (f.path(), f.status())).collect();
    assert_eq!(paths, [("a.rs", "modified"), ("gone.txt", "deleted")]);
    assert!(matches!(rv.files[0].body(), Body::Text(h) if h.len() == 1));

    // A thread on the added line, as the tab starts one.
    let refs = rv.mr.diff_refs.clone().unwrap();
    let d = c
        .line_comment(1, 1, &refs, "a.rs", "a.rs", None, Some(2), "Why bee?")
        .unwrap();
    let jobs = local::take_jobs(&id);
    assert_eq!(
        jobs,
        [local::AgentJob {
            review: id.clone(),
            threads: vec![d.id.clone()],
            reply: false
        }]
    );
    assert!(local::take_jobs(&id).is_empty());

    // The agent's prompt has the code and how to answer.
    let launch = local::launch(&jobs[0], "/x/abromux").unwrap();
    let cmd = launch.command.unwrap();
    assert!(cmd.starts_with("claude --model x \"$(cat '"), "{cmd}");
    let prompt = std::fs::read_to_string(&launch.prompt_file).unwrap();
    assert!(prompt.contains("+fn bee() {}   <-- this line"), "{prompt}");
    assert!(prompt.contains("> Why bee?"), "{prompt}");
    assert!(
        prompt.contains(&format!("'/x/abromux' diff reply {id} {} --resolve", d.id)),
        "{prompt}"
    );
    assert_eq!(launch.cwd, r.0.to_string_lossy());

    // It answers; the thread shows it, resolved, on its line.
    local::reply(&id, &d.id, "Because bees.", true, None).unwrap();
    let rv = load_review(&c, 1, 1).unwrap();
    let (_, _, t) = &rv.files[0].threads[0];
    assert_eq!(t.notes.len(), 2);
    assert_eq!(t.notes[1].author.name, "Claude");
    assert!(t.resolved());

    // A reply of yours asks again.
    c.reply(1, 1, &d.id, "And c?").unwrap();
    assert_eq!(
        local::take_jobs(&id),
        [local::AgentJob {
            review: id.clone(),
            threads: vec![d.id.clone()],
            reply: true
        }]
    );

    // A review's comments go to one agent when it's submitted.
    c.draft_line_comment(1, 1, &refs, "a.rs", "a.rs", Some(2), None, "one")
        .unwrap();
    c.comment(1, 1, "two").unwrap();
    let _ = local::take_jobs(&id);
    assert_eq!(c.draft_notes(1, 1).unwrap().len(), 1);
    c.publish_drafts(1, 1).unwrap();
    let jobs = local::take_jobs(&id);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].threads.len(), 1);
    let rv = load_review(&c, 1, 1).unwrap();
    assert!(rv.drafts.is_empty());
    assert_eq!(rv.general.len(), 1);
    // On the removed line's old side.
    assert_eq!(rv.files[0].threads.len(), 2);
}

#[test]
fn the_working_tree_with_untracked_files() {
    init();
    let r = repo();
    let id = local::create(&r.0, "", Default::default()).unwrap();
    let c = client(&id);
    let before = local::signature(&id);
    std::fs::write(r.0.join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(r.0.join("new.md"), "# hi\nthere").unwrap();
    assert_ne!(local::signature(&id), before);
    let rv = load_review(&c, 1, 1).unwrap();
    assert_eq!(rv.mr.title, "uncommitted changes");
    assert_eq!(rv.mr.diff_refs.as_ref().unwrap().head_sha, local::WORKTREE);
    let paths: Vec<(&str, &str)> = rv.files.iter().map(|f| (f.path(), f.status())).collect();
    assert_eq!(paths, [("a.rs", "modified"), ("new.md", "added")]);
    assert_eq!(rv.files[1].counts(), (2, 0));
    // Unchanged lines come from the working tree and from git.
    assert_eq!(
        c.raw_file(1, "a.rs", local::WORKTREE).unwrap(),
        "fn a() {}\n"
    );
    let base = &rv.mr.diff_refs.as_ref().unwrap().base_sha;
    assert_eq!(
        c.raw_file(1, "a.rs", base).unwrap(),
        "fn a() {}\nfn bee() {}\nfn c() {}\n"
    );
    assert!(
        c.raw_maybe(1, "../outside", local::WORKTREE)
            .unwrap()
            .is_none()
    );
    // A commit's own diff (the Commits tab).
    let commit = &rv.commits;
    assert!(commit.is_empty());
}

#[test]
fn sending_to_a_terminal_types_one_line() {
    init();
    let r = repo();
    let id = local::create(
        &r.0,
        "HEAD~1..HEAD",
        local::Open {
            send_to: Some(Some("surface:7".into())),
            ..Default::default()
        },
    )
    .unwrap();
    let c = client(&id);
    c.comment(1, 1, "Looks off").unwrap();
    let job = local::take_jobs(&id).remove(0);
    let l = local::launch(&job, "abromux").unwrap();
    assert_eq!(l.send_to.as_deref(), Some("surface:7"));
    assert!(l.command.is_none());
    assert!(!l.text.unwrap().contains('\n'));
}

#[test]
fn the_branchs_work_is_what_to_review_by_default() {
    init();
    let d = repo();
    // On `feature`, a commit past `main`: everything since the fork, with
    // what isn't committed yet.
    assert_eq!(local::work_spec(&d.0), "--merge-base main");
    std::fs::write(d.0.join("new.txt"), "hi\n").unwrap();
    let id = local::create(&d.0, &local::work_spec(&d.0), local::Open::default()).unwrap();
    let s = local::load(&id).unwrap();
    assert_eq!(s.describe(), "changes since main");
    let r = load_review(&client(&id), local::PROJECT, local::IID).unwrap();
    let paths: Vec<&str> = r.files.iter().map(|f| f.path()).collect();
    assert!(
        paths.contains(&"a.rs") && paths.contains(&"gone.txt") && paths.contains(&"new.txt"),
        "{paths:?}"
    );
    assert_eq!(r.commits.len(), 1);
    // On the default branch itself: the uncommitted changes.
    git(&d.0, &["checkout", "-q", "main"]);
    assert_eq!(local::work_spec(&d.0), "");
}
