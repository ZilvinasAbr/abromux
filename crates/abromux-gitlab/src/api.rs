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

/// One HTTP response.
#[derive(Clone, Debug, Default)]
pub struct Response {
    pub status: u16,
    /// `X-Next-Page`, for paginated lists.
    pub next_page: Option<u32>,
    /// `X-Total`: how many items the whole list has (GitLab leaves it out
    /// past 10,000).
    pub total: Option<u64>,
    pub body: Vec<u8>,
}

pub trait Transport: Send + Sync {
    /// GET `url` with the `PRIVATE-TOKEN` header set to `token`.
    fn get(&self, url: &str, token: &str) -> anyhow::Result<Response>;

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
}

impl Client {
    pub fn new(host: &str, token: &str, transport: Arc<dyn Transport>) -> Self {
        Self {
            host: host.trim_end_matches('/').to_string(),
            token: token.to_string(),
            transport,
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// GET `path` (after `/api/v4/`) as `T`.
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let r = self.fetch(path)?;
        decode(&r.body, path)
    }

    /// Every page of the list at `path`.
    pub fn get_all<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<Vec<T>> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut all = vec![];
        let mut page = 1;
        loop {
            let paged = format!("{path}{sep}per_page=100&page={page}");
            let r = self.fetch(&paged)?;
            all.extend(decode::<Vec<T>>(&r.body, &paged)?);
            match r.next_page {
                // A broken server repeating a page would loop forever.
                Some(n) if n > page && page < 100 => page = n,
                _ => return Ok(all),
            }
        }
    }

    fn fetch(&self, path: &str) -> anyhow::Result<Response> {
        let url = format!("{}/api/v4/{path}", self.host);
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
            403 => bail!("GitLab didn't allow this (403): the token needs the api scope to comment or approve"),
            _ => self.check(r, &url)?,
        };
        let body = if r.body.trim_ascii().is_empty() { b"null".as_slice() } else { &r.body };
        decode(body, path)
    }

    fn check(&self, r: Response, url: &str) -> anyhow::Result<Response> {
        match r.status {
            200..=299 => Ok(r),
            401 => bail!("{} rejected the token (401): check it has the api scope (read_api to only read)", self.host),
            403 => bail!("not allowed (403): {url}"),
            404 => bail!("not found (404): {url}"),
            s => bail!("GitLab answered {s} for {url}: {}", snippet(&r.body)),
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

    /// The same, in `state`: opened, merged, closed or all.
    pub fn merge_requests_in(&self, scope: Scope, state: &str) -> anyhow::Result<Vec<MergeRequest>> {
        let q = match scope {
            Scope::ToReview(me) => format!("reviewer_username={me}&scope=all"),
            Scope::Mine => "scope=created_by_me".into(),
            Scope::All => "scope=all".into(),
        };
        self.get_all(&format!("merge_requests?state={state}&order_by=created_at&with_labels_details=true&{q}"))
    }

    /// How many merge requests `merge_requests_in` would list, from one
    /// short page's `X-Total`; None when GitLab doesn't say.
    pub fn count_merge_requests(&self, scope: &Scope, state: &str) -> anyhow::Result<Option<u64>> {
        let q = match scope {
            Scope::ToReview(me) => format!("reviewer_username={me}&scope=all"),
            Scope::Mine => "scope=created_by_me".into(),
            Scope::All => "scope=all".into(),
        };
        Ok(self.fetch(&format!("merge_requests?state={state}&per_page=1&{q}"))?.total)
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
        self.get_all(&format!("projects/{project}/merge_requests/{iid}/discussions"))
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
        let all: Vec<Pipeline> = self.get(&format!("projects/{project}/merge_requests/{iid}/pipelines?per_page=1"))?;
        Ok(all.into_iter().next())
    }

    /// The project's default branch.
    pub fn default_branch(&self, project: u64) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        struct Project {
            #[serde(default)]
            default_branch: String,
        }
        Ok(self.get::<Project>(&format!("projects/{project}"))?.default_branch)
    }

    /// The project's labels, with their colors.
    pub fn labels(&self, project: u64) -> anyhow::Result<Vec<Label>> {
        self.get_all(&format!("projects/{project}/labels"))
    }

    /// Whether the merge request takes several assignees and several
    /// reviewers: GitLab's paid tiers do, the free one takes one of each
    /// (and its REST API quietly keeps the first). Only GraphQL says.
    pub fn allows_multiple(&self, full_path: &str, iid: u64) -> anyhow::Result<(bool, bool)> {
        let q = format!(
            "{{ project(fullPath: {full_path:?}) {{ mergeRequest(iid: \"{iid}\") {{ allowsMultipleAssignees allowsMultipleReviewers }} }} }}"
        );
        let url = format!("{}/api/graphql", self.host);
        let r = self.transport.send("POST", &url, &self.token, serde_json::json!({ "query": q }).to_string().as_bytes())?;
        let v: serde_json::Value = decode(&r.body, &url)?;
        let mr = &v["data"]["project"]["mergeRequest"];
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
            Err(e) if format!("{e:#}").contains("(404)") => Ok(None),
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
        self.get(&format!("projects/{project}/merge_requests/{iid}/approvals"))
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
    }
}
