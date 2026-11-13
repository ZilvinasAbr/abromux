//! Local reviews: any git diff (`X..Y`, `X...Y`, `X` against the working
//! tree, or the uncommitted changes) shown in the GitLab tab, as if it were a
//! merge request, with no GitLab at all.
//!
//! A local review is a [`Transport`] that answers the GitLab REST calls the
//! tab makes from `git` and from a small JSON file of comment threads (the
//! [`Store`]). The tab works on it unchanged: Changes, Commits, threads on
//! lines, replies, resolving, "Start a review".
//!
//! What's different is what a comment does: each new thread (or a review's
//! batch of them, or a reply) becomes a prompt for a coding agent, queued as
//! an [`AgentJob`]. The tab takes the jobs after the write and asks abromux to
//! start the agent in a terminal beside it (`abromux_plugin::Host::run_terminal`).
//! The agent answers in the thread with `abromux diff reply`, which writes
//! the store; the tab sees the file change and reloads, as it sees the diff
//! change when the agent edits the code.
//!
//! A review's host is `local://<id>`; its one "merge request" is project 1,
//! iid 1. The id comes from the repository and the diff asked for, so asking
//! for the same diff again finds the same threads.

use crate::api::{Response, Transport};
use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// The scheme of a local review's host.
pub const SCHEME: &str = "local://";
/// The working tree's stand-in for a commit sha.
pub const WORKTREE: &str = "WORKTREE";
/// The project and iid every local review answers as.
pub const PROJECT: u64 = 1;
pub const IID: u64 = 1;
/// The reviewer (you) and the agent, as GitLab users.
const ME_ID: u64 = 1;
const AGENT_ID: u64 = 2;
/// Files bigger than this aren't diffed (the tab says it's too large).
const TOO_LARGE: usize = 2 * 1024 * 1024;

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Keep local reviews in `dir` (abromux: `local-diffs/` in its support folder).
pub fn init(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// `$ABROMUX_LOCAL_DIFFS_DIR` (tests, a dev app) over `init`'s.
fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("ABROMUX_LOCAL_DIFFS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    DIR.get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("abromux-local-diffs"))
}

pub fn is_local(host: &str) -> bool {
    host.starts_with(SCHEME)
}

pub fn host_of(id: &str) -> String {
    format!("{SCHEME}{id}")
}

pub fn id_of(host: &str) -> Option<&str> {
    host.strip_prefix(SCHEME)
        .map(|h| h.trim_end_matches('/'))
        .filter(|h| !h.is_empty())
}

// ---- the store ----------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Person {
    pub id: u64,
    pub username: String,
    pub name: String,
}

/// Where a thread sits, as GitLab's `position`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Pos {
    pub base_sha: String,
    pub start_sha: String,
    pub head_sha: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub position_type: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
}

impl Pos {
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or_default()
    }
}

/// One note, in GitLab's shape (it's served as it's kept).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StoredNote {
    pub id: u64,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub body: String,
    pub author: Person,
    pub created_at: String,
    pub system: bool,
    pub resolvable: bool,
    pub resolved: bool,
    pub resolved_by: Option<Person>,
    pub resolved_at: Option<String>,
    pub position: Option<Pos>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Thread {
    pub id: String,
    pub individual_note: bool,
    pub notes: Vec<StoredNote>,
}

impl Thread {
    pub fn position(&self) -> Option<&Pos> {
        self.notes.first()?.position.as_ref()
    }

    pub fn resolved(&self) -> bool {
        self.notes.iter().any(|n| n.resolvable)
            && self
                .notes
                .iter()
                .filter(|n| n.resolvable)
                .all(|n| n.resolved)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Draft {
    pub id: u64,
    pub note: String,
    pub position: Option<Pos>,
}

/// A local review, as kept on disk: what to diff, who answers comments,
/// and the threads.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Store {
    pub id: String,
    /// The repository's top folder.
    pub repo: String,
    /// The diff as asked for (`main...feature`, `HEAD~3`, empty: uncommitted).
    pub spec: String,
    /// The old side, a revision.
    pub base: String,
    /// The new side, a revision; None: the working tree (untracked files too).
    pub head: Option<String>,
    /// `X...Y`: the old side is where they forked.
    pub merge_base: bool,
    pub title: Option<String>,
    /// The agent a comment starts: a command, the prompt appended (or put
    /// where `{prompt}` / `{prompt_file}` is).
    pub agent: String,
    /// Send comments to this terminal (`surface:3`) instead of starting agents.
    pub send_to: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub next_id: u64,
    pub threads: Vec<Thread>,
    pub drafts: Vec<Draft>,
    pub approved: bool,
}

impl Store {
    fn next(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn thread(&self, id: &str) -> Option<&Thread> {
        self.threads
            .iter()
            .find(|t| t.id == id || t.id.starts_with(id) && id.len() >= 4)
    }

    fn thread_mut(&mut self, id: &str) -> Option<&mut Thread> {
        let full = self.thread(id)?.id.clone();
        self.threads.iter_mut().find(|t| t.id == full)
    }

    /// How the diff reads: `git diff …`'s arguments.
    pub fn describe(&self) -> String {
        match (&self.head, self.merge_base) {
            (None, _) if self.base == "HEAD" => "uncommitted changes".into(),
            (None, true) => format!("changes since {}", self.base),
            (None, false) => format!("{} → working tree", self.base),
            (Some(h), true) => format!("{}...{h}", self.base),
            (Some(h), false) => format!("{}..{h}", self.base),
        }
    }

    fn head_label(&self) -> String {
        self.head.clone().unwrap_or_else(|| "working tree".into())
    }

    /// The agent's name, as its replies are signed: `claude` → Claude.
    pub fn agent_name(&self) -> String {
        let word = self.agent.split_whitespace().next().unwrap_or("agent");
        let word = word.rsplit('/').next().unwrap_or(word);
        let mut c = word.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => "Agent".into(),
        }
    }
}

pub fn path(id: &str) -> PathBuf {
    dir().join(format!("{id}.json"))
}

/// The prompts written for the agents of review `id`.
fn prompt_dir(id: &str) -> PathBuf {
    dir().join(id)
}

pub fn load(id: &str) -> anyhow::Result<Store> {
    let p = path(id);
    let bytes =
        std::fs::read(&p).with_context(|| format!("no local review {id} ({})", p.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("reading {}", p.display()))
}

fn save(s: &Store) -> anyhow::Result<()> {
    let p = path(&s.id);
    std::fs::create_dir_all(dir())?;
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// Change review `id` under its lock (the app and `abromux diff reply` both write it).
pub fn update<T>(id: &str, f: impl FnOnce(&mut Store) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let _lock = Lock::take(id)?;
    let mut s = load(id)?;
    let out = f(&mut s)?;
    s.updated_at = now_iso();
    save(&s)?;
    Ok(out)
}

/// A lock file beside the store, taken by creating it.
struct Lock(PathBuf);

impl Lock {
    fn take(id: &str) -> anyhow::Result<Lock> {
        std::fs::create_dir_all(dir())?;
        let p = dir().join(format!("{id}.lock"));
        let started = Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p)
            {
                Ok(_) => return Ok(Lock(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // A writer that died leaves it: a stale one is taken over.
                    let stale = std::fs::metadata(&p)
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t.elapsed().unwrap_or_default() > Duration::from_secs(10));
                    if stale || started.elapsed() > Duration::from_secs(5) {
                        let _ = std::fs::remove_file(&p);
                        continue;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e).context("locking the local review"),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What `abromux diff` was asked to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub base: String,
    pub head: Option<String>,
    pub merge_base: bool,
}

/// `X...Y`, `X..Y`, `X` (against the working tree), or nothing (the
/// uncommitted changes, against HEAD). An empty side is HEAD, as git has it.
/// `--merge-base X`, as `git diff` takes it: the working tree against where
/// HEAD forked from X, so a branch's commits and what isn't committed yet.
pub fn parse_spec(spec: &str) -> Spec {
    let spec = spec.trim();
    let side = |s: &str| {
        if s.is_empty() {
            "HEAD".to_string()
        } else {
            s.to_string()
        }
    };
    if let Some(base) = spec.strip_prefix("--merge-base") {
        return Spec {
            base: side(base.trim()),
            head: None,
            merge_base: true,
        };
    }
    if let Some((a, b)) = spec.split_once("...") {
        return Spec {
            base: side(a),
            head: Some(side(b)),
            merge_base: true,
        };
    }
    if let Some((a, b)) = spec.split_once("..") {
        return Spec {
            base: side(a),
            head: Some(side(b)),
            merge_base: false,
        };
    }
    Spec {
        base: side(spec),
        head: None,
        merge_base: false,
    }
}

/// FNV-1a, for ids that stay the same from run to run.
fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

/// What a new or reopened local review takes.
#[derive(Clone, Debug, Default)]
pub struct Open {
    pub title: Option<String>,
    pub agent: Option<String>,
    /// Some(None): stop sending to a terminal.
    pub send_to: Option<Option<String>>,
}

/// The local review of `spec` in the repository at `cwd`: made, or found
/// again with its threads (taking what `open` changes). Gives its id.
pub fn create(cwd: &Path, spec: &str, open: Open) -> anyhow::Result<String> {
    let top = git(cwd, &["rev-parse", "--show-toplevel"]).context("not in a git repository")?;
    let repo = String::from_utf8_lossy(&top).trim().to_string();
    let parsed = parse_spec(spec);
    // Check the revisions now: a typo fails here, not in the tab.
    for rev in std::iter::once(&parsed.base).chain(&parsed.head) {
        git(
            Path::new(&repo),
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{rev}^{{commit}}"),
            ],
        )
        .with_context(|| format!("{rev:?} isn't a commit in {repo}"))?;
    }
    let id = format!(
        "{:012x}",
        hash(&format!(
            "{repo}\0{}\0{:?}\0{}",
            parsed.base, parsed.head, parsed.merge_base
        )) & 0xffff_ffff_ffff
    );
    let _lock = Lock::take(&id)?;
    let mut s = load(&id).unwrap_or_else(|_| Store {
        id: id.clone(),
        repo: repo.clone(),
        created_at: now_iso(),
        agent: default_agent(),
        ..Default::default()
    });
    s.spec = spec.trim().to_string();
    (s.base, s.head, s.merge_base) = (parsed.base, parsed.head, parsed.merge_base);
    if open.title.is_some() {
        s.title = open.title;
    }
    if let Some(a) = open.agent.filter(|a| !a.trim().is_empty()) {
        s.agent = a;
    }
    if let Some(t) = open.send_to {
        s.send_to = t;
    }
    s.updated_at = now_iso();
    save(&s)?;
    Ok(id)
}

/// What to review in the repository at `cwd` when nothing says: the
/// branch's work, committed or not, since it forked from the default branch
/// (`--merge-base origin/main`); on the default branch itself, or without
/// one to be found, the uncommitted changes (the empty spec).
pub fn work_spec(cwd: &Path) -> String {
    let rev = |r: &str| {
        git_text(
            cwd,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{r}^{{commit}}"),
            ],
        )
        .ok()
    };
    let default = git_text(
        cwd,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .filter(|b| !b.is_empty() && rev(b).is_some())
    .or_else(|| {
        ["origin/main", "origin/master", "main", "master"]
            .into_iter()
            .find(|b| rev(b).is_some())
            .map(str::to_string)
    });
    let Some(base) = default else {
        return String::new();
    };
    let (Some(head), Ok(fork)) = (rev("HEAD"), git_text(cwd, &["merge-base", &base, "HEAD"]))
    else {
        return String::new();
    };
    // Nothing committed since the fork: the uncommitted changes say it all.
    if fork == head {
        String::new()
    } else {
        format!("--merge-base {base}")
    }
}

/// `$ABROMUX_DIFF_AGENT`, else Claude Code.
pub fn default_agent() -> String {
    std::env::var("ABROMUX_DIFF_AGENT")
        .ok()
        .filter(|a| !a.trim().is_empty())
        .unwrap_or_else(|| "claude".into())
}

/// Every local review on disk, newest first.
pub fn all() -> Vec<Store> {
    let mut out: Vec<Store> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    out.sort_by(|a: &Store, b: &Store| b.updated_at.cmp(&a.updated_at));
    out
}

/// Review `id` found by a prefix of it, as `abromux diff` prints ids.
pub fn find(id: &str) -> anyhow::Result<String> {
    if path(id).exists() {
        return Ok(id.to_string());
    }
    let hits: Vec<String> = all()
        .into_iter()
        .map(|s| s.id)
        .filter(|s| s.starts_with(id))
        .collect();
    match hits.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no local review {id} (`abromux diff list`)"),
        _ => bail!("{id} could be any of {}", hits.join(", ")),
    }
}

/// The agent's answer in thread `thread`: a reply, and with `resolve` the
/// thread resolved. `as_name` signs it (else the agent's name).
pub fn reply(
    id: &str,
    thread: &str,
    body: &str,
    resolve: bool,
    as_name: Option<&str>,
) -> anyhow::Result<String> {
    update(id, |s| {
        let agent = Person {
            id: AGENT_ID,
            username: "agent".into(),
            name: as_name
                .map(str::to_string)
                .unwrap_or_else(|| s.agent_name()),
        };
        let note_id = s.next();
        let t = s
            .thread_mut(thread)
            .with_context(|| format!("no thread {thread} in this review"))?;
        let resolvable = t.notes.first().is_some_and(|n| n.resolvable);
        let body = body.trim();
        if !body.is_empty() {
            t.notes.push(StoredNote {
                id: note_id,
                kind: Some("DiscussionNote".into()),
                body: body.to_string(),
                author: agent.clone(),
                created_at: now_iso(),
                resolvable,
                ..Default::default()
            });
        }
        if resolve {
            set_resolved(t, true, &agent);
        }
        Ok(t.id.clone())
    })
}

fn set_resolved(t: &mut Thread, resolved: bool, by: &Person) {
    let at = now_iso();
    for n in t.notes.iter_mut().filter(|n| n.resolvable) {
        n.resolved = resolved;
        n.resolved_by = resolved.then(|| by.clone());
        n.resolved_at = resolved.then(|| at.clone());
    }
}

// ---- agents ---------------------------------------------------------------------

/// Comments that want an agent: new threads (several, from a submitted
/// review), or a reply in one.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentJob {
    pub review: String,
    pub threads: Vec<String>,
    pub reply: bool,
}

static JOBS: Mutex<Vec<AgentJob>> = Mutex::new(Vec::new());

fn queue(job: AgentJob) {
    JOBS.lock().unwrap_or_else(|e| e.into_inner()).push(job);
}

/// The jobs of review `id` not yet taken.
pub fn take_jobs(id: &str) -> Vec<AgentJob> {
    let mut all = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let (mine, rest): (Vec<_>, Vec<_>) = all.drain(..).partition(|j| j.review == id);
    *all = rest;
    mine
}

/// How to run a job: start `command` in a terminal in `cwd`, or type `text`
/// into terminal `send_to`.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub cwd: String,
    pub command: Option<String>,
    pub send_to: Option<String>,
    pub text: Option<String>,
    pub title: String,
    pub prompt_file: PathBuf,
}

/// The prompt for `job`, written to a file, and how to hand it to the agent.
/// `exe`: the abromux to reply with.
pub fn launch(job: &AgentJob, exe: &str) -> anyhow::Result<Launch> {
    let s = load(&job.review)?;
    let prompt = prompt(&s, job, exe)?;
    let dir = prompt_dir(&s.id);
    std::fs::create_dir_all(&dir)?;
    let n = std::fs::read_dir(&dir).map_or(0, |d| d.count()) + 1;
    let file = dir.join(format!("prompt-{n}.md"));
    std::fs::write(&file, &prompt)?;
    let quoted = shell_quote(&file.to_string_lossy());
    let what = if job.threads.len() == 1 {
        "a review comment"
    } else {
        "review comments"
    };
    let title = format!(
        "{} · {}",
        s.agent_name(),
        s.title.clone().unwrap_or_else(|| s.describe())
    );
    if let Some(t) = &s.send_to {
        // One line: a newline would send it half-typed.
        let text = format!(
            "Please handle {what} on the local diff: read {} and do what it says.",
            file.display()
        );
        return Ok(Launch {
            cwd: s.repo.clone(),
            command: None,
            send_to: Some(t.clone()),
            text: Some(text),
            title,
            prompt_file: file,
        });
    }
    let arg = format!("\"$(cat {quoted})\"");
    let command = if s.agent.contains("{prompt_file}") {
        s.agent.replace("{prompt_file}", &quoted)
    } else if s.agent.contains("{prompt}") {
        s.agent.replace("{prompt}", &arg)
    } else {
        format!("{} {arg}", s.agent)
    };
    Ok(Launch {
        cwd: s.repo.clone(),
        command: Some(command),
        send_to: None,
        text: None,
        title,
        prompt_file: file,
    })
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// What the agent is told: where the diff is, each thread with the code
/// it's on, and how to answer in it.
pub fn prompt(s: &Store, job: &AgentJob, exe: &str) -> anyhow::Result<String> {
    let r = resolve(s)?;
    let files = diffs(s, &r).unwrap_or_default();
    let mut out = String::new();
    let many = job.threads.len() > 1;
    out.push_str(&format!(
        "{} left {} on a diff they're reviewing in AbroMux.\n\n",
        me().name,
        if many {
            "review comments"
        } else if job.reply {
            "a reply to a review thread"
        } else {
            "a review comment"
        }
    ));
    out.push_str(&format!("- Repository: {}\n", s.repo));
    let cmd = match &s.head {
        None => format!(
            "git diff {} (the working tree, untracked files too)",
            short(&r.base_sha)
        ),
        Some(_) => format!("git diff {}..{}", short(&r.base_sha), short(&r.head_sha)),
    };
    out.push_str(&format!("- The diff: {} — `{cmd}`\n", s.describe()));
    if let Some(t) = &s.title {
        out.push_str(&format!("- Titled: {t}\n"));
    }
    out.push('\n');
    for (i, tid) in job.threads.iter().enumerate() {
        let Some(t) = s.thread(tid) else { continue };
        let heading = match t.position() {
            Some(p) => {
                let line = p
                    .new_line
                    .map(|n| format!("line {n}"))
                    .or(p.old_line.map(|o| format!("removed line {o} (old side)")));
                format!("on `{}`, {}", p.path(), line.unwrap_or_default())
            }
            None => "on the whole diff".into(),
        };
        out.push_str(&format!(
            "## {}Thread {} {heading}\n\n",
            if many {
                format!("{}. ", i + 1)
            } else {
                String::new()
            },
            t.id
        ));
        if let Some(p) = t.position()
            && let Some(code) = excerpt(&files, p)
        {
            out.push_str(&format!("```diff\n{code}```\n\n"));
        }
        for n in &t.notes {
            let quoted: String = n.body.lines().map(|l| format!("> {l}\n")).collect();
            out.push_str(&format!("**{}**:\n{quoted}\n", n.author.name));
        }
    }
    let repo = &s.repo;
    out.push_str("## What to do\n\n");
    out.push_str(&format!(
        "- If a comment asks for a change, make it in {repo}{}. Keep to what's asked.\n",
        if s.head.is_none() {
            " (in the working tree; don't commit unless asked)"
        } else {
            ""
        }
    ));
    out.push_str("- If it's a question, answer it; don't change code unless asked.\n");
    out.push_str("- Then answer in its thread, so the reviewer sees it beside the code:\n\n");
    let first = job
        .threads
        .first()
        .map(String::as_str)
        .unwrap_or("<thread>");
    out.push_str(&format!(
        "```sh\n{} diff reply {} {first} --resolve <<'EOF'\nWhat you changed, or the answer.\nEOF\n```\n\n",
        shell_quote(exe),
        s.id
    ));
    if many {
        out.push_str("  Each thread gets its own reply (its id is in its heading).\n");
    }
    out.push_str("  `--resolve` only when you made the change asked for; leave it off for an answer or a question back.\n");
    Ok(out)
}

/// The lines of the diff up to the one `p` is on, and a few after.
fn excerpt(files: &[Value], p: &Pos) -> Option<String> {
    let f = files.iter().find(|f| {
        (p.new_path.is_some() && f["new_path"].as_str() == p.new_path.as_deref())
            || (p.old_path.is_some() && f["old_path"].as_str() == p.old_path.as_deref())
    })?;
    let hunks = crate::diff::parse(f["diff"].as_str()?);
    for h in &hunks {
        let at = h.lines.iter().position(|l| match (p.old_line, p.new_line) {
            (_, Some(n)) => l.new == Some(n),
            (Some(o), None) => l.old == Some(o) && l.new.is_none(),
            _ => false,
        });
        let Some(at) = at else { continue };
        let from = at.saturating_sub(8);
        let to = (at + 4).min(h.lines.len());
        let mut out = String::new();
        for (i, l) in h.lines[from..to].iter().enumerate() {
            let sign = match l.kind {
                crate::diff::Kind::Added => '+',
                crate::diff::Kind::Removed => '-',
                crate::diff::Kind::Context => ' ',
                crate::diff::Kind::NoNewline => continue,
            };
            let mark = if from + i == at {
                "   <-- this line"
            } else {
                ""
            };
            out.push_str(&format!("{sign}{}{mark}\n", l.text));
        }
        return Some(out);
    }
    None
}

fn short(sha: &str) -> &str {
    sha.get(..10).unwrap_or(sha)
}

// ---- git ---------------------------------------------------------------------------

pub fn git(repo: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    abromux_plugin::git::run(repo, args).map_err(|e| anyhow::anyhow!("git {}: {e}", args.join(" ")))
}

fn git_text(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    Ok(String::from_utf8_lossy(&git(repo, args)?)
        .trim()
        .to_string())
}

/// The diff's two sides, as shas (the new one `WORKTREE` for the working tree).
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub base_sha: String,
    pub head_sha: String,
}

pub fn resolve(s: &Store) -> anyhow::Result<Resolved> {
    let repo = Path::new(&s.repo);
    let commit = |rev: &str| {
        git_text(
            repo,
            &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
        )
    };
    let head_sha = match &s.head {
        Some(h) => commit(h)?,
        None => WORKTREE.to_string(),
    };
    let base_sha = if s.merge_base {
        let h = if head_sha == WORKTREE {
            "HEAD".to_string()
        } else {
            head_sha.clone()
        };
        git_text(repo, &["merge-base", &s.base, &h])?
    } else {
        commit(&s.base)?
    };
    Ok(Resolved { base_sha, head_sha })
}

/// The diff as GitLab's per-file `diffs`.
pub fn diffs(s: &Store, r: &Resolved) -> anyhow::Result<Vec<Value>> {
    diff_between(Path::new(&s.repo), &r.base_sha, &r.head_sha)
}

/// `git diff from to` (to `WORKTREE`: the working tree, untracked files too).
fn diff_between(repo: &Path, from: &str, to: &str) -> anyhow::Result<Vec<Value>> {
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "-M",
        "--full-index",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        from,
    ];
    if to != WORKTREE {
        args.push(to);
    }
    let out = git(repo, &args)?;
    let mut files = parse_git_diff(&String::from_utf8_lossy(&out));
    if to == WORKTREE {
        let others = git(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for p in others.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            let p = String::from_utf8_lossy(p).into_owned();
            files.push(untracked(repo, &p));
        }
    }
    Ok(files)
}

/// A file git doesn't track yet, as an added file.
fn untracked(repo: &Path, path: &str) -> Value {
    let bytes = std::fs::read(repo.join(path)).unwrap_or_default();
    let mut f = json!({ "old_path": path, "new_path": path, "a_mode": "0", "b_mode": "100644", "new_file": true, "diff": "" });
    if bytes.len() > TOO_LARGE {
        f["too_large"] = json!(true);
    } else if bytes[..bytes.len().min(8000)].contains(&0) {
        f["diff"] = json!(format!("Binary files /dev/null and b/{path} differ\n"));
    } else if !bytes.is_empty() {
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let mut d = format!("@@ -0,0 +1,{} @@\n", lines.len());
        for l in &lines {
            d.push('+');
            d.push_str(l);
        }
        if !text.ends_with('\n') {
            d.push_str("\n\\ No newline at end of file\n");
        }
        f["diff"] = json!(d);
    }
    f
}

/// `"a/x y"` → `x y`, undoing git's C-style quoting of odd names.
fn unquote(p: &str) -> String {
    let Some(inner) = p.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
        return p.to_string();
    };
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

fn side(p: &str, prefix: &str) -> Option<String> {
    let p = unquote(p.trim_end_matches('\t'));
    if p == "/dev/null" {
        return None;
    }
    Some(p.strip_prefix(prefix).unwrap_or(&p).to_string())
}

/// `git diff`'s output, one GitLab `FileDiff` per file.
pub fn parse_git_diff(text: &str) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    let mut body: Option<String> = None;
    let finish = |out: &mut Vec<Value>, body: &mut Option<String>| {
        if let (Some(f), Some(b)) = (out.last_mut(), body.take()) {
            if b.len() > TOO_LARGE {
                f["too_large"] = json!(true);
            } else {
                f["diff"] = json!(b);
            }
        }
    };
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n');
        if let Some(rest) = bare.strip_prefix("diff --git ") {
            finish(&mut out, &mut body);
            // `a/x b/x`: both halves the same unless renamed (then the
            // `rename` lines say), so the middle is the split.
            let (old, new) = match rest.len() % 2 {
                1 if rest[..rest.len() / 2].strip_prefix("a/")
                    == rest[rest.len() / 2 + 1..].strip_prefix("b/") =>
                {
                    let p = rest[2..rest.len() / 2].to_string();
                    (p.clone(), p)
                }
                _ => match rest.split_once(" b/") {
                    Some((a, b)) => (
                        unquote(a).trim_start_matches("a/").to_string(),
                        b.to_string(),
                    ),
                    None => (rest.to_string(), rest.to_string()),
                },
            };
            out.push(
                json!({ "old_path": old, "new_path": new, "a_mode": "", "b_mode": "", "diff": "" }),
            );
            continue;
        }
        let Some(f) = out.last_mut() else { continue };
        if let Some(b) = body.as_mut() {
            b.push_str(line);
            continue;
        }
        if bare.starts_with("@@") {
            body = Some(line.to_string());
        } else if let Some(m) = bare.strip_prefix("new file mode ") {
            f["new_file"] = json!(true);
            f["b_mode"] = json!(m);
        } else if let Some(m) = bare.strip_prefix("deleted file mode ") {
            f["deleted_file"] = json!(true);
            f["a_mode"] = json!(m);
        } else if let Some(m) = bare.strip_prefix("old mode ") {
            f["a_mode"] = json!(m);
        } else if let Some(m) = bare.strip_prefix("new mode ") {
            f["b_mode"] = json!(m);
        } else if let Some(p) = bare.strip_prefix("rename from ") {
            f["renamed_file"] = json!(true);
            f["old_path"] = json!(unquote(p));
        } else if let Some(p) = bare.strip_prefix("rename to ") {
            f["renamed_file"] = json!(true);
            f["new_path"] = json!(unquote(p));
        } else if let Some(p) = bare.strip_prefix("--- ") {
            if let Some(p) = side(p, "a/") {
                f["old_path"] = json!(p);
            }
        } else if let Some(p) = bare.strip_prefix("+++ ") {
            if let Some(p) = side(p, "b/") {
                f["new_path"] = json!(p);
            }
        } else if bare.starts_with("Binary files ") {
            f["diff"] = json!(line);
        } else if let Some(m) = bare
            .strip_prefix("index ")
            .and_then(|i| i.split_whitespace().nth(1))
            && f["a_mode"] == ""
        {
            f["a_mode"] = json!(m);
            f["b_mode"] = json!(m);
        }
    }
    finish(&mut out, &mut body);
    out
}

/// The commits on the new side and not the old, newest first.
fn commits(s: &Store, r: &Resolved) -> anyhow::Result<Vec<Value>> {
    let head = if r.head_sha == WORKTREE {
        "HEAD"
    } else {
        &r.head_sha
    };
    let range = format!("{}..{head}", r.base_sha);
    let out = git(
        Path::new(&s.repo),
        &[
            "log",
            "--max-count=500",
            "--format=%H%x1f%h%x1f%s%x1f%B%x1f%an%x1f%ae%x1f%aI%x1f%cI%x1e",
            &range,
        ],
    )?;
    Ok(String::from_utf8_lossy(&out)
        .split('\x1e')
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim_start_matches('\n').split('\x1f').collect();
            (f.len() >= 8).then(|| {
                json!({
                    "id": f[0], "short_id": f[1], "title": f[2], "message": f[3].trim_end(),
                    "author_name": f[4], "author_email": f[5], "authored_date": f[6], "created_at": f[7],
                })
            })
        })
        .collect())
}

/// A file's bytes at `sha` (the working tree's for `WORKTREE`); None: not there.
fn raw(repo: &Path, path: &str, sha: &str) -> Option<Vec<u8>> {
    if sha == WORKTREE {
        let p = repo.join(path);
        // Only inside the repository.
        let inside = p
            .canonicalize()
            .ok()?
            .starts_with(repo.canonicalize().ok()?);
        return inside.then(|| std::fs::read(p).ok()).flatten();
    }
    git(repo, &["show", &format!("{sha}:{path}")]).ok()
}

/// Something that changes when the review would look different: the store,
/// the revisions, and (diffing the working tree) the files changed.
pub fn signature(id: &str) -> String {
    let Ok(s) = load(id) else {
        return String::new();
    };
    let stamp = |p: &Path| {
        std::fs::metadata(p).ok().map(|m| {
            (
                m.len(),
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos()),
            )
        })
    };
    let mut sig = format!("{:?}", stamp(&path(id)));
    if let Ok(r) = resolve(&s) {
        sig.push_str(&format!("|{}|{}", r.base_sha, r.head_sha));
    }
    if s.head.is_none() {
        let repo = Path::new(&s.repo);
        if let Ok(st) = git(
            repo,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        ) {
            let st = String::from_utf8_lossy(&st).into_owned();
            sig.push_str(&format!("|{}", hash(&st)));
            for entry in st.split('\0').filter(|e| e.len() > 3) {
                sig.push_str(&format!("|{:?}", stamp(&repo.join(&entry[3..]))));
            }
        }
    }
    sig
}

// ---- the transport --------------------------------------------------------------------

fn me() -> Person {
    static ME: OnceLock<Person> = OnceLock::new();
    ME.get_or_init(|| {
        let username = std::env::var("USER").unwrap_or_else(|_| "you".into());
        let name = abromux_plugin::git::text(Path::new("."), &["config", "user.name"])
            .ok()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| username.clone());
        Person {
            id: ME_ID,
            username,
            name,
        }
    })
    .clone()
}

pub fn now_iso() -> String {
    crate::time::iso(crate::time::now())
}

/// What a load asks several times (each answer of `load_review` resolves
/// the revisions and diffs): kept for a moment.
struct Snapshot {
    at: Instant,
    store: Store,
    resolved: Resolved,
    diffs: Vec<Value>,
    commits: Vec<Value>,
}

/// GitLab, answered from git and the store of review `id`.
pub struct LocalGitLab {
    id: String,
    snap: Mutex<Option<Snapshot>>,
}

impl LocalGitLab {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            snap: Mutex::new(None),
        }
    }

    fn snapshot<T>(&self, f: impl FnOnce(&Snapshot) -> T) -> anyhow::Result<T> {
        let mut snap = self.snap.lock().unwrap_or_else(|e| e.into_inner());
        let store = load(&self.id)?;
        let fresh = snap
            .as_ref()
            .is_some_and(|s| s.at.elapsed() < Duration::from_secs(2) && s.store == store);
        if !fresh {
            let resolved = resolve(&store)?;
            let diffs = diffs(&store, &resolved)?;
            let commits = commits(&store, &resolved).unwrap_or_default();
            *snap = Some(Snapshot {
                at: Instant::now(),
                store,
                resolved,
                diffs,
                commits,
            });
        }
        Ok(f(snap.as_ref().unwrap()))
    }

    fn forget(&self) {
        *self.snap.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn mr(&self, s: &Snapshot) -> Value {
        let st = &s.store;
        let repo_name = Path::new(&st.repo)
            .file_name()
            .map_or("repo".into(), |n| n.to_string_lossy().into_owned());
        let unresolved = st
            .threads
            .iter()
            .filter(|t| t.notes.iter().any(|n| n.resolvable) && !t.resolved())
            .count();
        let notes: usize = st.threads.iter().map(|t| t.notes.len()).sum();
        let title = st
            .title
            .clone()
            .unwrap_or_else(|| match (&st.head, s.commits.len()) {
                (Some(_), 1) => s.commits[0]["title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                _ => st.describe(),
            });
        let cmd = match &st.head {
            None if st.merge_base => format!("git diff --merge-base {}", st.base),
            None => format!("git diff {}", st.base),
            Some(h) => format!(
                "git diff {}{}{h}",
                st.base,
                if st.merge_base { "..." } else { ".." }
            ),
        };
        let agent = match &st.send_to {
            Some(t) => format!("sent to the agent in `{t}`"),
            None => format!("sent to a new `{}` in a terminal beside this tab", st.agent),
        };
        let description = format!(
            "A local diff of `{}`: `{cmd}`.\n\nComments here don't go to GitLab: each new thread (or a submitted review's threads, or a reply) is {agent}, which makes the change or answers, and replies in the thread.",
            st.repo
        );
        json!({
            "id": 1, "iid": IID, "project_id": PROJECT,
            "title": title,
            "description": description,
            "state": "opened",
            "draft": false,
            "author": me(),
            "assignees": [], "reviewers": [], "labels": [],
            "source_branch": st.head_label(),
            "target_branch": st.base,
            "references": { "full": format!("local/{repo_name}!{IID}") },
            "web_url": "",
            "created_at": st.created_at,
            "updated_at": st.updated_at,
            "user_notes_count": notes,
            "detailed_merge_status": if unresolved > 0 { "discussions_not_resolved" } else { "mergeable" },
            "diff_refs": { "base_sha": s.resolved.base_sha, "start_sha": s.resolved.base_sha, "head_sha": s.resolved.head_sha },
            "diverged_commits_count": 0,
            "time_stats": {},
        })
    }

    fn project(&self, s: &Snapshot) -> Value {
        let name = Path::new(&s.store.repo)
            .file_name()
            .map_or("repo".into(), |n| n.to_string_lossy().into_owned());
        json!({
            "id": PROJECT, "name": name, "name_with_namespace": format!("local / {name}"),
            "path_with_namespace": format!("local/{name}"), "default_branch": s.store.base, "web_url": "",
        })
    }

    fn member() -> Value {
        let m = me();
        json!({ "id": m.id, "username": m.username, "name": m.name, "access_level": 50, "state": "active" })
    }

    /// GET `path` (after `/api/v4/`, query kept).
    fn answer(&self, path: &str) -> anyhow::Result<Answer> {
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let q = |k: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='))
                .map(decode)
        };
        // Lists come whole on their first page.
        if q("page")
            .and_then(|p| p.parse::<u32>().ok())
            .is_some_and(|p| p > 1)
        {
            return Ok(Answer::List(vec![]));
        }
        let parts: Vec<&str> = route.split('/').collect();
        Ok(match parts.as_slice() {
            ["version"] => Answer::One(json!({ "version": "local", "revision": "" })),
            ["user"] => Answer::One(json!(me())),
            ["merge_requests"] | ["projects", _, "merge_requests"] => Answer::List(vec![self.snapshot(|s| self.mr(s))?]),
            ["projects"] => Answer::List(vec![self.snapshot(|s| self.project(s))?]),
            ["projects", _] => Answer::One(self.snapshot(|s| self.project(s))?),
            ["projects", _, "members", "all"] => Answer::List(vec![Self::member()]),
            ["projects", _, "members", "all", id] if id.parse() == Ok(ME_ID) => Answer::One(Self::member()),
            ["projects", _, "labels"] | ["projects", _, "milestones"] | ["projects", _, "pipelines"] => Answer::List(vec![]),
            ["projects", _, "merge_requests", _] => Answer::One(self.snapshot(|s| self.mr(s))?),
            ["projects", _, "merge_requests", _, what] => match *what {
                "diffs" => Answer::List(self.snapshot(|s| s.diffs.clone())?),
                "commits" => Answer::List(self.snapshot(|s| s.commits.clone())?),
                "discussions" => Answer::List(load(&self.id)?.threads.iter().map(|t| json!(t)).collect()),
                "draft_notes" => Answer::List(load(&self.id)?.drafts.iter().map(|d| json!(d)).collect()),
                "approvals" => {
                    let approved = load(&self.id)?.approved;
                    let by: Vec<Value> = if approved { vec![json!({ "user": me() })] } else { vec![] };
                    Answer::One(json!({ "approved": approved, "approved_by": by }))
                }
                "versions" => Answer::List(vec![self.snapshot(|s| {
                    json!({
                        "id": 1, "head_commit_sha": s.resolved.head_sha, "base_commit_sha": s.resolved.base_sha,
                        "start_commit_sha": s.resolved.base_sha, "created_at": s.store.updated_at,
                    })
                })?]),
                "award_emoji" | "closes_issues" | "reviewers" | "pipelines" | "resource_state_events" | "resource_label_events"
                | "resource_milestone_events" => Answer::List(vec![]),
                _ => Answer::Missing,
            },
            ["projects", _, "merge_requests", _, "versions", _] => Answer::One(json!({ "diffs": self.snapshot(|s| s.diffs.clone())? })),
            ["projects", _, "repository", "commits", sha, "diff"] => {
                let s = load(&self.id)?;
                let repo = Path::new(&s.repo);
                let sha = decode(sha);
                let parent = git_text(repo, &["rev-parse", "--verify", "--quiet", &format!("{sha}^")])
                    // The first commit: against the empty tree.
                    .unwrap_or_else(|_| "4b825dc642cb6eb9a060e54bf8d69288fbee4904".into());
                Answer::List(diff_between(repo, &parent, &sha)?)
            }
            ["projects", _, "repository", "compare"] => {
                let s = load(&self.id)?;
                let (from, to) = (q("from").unwrap_or_default(), q("to").unwrap_or_default());
                Answer::One(json!({ "diffs": diff_between(Path::new(&s.repo), &from, &to)? }))
            }
            ["projects", _, "repository", "files", file, "raw"] => {
                let s = load(&self.id)?;
                match raw(Path::new(&s.repo), &decode(file), &q("ref").unwrap_or_default()) {
                    Some(b) => Answer::Bytes(b),
                    None => Answer::Missing,
                }
            }
            _ => Answer::Missing,
        })
    }

    /// POST, PUT or DELETE `path` with `body`.
    fn write(&self, method: &str, path: &str, body: &Value) -> anyhow::Result<Answer> {
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let parts: Vec<&str> = route.split('/').collect();
        let text = |k: &str| body[k].as_str().unwrap_or_default().trim().to_string();
        let position = || {
            serde_json::from_value::<Pos>(body["position"].clone())
                .ok()
                .filter(|_| body["position"].is_object())
        };
        let id = self.id.clone();
        self.forget();
        let out = match (method, parts.as_slice()) {
            // A comment on the whole diff: a thread, so the agent can answer in it.
            ("POST", ["projects", _, "merge_requests", _, "notes"]) => {
                let t = update(&id, |s| Ok(new_thread(s, text("body"), None)))?;
                queue(AgentJob {
                    review: id,
                    threads: vec![t.id.clone()],
                    reply: false,
                });
                Answer::One(json!(t.notes[0]))
            }
            ("POST", ["projects", _, "merge_requests", _, "discussions"]) => {
                let t = update(&id, |s| Ok(new_thread(s, text("body"), position())))?;
                queue(AgentJob {
                    review: id,
                    threads: vec![t.id.clone()],
                    reply: false,
                });
                Answer::One(json!(t))
            }
            (
                "POST",
                [
                    "projects",
                    _,
                    "merge_requests",
                    _,
                    "discussions",
                    thread,
                    "notes",
                ],
            ) => {
                let thread = decode(thread);
                let note = update(&id, |s| {
                    let n = s.next();
                    let t = s.thread_mut(&thread).context("no such thread")?;
                    let note = StoredNote {
                        id: n,
                        kind: Some("DiscussionNote".into()),
                        body: text("body"),
                        author: me(),
                        created_at: now_iso(),
                        resolvable: t.notes.first().is_some_and(|n| n.resolvable),
                        ..Default::default()
                    };
                    t.notes.push(note.clone());
                    Ok(note)
                })?;
                queue(AgentJob {
                    review: id,
                    threads: vec![thread],
                    reply: true,
                });
                Answer::One(json!(note))
            }
            ("PUT", ["projects", _, "merge_requests", _, "discussions", thread]) => {
                let resolved = query.contains("resolved=true") || body["resolved"] == json!(true);
                let thread = decode(thread);
                let t = update(&id, |s| {
                    let t = s.thread_mut(&thread).context("no such thread")?;
                    set_resolved(t, resolved, &me());
                    Ok(t.clone())
                })?;
                Answer::One(json!(t))
            }
            ("POST", ["projects", _, "merge_requests", _, "draft_notes"]) => {
                let d = update(&id, |s| {
                    let d = Draft {
                        id: s.next(),
                        note: text("note"),
                        position: position(),
                    };
                    s.drafts.push(d.clone());
                    Ok(d)
                })?;
                Answer::One(json!(d))
            }
            ("DELETE", ["projects", _, "merge_requests", _, "draft_notes", d]) => {
                let d: u64 = d.parse().unwrap_or(0);
                update(&id, |s| {
                    s.drafts.retain(|x| x.id != d);
                    Ok(())
                })?;
                Answer::One(Value::Null)
            }
            // Submitting a review: its comments become threads, for one agent.
            (
                "POST",
                [
                    "projects",
                    _,
                    "merge_requests",
                    _,
                    "draft_notes",
                    "bulk_publish",
                ],
            ) => {
                let ids = update(&id, |s| {
                    let drafts = std::mem::take(&mut s.drafts);
                    Ok(drafts
                        .into_iter()
                        .map(|d| new_thread(s, d.note, d.position).id)
                        .collect::<Vec<_>>())
                })?;
                if !ids.is_empty() {
                    queue(AgentJob {
                        review: id,
                        threads: ids,
                        reply: false,
                    });
                }
                Answer::One(Value::Null)
            }
            (
                "POST",
                [
                    "projects",
                    _,
                    "merge_requests",
                    _,
                    what @ ("approve" | "unapprove"),
                ],
            ) => {
                let approve = *what == "approve";
                update(&id, |s| {
                    s.approved = approve;
                    Ok(())
                })?;
                Answer::One(json!({}))
            }
            ("PUT", ["projects", _, "merge_requests", _]) => {
                if let Some(t) = body["title"].as_str() {
                    let t = t.trim().to_string();
                    update(&id, |s| {
                        s.title = (!t.is_empty()).then_some(t);
                        Ok(())
                    })?;
                }
                Answer::One(self.snapshot(|s| self.mr(s))?)
            }
            ("POST" | "DELETE", ["projects", _, "merge_requests", _, "award_emoji", ..]) => {
                Answer::One(json!({}))
            }
            (_, ["graphql"]) => Answer::One(json!({ "data": {} })),
            _ => bail!("a local diff can't do that ({method} {route})"),
        };
        Ok(out)
    }

    fn route<'a>(&self, url: &'a str) -> anyhow::Result<&'a str> {
        let host = host_of(&self.id);
        let rest = url
            .strip_prefix(&host)
            .with_context(|| format!("{url} isn't on {host}"))?;
        Ok(rest
            .strip_prefix("/api/v4/")
            .or_else(|| rest.strip_prefix("/api/"))
            .unwrap_or(rest))
    }
}

/// A new thread in `s`, by you.
fn new_thread(s: &mut Store, body: String, position: Option<Pos>) -> Thread {
    let n = s.next();
    let id = format!(
        "{:x}",
        hash(&format!("{}-{n}-{}", s.id, now_iso())) & 0xffff_ffff
    );
    let t = Thread {
        id,
        individual_note: false,
        notes: vec![StoredNote {
            id: n,
            kind: Some(
                if position.is_some() {
                    "DiffNote"
                } else {
                    "DiscussionNote"
                }
                .into(),
            ),
            body,
            author: me(),
            created_at: now_iso(),
            resolvable: true,
            position: position.map(|mut p| {
                if p.position_type.is_empty() {
                    p.position_type = "text".into();
                }
                p
            }),
            ..Default::default()
        }],
    };
    s.threads.push(t.clone());
    t
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                Some(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                None => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

enum Answer {
    One(Value),
    List(Vec<Value>),
    Bytes(Vec<u8>),
    Missing,
}

impl Answer {
    fn response(self) -> Response {
        match self {
            Answer::One(v) => Response {
                status: 200,
                body: v.to_string().into_bytes(),
                ..Default::default()
            },
            Answer::List(v) => Response {
                status: 200,
                total: Some(v.len() as u64),
                body: Value::Array(v).to_string().into_bytes(),
                ..Default::default()
            },
            Answer::Bytes(b) => Response {
                status: 200,
                body: b,
                ..Default::default()
            },
            Answer::Missing => Response {
                status: 404,
                body: br#"{"message":"404 Not found"}"#.to_vec(),
                ..Default::default()
            },
        }
    }
}

impl Transport for LocalGitLab {
    fn get(&self, url: &str, _token: &str) -> anyhow::Result<Response> {
        let path = self.route(url)?;
        Ok(self.answer(path)?.response())
    }

    fn send(&self, method: &str, url: &str, _token: &str, body: &[u8]) -> anyhow::Result<Response> {
        let path = self.route(url)?;
        let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        Ok(self.write(method, path, &body)?.response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs() {
        assert_eq!(
            parse_spec(""),
            Spec {
                base: "HEAD".into(),
                head: None,
                merge_base: false
            }
        );
        assert_eq!(
            parse_spec("main"),
            Spec {
                base: "main".into(),
                head: None,
                merge_base: false
            }
        );
        assert_eq!(
            parse_spec("main..x"),
            Spec {
                base: "main".into(),
                head: Some("x".into()),
                merge_base: false
            }
        );
        assert_eq!(
            parse_spec("main...x"),
            Spec {
                base: "main".into(),
                head: Some("x".into()),
                merge_base: true
            }
        );
        assert_eq!(
            parse_spec("..x"),
            Spec {
                base: "HEAD".into(),
                head: Some("x".into()),
                merge_base: false
            }
        );
        assert_eq!(
            parse_spec("--merge-base origin/main"),
            Spec {
                base: "origin/main".into(),
                head: None,
                merge_base: true
            }
        );
    }

    #[test]
    fn parses_git_diffs() {
        let text = "diff --git a/src/a b.rs b/src/a b.rs\nindex 111..222 100644\n--- a/src/a b.rs\n+++ b/src/a b.rs\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n\
diff --git a/old.txt b/new.txt\nsimilarity index 90%\nrename from old.txt\nrename to new.txt\n\
diff --git a/gone b/gone\ndeleted file mode 100644\nindex 333..000\n--- a/gone\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n\
diff --git a/img.png b/img.png\nnew file mode 100644\nindex 000..444\nBinary files /dev/null and b/img.png differ\n";
        let f = parse_git_diff(text);
        assert_eq!(f.len(), 4);
        assert_eq!(f[0]["new_path"], "src/a b.rs");
        assert_eq!(f[0]["diff"], "@@ -1,2 +1,2 @@\n a\n-b\n+c\n");
        assert_eq!(
            (
                f[1]["old_path"].as_str(),
                f[1]["new_path"].as_str(),
                f[1]["renamed_file"].as_bool()
            ),
            (Some("old.txt"), Some("new.txt"), Some(true))
        );
        assert_eq!(
            (f[2]["old_path"].as_str(), f[2]["deleted_file"].as_bool()),
            (Some("gone"), Some(true))
        );
        assert!(f[3]["diff"].as_str().unwrap().starts_with("Binary files"));
        assert_eq!(f[3]["new_file"], true);
    }

    #[test]
    fn decodes_paths() {
        assert_eq!(decode("src%2Fa%20b.rs"), "src/a b.rs");
        assert_eq!(decode("100%"), "100%");
    }
}
