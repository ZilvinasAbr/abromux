//! The GitLab REST API v4: reading a review, and writing comments, replies,
//! resolutions and approvals.
//!
//! Requests go through a [`Transport`]: [`Curl`] in the app (as abromux's
//! updater does: no TLS stack of our own, and it trusts what macOS trusts,
//! so self-hosted GitLabs behind a company CA work), canned responses in
//! tests. Calls block; run them off the main thread.

use crate::types::*;
use anyhow::{Context as _, bail};
use serde::de::DeserializeOwned;
use std::{io::Write as _, process::Command, sync::Arc};

/// GitLab answered with an error status. Callers tell a rejected token, a
/// missing page or a rate limit apart by [`status`](Self::status), never by
/// the wording of the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    message: String,
}

impl ApiError {
    fn new(status: u16, message: String) -> Self {
        ApiError { status, message }
    }

    /// The status GitLab gave, anywhere in `e`'s chain (contexts included).
    pub fn status_of(e: &anyhow::Error) -> Option<u16> {
        e.chain().find_map(|c| c.downcast_ref::<ApiError>()).map(|a| a.status)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// One HTTP response.
#[derive(Clone, Debug, Default)]
pub struct Response {
    pub status: u16,
    /// `X-Next-Page`, for paginated lists.
    pub next_page: Option<u32>,
    /// `X-Total`: how many items the whole list has (GitLab leaves it out
    /// past 10,000).
    pub total: Option<u64>,
    /// `Retry-After` (seconds) on a 429, and `RateLimit-Remaining` /
    /// `RateLimit-Limit`: for whoever paces requests (abromux's cache).
    pub retry_after: Option<u64>,
    pub rate_remaining: Option<u64>,
    pub rate_limit: Option<u64>,
    pub body: Vec<u8>,
}

pub trait Transport: Send + Sync {
    /// GET `url` with the `PRIVATE-TOKEN` header set to `token`.
    fn get(&self, url: &str, token: &str) -> anyhow::Result<Response>;

    /// GET each of `urls`, as many at once as the transport can: the
    /// answers in the same order. One at a time unless it knows better.
    fn get_many(&self, urls: &[String], token: &str) -> Vec<anyhow::Result<Response>> {
        urls.iter().map(|u| self.get(u, token)).collect()
    }

    /// `method` (POST, PUT, DELETE) `url` with a JSON `body`.
    fn send(&self, method: &str, url: &str, token: &str, body: &[u8]) -> anyhow::Result<Response> {
        let _ = (url, token, body);
        bail!("{method} isn't supported here")
    }
}

/// `/usr/bin/curl`. The token goes in on stdin, so it never shows in `ps`.
pub struct Curl;

impl Transport for Curl {
    fn get(&self, url: &str, token: &str) -> anyhow::Result<Response> {
        curl(&[], url, token)
    }

    /// One curl for all of them: up to `PARALLEL` at once, over connections
    /// it keeps open and reuses (one multiplexed connection over HTTP/2), so
    /// a far-away GitLab costs a round trip or two, not one handshake and
    /// round trip per request.
    fn get_many(&self, urls: &[String], token: &str) -> Vec<anyhow::Result<Response>> {
        // A review's first batch at once (HTTP/2 takes them all on one
        // connection anyway; HTTP/1.1 opens this many at most).
        const PARALLEL: &str = "20";
        if urls.len() < 2 {
            return urls.iter().map(|u| self.get(u, token)).collect();
        }
        let fail = |e: &anyhow::Error| urls.iter().map(|_| Err(anyhow::anyhow!("{e:#}"))).collect();
        let dir = match batch_dir() {
            Ok(d) => d,
            Err(e) => return fail(&e),
        };
        let run = || -> anyhow::Result<Vec<anyhow::Result<Response>>> {
            let mut cmd = Command::new("/usr/bin/curl");
            cmd.args(["-sS", "--compressed", "--connect-timeout", "10", "--max-time", "60"])
                .args(["--parallel", "--parallel-max", PARALLEL])
                .args(["-A", "abromux-gitlab", "-H", "@-"])
                // One line a transfer: which file it went to, and what we use of its headers.
                .args(["-w", "%{filename_effective}\t%{http_code}\t%header{x-next-page}\t%header{x-total}\t%{errormsg}\t%header{retry-after}\t%header{ratelimit-remaining}\t%header{ratelimit-limit}\n"]);
            for (i, url) in urls.iter().enumerate() {
                cmd.arg("-o").arg(dir.join(i.to_string())).arg("--url").arg(url);
            }
            let mut child = cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .context("running curl")?;
            {
                let mut stdin = child.stdin.take().context("curl stdin")?;
                writeln!(stdin, "PRIVATE-TOKEN: {token}")?;
            }
            let out = child.wait_with_output()?;
            let mut got: Vec<Option<anyhow::Result<Response>>> = urls.iter().map(|_| None).collect();
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let f: Vec<&str> = line.split('\t').collect();
                let Some(i) = f.first().and_then(|p| p.rsplit('/').next()).and_then(|n| n.parse::<usize>().ok()) else { continue };
                let Some(slot) = got.get_mut(i) else { continue };
                let status: u16 = f.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                *slot = Some(if status == 0 {
                    Err(anyhow::anyhow!("{}", f.get(4).copied().filter(|e| !e.is_empty()).unwrap_or("no answer")))
                } else {
                    Ok(Response {
                        status,
                        next_page: f.get(2).and_then(|v| v.trim().parse().ok()),
                        total: f.get(3).and_then(|v| v.trim().parse().ok()),
                        retry_after: f.get(5).and_then(|v| v.trim().parse().ok()),
                        rate_remaining: f.get(6).and_then(|v| v.trim().parse().ok()),
                        rate_limit: f.get(7).and_then(|v| v.trim().parse().ok()),
                        body: std::fs::read(dir.join(i.to_string())).unwrap_or_default(),
                    })
                });
            }
            let stderr = String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("curl: ").to_string();
            Ok(got
                .into_iter()
                .map(|g| g.unwrap_or_else(|| Err(anyhow::anyhow!("{}", if stderr.is_empty() { "no answer" } else { &stderr }))))
                .collect())
        };
        let got = run();
        let _ = std::fs::remove_dir_all(&dir);
        got.unwrap_or_else(|e| fail(&e))
    }

    fn send(&self, method: &str, url: &str, token: &str, body: &[u8]) -> anyhow::Result<Response> {
        // The body goes through a file only we can read: stdin carries the token.
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("abromux-gitlab-{}-{n}.json", std::process::id()));
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
            f.write_all(body)?;
        }
        let data = format!("@{}", path.display());
        let out = curl(&["-X", method, "-H", "Content-Type: application/json", "--data-binary", &data], url, token);
        let _ = std::fs::remove_file(&path);
        out
    }
}

/// A fresh directory only we can read, for one batch's answers.
fn batch_dir() -> anyhow::Result<std::path::PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("abromux-gitlab-{}-batch-{n}", std::process::id()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

fn curl(extra: &[&str], url: &str, token: &str) -> anyhow::Result<Response> {
    {
        let mut child = Command::new("/usr/bin/curl")
            .args(["-sS", "-i", "--compressed", "--connect-timeout", "10", "--max-time", "60"])
            .args(extra)
            .args(["-A", "abromux-gitlab", "-H", "@-", "--", url])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("running curl")?;
        {
            let mut stdin = child.stdin.take().context("curl stdin")?;
            writeln!(stdin, "PRIVATE-TOKEN: {token}")?;
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("curl: "));
        }
        parse_response(&out.stdout)
    }
}

/// Split curl's `-i` output into status, the headers we use, and body. A
/// proxy's or `100 Continue`'s header block can come before the real one.
fn parse_response(raw: &[u8]) -> anyhow::Result<Response> {
    let mut rest = raw;
    loop {
        let end = find(rest, b"\r\n\r\n").context("no HTTP header in response")?;
        let head = String::from_utf8_lossy(&rest[..end]).into_owned();
        rest = &rest[end + 4..];
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .context("bad HTTP status line")?;
        let another = rest.starts_with(b"HTTP/");
        if (status == 100 || status == 200 && head.contains("Connection established")) && another {
            continue;
        }
        let header = |name: &str| {
            head.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
            })
        };
        return Ok(Response {
            status,
            next_page: header("x-next-page").and_then(|v| v.parse().ok()),
            total: header("x-total").and_then(|v| v.parse().ok()),
            retry_after: header("retry-after").and_then(|v| v.parse().ok()),
            rate_remaining: header("ratelimit-remaining").and_then(|v| v.parse().ok()),
            rate_limit: header("ratelimit-limit").and_then(|v| v.parse().ok()),
            body: rest.to_vec(),
        });
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A GitLab host and the token to use there.
#[derive(Clone)]
pub struct Client {
    host: String,
    token: String,
    transport: Arc<dyn Transport>,
    /// Answers fetched ahead in one batch (`prefetch`), by URL, each taken
    /// by the first call that asks for it.
    ahead: Arc<std::sync::Mutex<std::collections::HashMap<String, Response>>>,
}

impl Client {
    pub fn new(host: &str, token: &str, transport: Arc<dyn Transport>) -> Self {
        Self {
            host: host.trim_end_matches('/').to_string(),
            token: token.to_string(),
            transport,
            ahead: Default::default(),
        }
    }

    /// Fetch `paths` (after `/api/v4/`) together, for the calls that ask
    /// for them next. A list's first page is [`first_page`] of its path.
    /// Failures aren't kept: the call that wants it tries again and says why.
    pub fn prefetch(&self, paths: impl IntoIterator<Item = String>) {
        let urls: Vec<String> = {
            let ahead = self.ahead.lock().unwrap_or_else(|e| e.into_inner());
            let mut urls: Vec<String> = paths.into_iter().map(|p| self.url(&p)).filter(|u| !ahead.contains_key(u)).collect();
            urls.dedup();
            urls
        };
        let got = self.transport.get_many(&urls, &self.token);
        let mut ahead = self.ahead.lock().unwrap_or_else(|e| e.into_inner());
        for (url, r) in urls.into_iter().zip(got) {
            if let Ok(r) = r {
                ahead.insert(url, r);
            }
        }
    }

    /// The same host and token over another transport (nothing fetched ahead).
    pub fn with_transport(&self, transport: Arc<dyn Transport>) -> Self {
        Self::new(&self.host, &self.token, transport)
    }

    pub fn transport(&self) -> Arc<dyn Transport> {
        self.transport.clone()
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Drop what was fetched ahead and not asked for.
    pub fn forget_prefetched(&self) {
        self.ahead.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v4/{path}", self.host)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// GET `path` (after `/api/v4/`) as `T`.
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let r = self.fetch(path)?;
        decode(&r.body, path)
    }

    /// Every page of the list at `path`. Once the first says how many there
    /// are, the rest come in one batch.
    pub fn get_all<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<Vec<T>> {
        let mut all = vec![];
        let mut page = 1;
        loop {
            let this_page = paged(path, page);
            let r = self.fetch(&this_page)?;
            if page == 1
                && let Some(total) = r.total
            {
                let pages = total.div_ceil(100).min(100) as u32;
                if pages > 1 {
                    self.prefetch((2..=pages).map(|p| paged(path, p)));
                }
            }
            all.extend(decode::<Vec<T>>(&r.body, &this_page)?);
            match r.next_page {
                // A broken server repeating a page would loop forever.
                Some(n) if n > page && page < 100 => page = n,
                _ => return Ok(all),
            }
        }
    }

    fn fetch(&self, path: &str) -> anyhow::Result<Response> {
        let url = self.url(path);
        if let Some(r) = self.ahead.lock().unwrap_or_else(|e| e.into_inner()).remove(&url) {
            return self.check(r, &url);
        }
        let r = self
            .transport
            .get(&url, &self.token)
            .with_context(|| format!("GET {url}"))?;
        self.check(r, &url)
    }

    /// `method` `path` with `body`, the answer as `T`.
    fn write<T: DeserializeOwned>(&self, method: &str, path: &str, body: serde_json::Value) -> anyhow::Result<T> {
        let url = format!("{}/api/v4/{path}", self.host);
        let r = self
            .transport
            .send(method, &url, &self.token, body.to_string().as_bytes())
            .with_context(|| format!("{method} {url}"))?;
        let r = match r.status {
            // A read_api token can read but not write.
            403 => {
                return Err(ApiError::new(403, "GitLab didn't allow this (403): the token needs the api scope to comment or approve".into()).into())
            }
            _ => self.check(r, &url)?,
        };
        let body = if r.body.trim_ascii().is_empty() { b"null".as_slice() } else { &r.body };
        decode(body, path)
    }

    fn check(&self, r: Response, url: &str) -> anyhow::Result<Response> {
        match r.status {
            200..=299 => Ok(r),
            401 => Err(ApiError::new(401, format!("{} rejected the token (401): check it has the api scope (read_api to only read)", self.host)).into()),
            403 => Err(ApiError::new(403, format!("not allowed (403): {url}")).into()),
            404 => Err(ApiError::new(404, format!("not found (404): {url}")).into()),
            s => Err(ApiError::new(s, format!("GitLab answered {s} for {url}: {}", snippet(&r.body))).into()),
        }
    }

    pub fn version(&self) -> anyhow::Result<Version> {
        self.get("version")
    }

    pub fn current_user(&self) -> anyhow::Result<User> {
        self.get("user")
    }

    /// Open merge requests the user can see where they're a reviewer, the
    /// author, or (`all`) any.
    pub fn merge_requests(&self, scope: Scope) -> anyhow::Result<Vec<MergeRequest>> {
        self.merge_requests_in(scope, "opened")
    }

    /// What GitLab's "Search or go to…" finds for `query`: `group/project!12`
    /// that merge request; `!12` those numbered 12 in `projects` (a number
    /// only means something within a project); else merge requests anywhere
    /// you can see with it in their title or description.
    pub fn search_merge_requests(&self, query: &str, projects: &[u64]) -> anyhow::Result<Vec<MergeRequest>> {
        let q = query.trim();
        let enc = |s: &str| -> String {
            s.bytes()
                .map(|b| match b {
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
                    _ => format!("%{b:02X}"),
                })
                .collect()
        };
        if let Some((path, n)) = q.rsplit_once('!')
            && let Ok(iid) = n.parse::<u64>()
        {
            if !path.is_empty() {
                return Ok(vec![self.get(&format!("projects/{}/merge_requests/{iid}", enc(path)))?]);
            }
            let paths: Vec<String> = projects.iter().map(|p| format!("projects/{p}/merge_requests/{iid}")).collect();
            self.prefetch(paths.clone());
            let found = paths.iter().filter_map(|p| self.get(p).ok()).collect();
            return Ok(found);
        }
        self.get(&format!(
            "merge_requests?scope=all&state=all&order_by=updated_at&per_page=20&with_labels_details=true&search={}",
            enc(q)
        ))
    }

    /// The same, in `state`: opened, merged, closed or all.
    pub fn merge_requests_in(&self, scope: Scope, state: &str) -> anyhow::Result<Vec<MergeRequest>> {
        self.get_all(&list_path(&scope, state))
    }

    /// How many merge requests `merge_requests_in` would list, from one
    /// short page's `X-Total`; None when GitLab doesn't say.
    pub fn count_merge_requests(&self, scope: &Scope, state: &str) -> anyhow::Result<Option<u64>> {
        Ok(self.fetch(&count_path(scope, state))?.total)
    }

    /// The projects you're a member of, most recently active first (the
    /// first hundred), or those matching `search`.
    pub fn projects(&self, search: &str) -> anyhow::Result<Vec<Project>> {
        self.get(&projects_path(search))
    }

    /// The id of the project at `path` (`group/project`).
    pub fn project_id(&self, path: &str) -> anyhow::Result<u64> {
        #[derive(serde::Deserialize)]
        struct Project {
            id: u64,
        }
        let path = path.trim_matches('/').replace('/', "%2F");
        Ok(self.get::<Project>(&format!("projects/{path}"))?.id)
    }

    pub fn merge_request(&self, project: u64, iid: u64) -> anyhow::Result<MergeRequest> {
        self.get(&format!("projects/{project}/merge_requests/{iid}?include_diverged_commits_count=true"))
    }

    pub fn diffs(&self, project: u64, iid: u64) -> anyhow::Result<Vec<FileDiff>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/diffs"))
    }

    pub fn discussions(&self, project: u64, iid: u64) -> anyhow::Result<Vec<Discussion>> {
        self.get_all(&discussions_path(project, iid))
    }

    /// When it was closed, reopened or merged, and by whom.
    pub fn label_events(&self, project: u64, iid: u64) -> anyhow::Result<Vec<LabelEvent>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/resource_label_events"))
    }

    pub fn milestone_events(&self, project: u64, iid: u64) -> anyhow::Result<Vec<MilestoneEvent>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/resource_milestone_events"))
    }

    pub fn state_events(&self, project: u64, iid: u64) -> anyhow::Result<Vec<StateEvent>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/resource_state_events"))
    }

    /// The merge request's versions, newest first.
    pub fn versions(&self, project: u64, iid: u64) -> anyhow::Result<Vec<MrVersion>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/versions"))
    }

    /// One version's diffs against the target branch.
    pub fn version_diffs(&self, project: u64, iid: u64, version: u64) -> anyhow::Result<Vec<FileDiff>> {
        Ok(self.get::<Diffs>(&format!("projects/{project}/merge_requests/{iid}/versions/{version}"))?.diffs)
    }

    /// What one commit changed.
    pub fn commit_diff(&self, project: u64, sha: &str) -> anyhow::Result<Vec<FileDiff>> {
        self.get_all(&format!("projects/{project}/repository/commits/{sha}/diff"))
    }

    /// The diffs between two commits (one version against another).
    pub fn compare(&self, project: u64, from: &str, to: &str) -> anyhow::Result<Vec<FileDiff>> {
        Ok(self.get::<Diffs>(&format!("projects/{project}/repository/compare?from={from}&to={to}&straight=true"))?.diffs)
    }

    // ---- writing ------------------------------------------------------

    /// A comment on the merge request (the Overview's "Add a comment").
    pub fn comment(&self, project: u64, iid: u64, body: &str) -> anyhow::Result<Note> {
        self.write("POST", &format!("projects/{project}/merge_requests/{iid}/notes"), serde_json::json!({ "body": body }))
    }

    /// A reply in thread `discussion`.
    pub fn reply(&self, project: u64, iid: u64, discussion: &str, body: &str) -> anyhow::Result<Note> {
        let path = format!("projects/{project}/merge_requests/{iid}/discussions/{discussion}/notes");
        self.write("POST", &path, serde_json::json!({ "body": body }))
    }

    /// Resolve or unresolve thread `discussion`.
    pub fn resolve(&self, project: u64, iid: u64, discussion: &str, resolved: bool) -> anyhow::Result<Discussion> {
        let path = format!("projects/{project}/merge_requests/{iid}/discussions/{discussion}?resolved={resolved}");
        self.write("PUT", &path, serde_json::json!({}))
    }

    /// A new thread on a line of the diff at `refs`: `new_line` for an added
    /// or unchanged line, `old_line` for a removed (or unchanged) one.
    #[allow(clippy::too_many_arguments)]
    /// A thread on a line, as `line_comment` starts, but held in your
    /// review until it's submitted (GitLab's "Start a review").
    #[allow(clippy::too_many_arguments)]
    pub fn draft_line_comment(
        &self,
        project: u64,
        iid: u64,
        refs: &DiffRefs,
        old_path: &str,
        new_path: &str,
        old_line: Option<u32>,
        new_line: Option<u32>,
        body: &str,
    ) -> anyhow::Result<DraftNote> {
        let mut position = serde_json::json!({
            "position_type": "text",
            "base_sha": refs.base_sha,
            "start_sha": refs.start_sha,
            "head_sha": refs.head_sha,
            "old_path": old_path,
            "new_path": new_path,
        });
        if let Some(o) = old_line {
            position["old_line"] = o.into();
        }
        if let Some(n) = new_line {
            position["new_line"] = n.into();
        }
        let path = format!("projects/{project}/merge_requests/{iid}/draft_notes");
        self.write("POST", &path, serde_json::json!({ "note": body, "position": position }))
    }

    /// Your review's comments not yet submitted.
    pub fn draft_notes(&self, project: u64, iid: u64) -> anyhow::Result<Vec<DraftNote>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/draft_notes"))
    }

    /// Submit your review: every pending comment at once.
    pub fn publish_drafts(&self, project: u64, iid: u64) -> anyhow::Result<()> {
        let _: serde_json::Value =
            self.write("POST", &format!("projects/{project}/merge_requests/{iid}/draft_notes/bulk_publish"), serde_json::json!({}))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn line_comment(
        &self,
        project: u64,
        iid: u64,
        refs: &DiffRefs,
        old_path: &str,
        new_path: &str,
        old_line: Option<u32>,
        new_line: Option<u32>,
        body: &str,
    ) -> anyhow::Result<Discussion> {
        let mut position = serde_json::json!({
            "position_type": "text",
            "base_sha": refs.base_sha,
            "start_sha": refs.start_sha,
            "head_sha": refs.head_sha,
            "old_path": old_path,
            "new_path": new_path,
        });
        if let Some(o) = old_line {
            position["old_line"] = o.into();
        }
        if let Some(n) = new_line {
            position["new_line"] = n.into();
        }
        let path = format!("projects/{project}/merge_requests/{iid}/discussions");
        self.write("POST", &path, serde_json::json!({ "body": body, "position": position }))
    }

    /// Approve, or take back the approval.
    pub fn approve(&self, project: u64, iid: u64, approve: bool) -> anyhow::Result<()> {
        let what = if approve { "approve" } else { "unapprove" };
        let _: serde_json::Value = self.write("POST", &format!("projects/{project}/merge_requests/{iid}/{what}"), serde_json::json!({}))?;
        Ok(())
    }

    /// The issues merging it closes.
    pub fn closes_issues(&self, project: u64, iid: u64) -> anyhow::Result<Vec<Issue>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/closes_issues"))
    }

    /// Each reviewer's review state.
    pub fn reviewers(&self, project: u64, iid: u64) -> anyhow::Result<Vec<Reviewer>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/reviewers"))
    }

    /// The emoji reactions on the merge request.
    pub fn awards(&self, project: u64, iid: u64) -> anyhow::Result<Vec<AwardEmoji>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/award_emoji"))
    }

    /// React with `name` (thumbsup, tada).
    pub fn award(&self, project: u64, iid: u64, name: &str) -> anyhow::Result<()> {
        let _: serde_json::Value =
            self.write("POST", &format!("projects/{project}/merge_requests/{iid}/award_emoji"), serde_json::json!({ "name": name }))?;
        Ok(())
    }

    /// Take back reaction `id`.
    pub fn unaward(&self, project: u64, iid: u64, id: u64) -> anyhow::Result<()> {
        let _: serde_json::Value =
            self.write("DELETE", &format!("projects/{project}/merge_requests/{iid}/award_emoji/{id}"), serde_json::json!({}))?;
        Ok(())
    }

    /// The emoji reactions on each comment of the merge request, by note id
    /// (only notes with some). REST has them per note only, so one GraphQL
    /// query a hundred notes. By the project's id, as [`Self::allows_multiple`].
    pub fn note_awards(&self, project: u64, iid: u64) -> anyhow::Result<std::collections::HashMap<u64, Vec<AwardEmoji>>> {
        let url = format!("{}/api/graphql", self.host);
        let mut out = std::collections::HashMap::new();
        let mut after = String::new();
        // A thousand comments is plenty; past that, the rest go without.
        for _ in 0..10 {
            let q = format!(
                "{{ projects(ids: [\"gid://gitlab/Project/{project}\"]) {{ nodes {{ mergeRequest(iid: \"{iid}\") {{ \
                 notes(first: 100{after}) {{ pageInfo {{ hasNextPage endCursor }} \
                 nodes {{ id awardEmoji {{ nodes {{ name user {{ id username name avatarUrl }} }} }} }} }} }} }} }} }}"
            );
            let r = self.transport.send("POST", &url, &self.token, serde_json::json!({ "query": q }).to_string().as_bytes())?;
            let v: serde_json::Value = decode(&r.body, &url)?;
            let notes = &v["data"]["projects"]["nodes"][0]["mergeRequest"]["notes"];
            let nodes = notes["nodes"].as_array().context("no notes in GitLab's answer")?;
            out.extend(nodes.iter().filter_map(note_awards_of));
            match notes["pageInfo"]["endCursor"].as_str().filter(|_| notes["pageInfo"]["hasNextPage"].as_bool() == Some(true)) {
                Some(cursor) => after = format!(", after: {cursor:?}"),
                None => break,
            }
        }
        Ok(out)
    }

    /// React to comment `note` with `name`.
    pub fn note_award(&self, project: u64, iid: u64, note: u64, name: &str) -> anyhow::Result<()> {
        let path = format!("projects/{project}/merge_requests/{iid}/notes/{note}/award_emoji");
        let _: serde_json::Value = self.write("POST", &path, serde_json::json!({ "name": name }))?;
        Ok(())
    }

    /// Take back user `me`'s `name` on comment `note`. GraphQL doesn't give
    /// a reaction's id, so this looks it up first.
    pub fn note_unaward(&self, project: u64, iid: u64, note: u64, name: &str, me: u64) -> anyhow::Result<()> {
        let path = format!("projects/{project}/merge_requests/{iid}/notes/{note}/award_emoji");
        let all: Vec<AwardEmoji> = self.get_all(&path)?;
        let a = all.iter().find(|a| a.name == name && a.user.id == me).context("that reaction is gone already")?;
        let _: serde_json::Value = self.write("DELETE", &format!("{path}/{}", a.id), serde_json::json!({}))?;
        Ok(())
    }

    /// Rebase the source branch onto the target, as GitLab's "Rebase source
    /// branch" does. GitLab answers at once and rebases in the background.
    pub fn rebase(&self, project: u64, iid: u64) -> anyhow::Result<()> {
        let _: serde_json::Value = self.write("PUT", &format!("projects/{project}/merge_requests/{iid}/rebase"), serde_json::json!({}))?;
        Ok(())
    }

    /// The merge request's pipelines, newest first: each commit's status.
    pub fn pipelines(&self, project: u64, iid: u64) -> anyhow::Result<Vec<Pipeline>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/pipelines"))
    }

    /// The latest pipeline that ran on commit `sha`, if any.
    pub fn pipeline_for_sha(&self, project: u64, sha: &str) -> anyhow::Result<Option<Pipeline>> {
        let all: Vec<Pipeline> = self.get(&format!("projects/{project}/pipelines?sha={sha}&per_page=1"))?;
        Ok(all.into_iter().next())
    }

    /// The merge request's latest pipeline, if it has one.
    pub fn latest_pipeline(&self, project: u64, iid: u64) -> anyhow::Result<Option<Pipeline>> {
        let all: Vec<Pipeline> = self.get(&latest_pipeline_path(project, iid))?;
        Ok(all.into_iter().next())
    }

    /// The project's default branch.
    pub fn default_branch(&self, project: u64) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        struct Project {
            #[serde(default)]
            default_branch: String,
        }
        Ok(self.get::<Project>(&project_path(project))?.default_branch)
    }

    /// The project's labels, with their colors.
    pub fn labels(&self, project: u64) -> anyhow::Result<Vec<Label>> {
        self.get_all(&format!("projects/{project}/labels"))
    }

    /// Whether the merge request takes several assignees and several
    /// reviewers: GitLab's paid tiers do, the free one takes one of each
    /// (and its REST API quietly keeps the first). Only GraphQL says.
    /// By the project's id, so it can be asked before the merge request
    /// (and its project's path) has come.
    pub fn allows_multiple(&self, project: u64, iid: u64) -> anyhow::Result<(bool, bool)> {
        let q = format!(
            "{{ projects(ids: [\"gid://gitlab/Project/{project}\"]) {{ nodes {{ mergeRequest(iid: \"{iid}\") {{ allowsMultipleAssignees allowsMultipleReviewers }} }} }} }}"
        );
        let url = format!("{}/api/graphql", self.host);
        let r = self.transport.send("POST", &url, &self.token, serde_json::json!({ "query": q }).to_string().as_bytes())?;
        let v: serde_json::Value = decode(&r.body, &url)?;
        let mr = &v["data"]["projects"]["nodes"][0]["mergeRequest"];
        let flag = |k: &str| mr[k].as_bool().with_context(|| format!("no {k} in GitLab's answer"));
        Ok((flag("allowsMultipleAssignees")?, flag("allowsMultipleReviewers")?))
    }

    /// Every changed file's path with lines added and removed, past the
    /// 1000 files the REST diff stops at. Only GraphQL lists them all.
    pub fn diff_stats(&self, full_path: &str, iid: u64) -> anyhow::Result<Vec<(String, usize, usize)>> {
        let q = format!(
            "{{ project(fullPath: {full_path:?}) {{ mergeRequest(iid: \"{iid}\") {{ diffStats {{ path additions deletions }} }} }} }}"
        );
        let url = format!("{}/api/graphql", self.host);
        let r = self.transport.send("POST", &url, &self.token, serde_json::json!({ "query": q }).to_string().as_bytes())?;
        let v: serde_json::Value = decode(&r.body, &url)?;
        let stats = v["data"]["project"]["mergeRequest"]["diffStats"].as_array().context("no diffStats in GitLab's answer")?;
        Ok(stats
            .iter()
            .filter_map(|s| {
                let n = |k: &str| s[k].as_u64().unwrap_or(0) as usize;
                Some((s["path"].as_str()?.to_string(), n("additions"), n("deletions")))
            })
            .collect())
    }

    /// A file's bytes at `sha`, or `None` when it isn't there (added or deleted).
    pub fn raw_maybe(&self, project: u64, path: &str, sha: &str) -> anyhow::Result<Option<Vec<u8>>> {
        match self.raw_bytes(project, path, sha) {
            Ok(b) => Ok(Some(b)),
            Err(e) if ApiError::status_of(&e) == Some(404) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The project's open milestones.
    pub fn milestones(&self, project: u64) -> anyhow::Result<Vec<Milestone>> {
        self.get_all(&format!("projects/{project}/milestones?state=active"))
    }

    /// Change the merge request's fields: `reviewer_ids`, `assignee_ids`,
    /// `labels` ("a,b"), `milestone_id` (0 for none). GitLab's sidebar.
    pub fn update(&self, project: u64, iid: u64, fields: serde_json::Value) -> anyhow::Result<MergeRequest> {
        self.write("PUT", &format!("projects/{project}/merge_requests/{iid}"), fields)
    }

    /// Everyone with access to the project, inherited too, with their role.
    pub fn members(&self, project: u64) -> anyhow::Result<Vec<Member>> {
        self.get_all(&format!("projects/{project}/members/all"))
    }

    /// The first hundred of them, and whether there are more: a big group
    /// has thousands, which the pickers search for instead.
    pub fn members_first(&self, project: u64) -> anyhow::Result<(Vec<Member>, bool)> {
        let path = first_page(&format!("projects/{project}/members/all"));
        let r = self.fetch(&path)?;
        let more = r.next_page.is_some() || r.total.is_some_and(|t| t > 100);
        Ok((decode(&r.body, &path)?, more))
    }

    /// One member's role, inherited too (404: not a member).
    pub fn member(&self, project: u64, user: u64) -> anyhow::Result<Member> {
        self.get(&member_path(project, user))
    }

    /// Members whose name or username has `query`, as a picker types it.
    pub fn search_members(&self, project: u64, query: &str) -> anyhow::Result<Vec<Member>> {
        let q: String = query
            .bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        self.get(&format!("projects/{project}/members/all?query={q}&per_page=20"))
    }

    /// A file's text at commit `sha`.
    pub fn raw_file(&self, project: u64, path: &str, sha: &str) -> anyhow::Result<String> {
        Ok(String::from_utf8_lossy(&self.raw_bytes(project, path, sha)?).into_owned())
    }

    /// A file's bytes at commit `sha`.
    pub fn raw_bytes(&self, project: u64, path: &str, sha: &str) -> anyhow::Result<Vec<u8>> {
        let path: String = path
            .bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        Ok(self.fetch(&format!("projects/{project}/repository/files/{path}/raw?ref={sha}"))?.body)
    }

    pub fn approvals(&self, project: u64, iid: u64) -> anyhow::Result<Approvals> {
        self.get(&approvals_path(project, iid))
    }

    pub fn commits(&self, project: u64, iid: u64) -> anyhow::Result<Vec<Commit>> {
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/commits"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    /// Where this username is a reviewer.
    ToReview(String),
    Mine,
    All,
    /// One project's, by id.
    Project(u64),
}

fn scope_query(scope: &Scope) -> String {
    match scope {
        Scope::ToReview(me) => format!("reviewer_username={me}&scope=all"),
        Scope::Mine => "scope=created_by_me".into(),
        Scope::All => "scope=all".into(),
        Scope::Project(id) => format!("project_id={id}"),
    }
}

/// The merge requests list in `scope` and `state` (all its pages: `get_all`).
pub fn list_path(scope: &Scope, state: &str) -> String {
    match scope {
        // A project's own list is under it.
        Scope::Project(id) => format!("projects/{id}/merge_requests?state={state}&order_by=created_at&with_labels_details=true"),
        s => format!("merge_requests?state={state}&order_by=created_at&with_labels_details=true&{}", scope_query(s)),
    }
}

/// One short page of that list: its `X-Total` counts it.
pub fn count_path(scope: &Scope, state: &str) -> String {
    match scope {
        Scope::Project(id) => format!("projects/{id}/merge_requests?state={state}&per_page=1"),
        s => format!("merge_requests?state={state}&per_page=1&{}", scope_query(s)),
    }
}

pub fn projects_path(search: &str) -> String {
    let mut p = "projects?membership=true&archived=false&order_by=last_activity_at&simple=true&per_page=100".to_string();
    if !search.trim().is_empty() {
        let q: String = search
            .trim()
            .bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        p.push_str(&format!("&search={q}&search_namespaces=true"));
    }
    p
}

pub fn approvals_path(project: u64, iid: u64) -> String {
    format!("projects/{project}/merge_requests/{iid}/approvals")
}

pub fn discussions_path(project: u64, iid: u64) -> String {
    format!("projects/{project}/merge_requests/{iid}/discussions")
}

pub fn latest_pipeline_path(project: u64, iid: u64) -> String {
    format!("projects/{project}/merge_requests/{iid}/pipelines?per_page=1")
}

pub fn project_path(project: u64) -> String {
    format!("projects/{project}")
}

pub fn member_path(project: u64, user: u64) -> String {
    format!("projects/{project}/members/all/{user}")
}

/// Page `page` of the list at `path`, as `get_all` asks for it.
pub fn paged(path: &str, page: u32) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}per_page=100&page={page}")
}

/// The first page of a list, as `get_all` asks for it: to `prefetch` it.
pub fn first_page(path: &str) -> String {
    paged(path, 1)
}

/// A project's merge requests page, `https://host/group/project/-/merge_requests[?…]`,
/// as (host URL, project path).
pub fn parse_project_mrs_url(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let (authority, path) = rest.split_once('/')?;
    let (project, after) = path.split_once("/-/merge_requests")?;
    let after = after.split(['?', '#']).next()?;
    (!project.is_empty() && after.trim_end_matches('/').is_empty()).then(|| (format!("{scheme}://{authority}"), project.to_string()))
}

/// A merge request's web URL, `https://host[:port]/group/project/-/merge_requests/12[/diffs…]`,
/// as (host URL, project path, iid). GitLab under a sub-path isn't told
/// apart from a group: the first path segment is taken as one.
pub fn parse_mr_url(url: &str) -> Option<(String, String, u64)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let (authority, path) = rest.split_once('/')?;
    let (project, after) = path.split_once("/-/merge_requests/")?;
    let iid = after.split(['/', '?', '#']).next()?.parse().ok()?;
    (!project.is_empty()).then(|| (format!("{scheme}://{authority}"), project.to_string(), iid))
}

/// One GraphQL note's reactions, `(note id, them)`, if it has some. Its
/// ids are global ("gid://gitlab/DiffNote/12"): the number is the REST id.
fn note_awards_of(n: &serde_json::Value) -> Option<(u64, Vec<AwardEmoji>)> {
    let gid = |v: &serde_json::Value| v.as_str()?.rsplit('/').next()?.parse::<u64>().ok();
    let awards: Vec<AwardEmoji> = n["awardEmoji"]["nodes"]
        .as_array()?
        .iter()
        .filter_map(|a| {
            let u = &a["user"];
            Some(AwardEmoji {
                id: 0,
                name: a["name"].as_str()?.to_string(),
                user: User {
                    id: gid(&u["id"])?,
                    username: u["username"].as_str().unwrap_or_default().to_string(),
                    name: u["name"].as_str().unwrap_or_default().to_string(),
                    avatar_url: u["avatarUrl"].as_str().map(str::to_string),
                },
            })
        })
        .collect();
    (!awards.is_empty()).then_some((gid(&n["id"])?, awards))
}

fn decode<T: DeserializeOwned>(body: &[u8], path: &str) -> anyhow::Result<T> {
    serde_json::from_slice(body)
        .with_context(|| format!("unexpected response to {path}: {}", snippet(body)))
}

fn snippet(body: &[u8]) -> String {
    let s = String::from_utf8_lossy(body);
    let s = s.trim();
    match s.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Answers from a list of (url suffix, response), and records the urls.
    #[derive(Default)]
    pub struct Canned {
        pub answers: Vec<(String, Response)>,
        pub asked: Mutex<Vec<String>>,
    }

    impl Transport for Canned {
        fn get(&self, url: &str, token: &str) -> anyhow::Result<Response> {
            assert_eq!(token, "t");
            self.asked.lock().unwrap().push(url.to_string());
            self.answers
                .iter()
                .find(|(suffix, _)| url.ends_with(suffix.as_str()))
                .map(|(_, r)| r.clone())
                .with_context(|| format!("no canned answer for {url}"))
        }
    }

    fn ok(body: &str, next: Option<u32>) -> Response {
        Response {
            status: 200,
            next_page: next,
            total: None,
            body: body.as_bytes().to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn parses_curl_output_with_a_continue_block() {
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nX-Next-Page: 3\r\nX-Total: 9\r\nContent-Type: application/json\r\n\r\n[1]";
        let r = parse_response(raw).unwrap();
        assert_eq!((r.status, r.next_page, r.total, r.body.as_slice()), (200, Some(3), Some(9), &b"[1]"[..]));
        let r = parse_response(b"HTTP/2 404 \r\nx-next-page: \r\n\r\n{}").unwrap();
        assert_eq!((r.status, r.next_page), (404, None));
    }

    #[test]
    fn parses_mr_urls() {
        assert_eq!(
            parse_mr_url("http://localhost:8929/alice/review-fixtures/-/merge_requests/1/diffs?x#y"),
            Some(("http://localhost:8929".into(), "alice/review-fixtures".into(), 1))
        );
        assert_eq!(
            parse_mr_url("https://gl.example.com/a/b/c/-/merge_requests/42"),
            Some(("https://gl.example.com".into(), "a/b/c".into(), 42))
        );
        assert_eq!(parse_mr_url("https://gl.example.com/a/b/-/issues/42"), None);
        assert_eq!(parse_mr_url("not a url"), None);
    }

    #[test]
    fn parses_project_merge_request_lists() {
        assert_eq!(
            parse_project_mrs_url("https://gl.example.com/a/b/-/merge_requests?state=merged"),
            Some(("https://gl.example.com".into(), "a/b".into()))
        );
        assert_eq!(parse_project_mrs_url("https://gl.example.com/a/b/-/merge_requests/"), Some(("https://gl.example.com".into(), "a/b".into())));
        // One merge request isn't the list.
        assert_eq!(parse_project_mrs_url("https://gl.example.com/a/b/-/merge_requests/3"), None);
        assert_eq!(parse_project_mrs_url("https://gl.example.com/a/b"), None);
    }

    #[test]
    fn search_reads_merge_request_references() {
        let mr = |p: u64| ok(&format!(r#"{{"iid":12,"project_id":{p},"title":"t"}}"#), None);
        let canned = Arc::new(Canned {
            answers: vec![
                ("projects/a%2Fb/merge_requests/12".into(), mr(9)),
                ("projects/3/merge_requests/12".into(), mr(3)),
                // Project 4 has no !12.
                ("projects/4/merge_requests/12".into(), Response { status: 404, ..Default::default() }),
                ("&search=rate%20limit".into(), ok("[]", None)),
            ],
            ..Default::default()
        });
        let c = Client::new("http://gl.test", "t", canned.clone());
        // group/project!12: that one.
        assert_eq!(c.search_merge_requests("a/b!12", &[]).unwrap()[0].project_id, 9);
        // !12: in each project at hand that has one.
        let found = c.search_merge_requests("!12", &[3, 4]).unwrap();
        assert_eq!(found.iter().map(|m| m.project_id).collect::<Vec<_>>(), [3]);
        // Words: GitLab's search, everywhere.
        assert!(c.search_merge_requests(" rate limit ", &[3]).unwrap().is_empty());
        assert!(canned.asked.lock().unwrap().last().unwrap().contains("merge_requests?scope=all&state=all"));
    }

    #[test]
    fn list_paths_by_scope() {
        assert_eq!(count_path(&Scope::Project(7), "opened"), "projects/7/merge_requests?state=opened&per_page=1");
        assert!(list_path(&Scope::ToReview("bob".into()), "merged").ends_with("&reviewer_username=bob&scope=all"));
        assert_eq!(first_page("x?a=1"), "x?a=1&per_page=100&page=1");
        assert_eq!(first_page("x"), "x?per_page=100&page=1");
    }

    #[test]
    fn prefetched_answers_are_used_once() {
        let canned = Arc::new(Canned {
            answers: vec![("version".into(), ok(r#"{"version":"19.5.3"}"#, None))],
            ..Default::default()
        });
        let c = Client::new("http://gl.test", "t", canned.clone());
        c.prefetch(["version".to_string()]);
        assert_eq!(canned.asked.lock().unwrap().len(), 1);
        // From what was fetched ahead: no second request.
        assert_eq!(c.version().unwrap().version, "19.5.3");
        assert_eq!(canned.asked.lock().unwrap().len(), 1);
        // Taken once: asked again, it's fetched again.
        c.version().unwrap();
        assert_eq!(canned.asked.lock().unwrap().len(), 2);
    }

    #[test]
    fn the_rest_of_a_list_comes_in_one_batch() {
        let page = |n: u32| Response { status: 200, next_page: (n < 3).then_some(n + 1), total: Some(250), body: format!("[{n}]").into_bytes(), ..Default::default() };
        let canned = Arc::new(Canned {
            answers: (1..=3).map(|n| (format!("&page={n}"), page(n))).collect(),
            ..Default::default()
        });
        let c = Client::new("http://gl.test", "t", canned.clone());
        let all: Vec<u32> = c.get_all("things?x=1").unwrap();
        assert_eq!(all, [1, 2, 3]);
        // Pages 2 and 3 were asked for together, after page 1, each once.
        let asked = canned.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 3);
        assert!(asked[0].ends_with("page=1") && asked[1].ends_with("page=2") && asked[2].ends_with("page=3"));
    }

    #[test]
    fn follows_pages() {
        let t = Arc::new(Canned {
            answers: vec![
                ("per_page=100&page=1".into(), ok("[1,2]", Some(2))),
                ("per_page=100&page=2".into(), ok("[3]", None)),
            ],
            ..Default::default()
        });
        let c = Client::new("http://gl.test/", "t", t.clone());
        assert_eq!(c.get_all::<u32>("things").unwrap(), [1, 2, 3]);
        assert_eq!(
            *t.asked.lock().unwrap(),
            [
                "http://gl.test/api/v4/things?per_page=100&page=1",
                "http://gl.test/api/v4/things?per_page=100&page=2"
            ]
        );
    }

    #[test]
    fn explains_errors() {
        let t = Arc::new(Canned {
            answers: vec![("user".into(), Response { status: 401, ..Default::default() })],
            ..Default::default()
        });
        let e = Client::new("http://gl.test", "t", t).current_user().unwrap_err();
        assert!(format!("{e:#}").contains("rejected the token"), "{e:#}");
        assert_eq!(ApiError::status_of(&e), Some(401));
    }

    /// Answers each GraphQL query by the first page that has no `after`,
    /// then the next: as GitLab pages a connection.
    struct Pages(Mutex<Vec<String>>);

    impl Transport for Pages {
        fn get(&self, url: &str, _: &str) -> anyhow::Result<Response> {
            bail!("no GET {url}")
        }
        fn send(&self, method: &str, url: &str, _: &str, body: &[u8]) -> anyhow::Result<Response> {
            assert_eq!((method, url), ("POST", "http://gl.test/api/graphql"));
            let q = String::from_utf8_lossy(body).to_string();
            self.0.lock().unwrap().push(q.clone());
            let page = if q.contains("after") {
                r#"{"data":{"projects":{"nodes":[{"mergeRequest":{"notes":{"pageInfo":{"hasNextPage":false,"endCursor":null},
                   "nodes":[{"id":"gid://gitlab/DiffNote/7","awardEmoji":{"nodes":[{"name":"eyes","user":{"id":"gid://gitlab/User/3","username":"carol","name":"Carol","avatarUrl":null}}]}}]}}}]}}}"#
            } else {
                r#"{"data":{"projects":{"nodes":[{"mergeRequest":{"notes":{"pageInfo":{"hasNextPage":true,"endCursor":"abc"},
                   "nodes":[{"id":"gid://gitlab/Note/5","awardEmoji":{"nodes":[{"name":"thumbsup","user":{"id":"gid://gitlab/User/2","username":"bob","name":"Bob"}},
                                                                           {"name":"tada","user":{"id":"gid://gitlab/User/3","username":"carol","name":"Carol"}}]}},
                            {"id":"gid://gitlab/Note/6","awardEmoji":{"nodes":[]}}]}}}]}}}"#
            };
            Ok(ok(page, None))
        }
    }

    #[test]
    fn note_awards_by_note_across_pages() {
        let t = Arc::new(Pages(Mutex::new(vec![])));
        let got = Client::new("http://gl.test", "t", t.clone()).note_awards(4, 9).unwrap();
        let names = |id: u64| got[&id].iter().map(|a| format!("{}:{}", a.user.username, a.name)).collect::<Vec<_>>();
        assert_eq!(names(5), ["bob:thumbsup", "carol:tada"]);
        assert_eq!(got[&5][0].user.id, 2);
        assert_eq!(names(7), ["carol:eyes"], "a diff note, from the second page");
        assert!(!got.contains_key(&6), "no reactions, no entry");
        let asked = t.0.lock().unwrap();
        assert_eq!(asked.len(), 2);
        assert!(asked[0].contains(r#"gid://gitlab/Project/4"#) && asked[0].contains(r#"iid: \"9\""#));
        assert!(asked[1].contains(r#"after: \"abc\""#), "{}", asked[1]);
    }
}
