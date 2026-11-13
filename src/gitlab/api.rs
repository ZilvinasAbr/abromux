//! Talking to GitLab through the GitLab plugin's client
//! (`abromux_gitlab::api`: `/usr/bin/curl`, token on stdin), every request
//! paced by the shared [`Throttle`]: a request made while it's paused fails
//! at once without going out, and each answer's rate-limit headers go to it.

use super::model::{Light, MrKey};
use super::throttle::Throttle;
use abromux_gitlab::api::{ApiError, Client, Response, Transport};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// 429, or a pause from an earlier one.
    Throttled,
    /// 401/403, or no token at all.
    Auth(String),
    NotFound,
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Throttled => write!(f, "rate-limited by GitLab"),
            Error::Auth(m) => write!(f, "{m}"),
            Error::NotFound => write!(f, "not found"),
            Error::Other(m) => write!(f, "{m}"),
        }
    }
}

impl Error {
    /// What the plugin's client said, as one of these: by the status in its
    /// [`ApiError`], or the throttle's [`Paused`].
    pub fn of(e: &anyhow::Error) -> Error {
        if e.chain().any(|c| c.is::<Paused>()) {
            return Error::Throttled;
        }
        Error::of_status(ApiError::status_of(e), format!("{e:#}"))
    }

    /// An answer's `status` (None: no answer at all), with `message` for
    /// the kinds that show one.
    fn of_status(status: Option<u16>, message: String) -> Error {
        match status {
            Some(401 | 403) => Error::Auth(message),
            Some(404) => Error::NotFound,
            Some(429) => Error::Throttled,
            _ => Error::Other(message),
        }
    }
}

/// A request not sent because the throttle is paused.
#[derive(Debug)]
struct Paused;

impl std::fmt::Display for Paused {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("paused for GitLab's rate limit")
    }
}

impl std::error::Error for Paused {}

/// A transport that keeps to the throttle.
pub struct Paced {
    pub inner: Arc<dyn Transport>,
    pub throttle: Arc<Mutex<Throttle>>,
}

impl Paced {
    fn check(&self) -> anyhow::Result<()> {
        if self
            .throttle
            .lock()
            .unwrap()
            .paused(Instant::now())
            .is_some()
        {
            return Err(Paused.into());
        }
        Ok(())
    }

    fn observe(&self, r: &anyhow::Result<Response>) {
        let Ok(r) = r else { return };
        let now = Instant::now();
        let mut t = self.throttle.lock().unwrap();
        if r.status == 429 {
            t.throttled(r.retry_after.map(Duration::from_secs), now);
        }
        if let (Some(rem), Some(lim)) = (r.rate_remaining, r.rate_limit) {
            t.remaining(rem, lim, now);
        }
    }
}

impl Transport for Paced {
    fn get(&self, url: &str, token: &str) -> anyhow::Result<Response> {
        self.check()?;
        let r = self.inner.get(url, token);
        self.observe(&r);
        r
    }

    fn get_many(&self, urls: &[String], token: &str) -> Vec<anyhow::Result<Response>> {
        if self.check().is_err() {
            return urls.iter().map(|_| Err(Paused.into())).collect();
        }
        let got = self.inner.get_many(urls, token);
        for r in &got {
            self.observe(r);
        }
        got
    }

    fn send(&self, method: &str, url: &str, token: &str, body: &[u8]) -> anyhow::Result<Response> {
        self.check()?;
        let r = self.inner.send(method, url, token, body);
        self.observe(&r);
        r
    }
}

/// A GraphQL query's `data`. Errors with no data at all fail it.
pub fn graphql(c: &Client, query: &str) -> Result<Value, Error> {
    let url = format!("{}/api/graphql", c.host());
    let body = json!({ "query": query }).to_string();
    let r = c
        .transport()
        .send("POST", &url, c.token(), body.as_bytes())
        .map_err(|e| Error::of(&e))?;
    if !(200..=299).contains(&r.status) {
        let message = match r.status {
            401 => format!("{} didn't accept the token", c.host()),
            403 => "the token can't read that (needs read_api)".into(),
            s => format!(
                "GitLab answered {s}: {}",
                String::from_utf8_lossy(&r.body)
                    .chars()
                    .take(200)
                    .collect::<String>()
            ),
        };
        return Err(Error::of_status(Some(r.status), message));
    }
    let v: Value = serde_json::from_slice(&r.body)
        .map_err(|e| Error::Other(format!("GitLab sent something that isn't JSON: {e}")))?;
    let errors = v["errors"].as_array().filter(|e| !e.is_empty());
    if v["data"].is_null() || v["data"].as_object().is_some_and(|o| o.is_empty()) {
        let msg = errors
            .and_then(|e| e[0]["message"].as_str())
            .unwrap_or("GitLab's GraphQL answered nothing");
        return Err(Error::Other(msg.to_string()));
    }
    Ok(v["data"].clone())
}

// ---- the cheap check ---------------------------------------------------

/// What the cheap check asks of every merge request. A GitLab too old for
/// some of it gets [`MIN_FIELDS`].
pub const FIELDS: &str = "iid title webUrl state draft sourceBranch targetBranch updatedAt \
    author { username } headPipeline { status } detailedMergeStatus approved \
    userNotesCount diffRefs { baseSha headSha startSha } project { id fullPath }";
pub const MIN_FIELDS: &str = "iid title webUrl state sourceBranch targetBranch updatedAt \
    author { username } headPipeline { status } userNotesCount \
    diffRefs { baseSha headSha startSha } project { id fullPath }";

/// Which lists the check takes along.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lists {
    pub mine: bool,
    pub review: bool,
    pub assigned: bool,
    /// Open merge requests from these branches, per project (the
    /// workspaces' checkouts).
    pub branches: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Checked {
    /// The merge requests asked for; None for ones GitLab doesn't have.
    pub mrs: Vec<(MrKey, Option<Light>)>,
    /// Some when the lists were asked for.
    pub authored: Option<Vec<(MrKey, Light)>>,
    pub review: Option<Vec<(MrKey, Light)>>,
    pub assigned: Option<Vec<(MrKey, Light)>>,
    /// Per project in `Lists::branches`.
    pub branches: Vec<(String, Vec<(MrKey, Light)>)>,
}

pub fn light_query(keys: &[MrKey], lists: Option<&Lists>, fields: &str) -> String {
    let s = |v: &str| serde_json::to_string(v).unwrap_or_default();
    let mut q = String::from("query {\n");
    for (i, k) in keys.iter().enumerate() {
        q.push_str(&format!(
            "  m{i}: project(fullPath: {}) {{ mergeRequest(iid: \"{}\") {{ ...F }} }}\n",
            s(&k.project),
            k.iid
        ));
    }
    if let Some(l) = lists {
        let list = |name: &str, field: &str| {
            format!("    {name}: {field}(state: opened, first: 100) {{ nodes {{ ...F }} }}\n")
        };
        if l.mine || l.review || l.assigned {
            q.push_str("  me: currentUser {\n");
            if l.mine {
                q.push_str(&list("authored", "authoredMergeRequests"));
            }
            if l.review {
                q.push_str(&list("review", "reviewRequestedMergeRequests"));
            }
            if l.assigned {
                q.push_str(&list("assigned", "assignedMergeRequests"));
            }
            q.push_str("  }\n");
        }
        for (j, (project, branches)) in l.branches.iter().enumerate() {
            let branches: Vec<String> = branches.iter().map(|b| s(b)).collect();
            q.push_str(&format!(
                "  p{j}: project(fullPath: {}) {{ mergeRequests(state: opened, sourceBranches: [{}], first: 20) {{ nodes {{ ...F }} }} }}\n",
                s(project),
                branches.join(", ")
            ));
        }
    }
    q.push_str(&format!("}}\nfragment F on MergeRequest {{ {fields} }}\n"));
    q
}

fn lower(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_ascii_lowercase())
}

/// A GraphQL merge request node.
pub fn parse_light(v: &Value) -> Option<(MrKey, Light)> {
    let project = v["project"]["fullPath"].as_str()?;
    let iid = v["iid"].as_str()?.parse().ok()?;
    let light = Light {
        title: v["title"].as_str().unwrap_or_default().into(),
        web_url: v["webUrl"].as_str().unwrap_or_default().into(),
        state: v["state"].as_str().unwrap_or_default().into(),
        draft: v["draft"].as_bool().unwrap_or_else(|| {
            v["title"]
                .as_str()
                .is_some_and(|t| t.starts_with("Draft:") || t.starts_with("WIP:"))
        }),
        author: v["author"]["username"].as_str().unwrap_or_default().into(),
        source_branch: v["sourceBranch"].as_str().unwrap_or_default().into(),
        target_branch: v["targetBranch"].as_str().unwrap_or_default().into(),
        updated_at: v["updatedAt"].as_str().unwrap_or_default().into(),
        pipeline: lower(&v["headPipeline"]["status"]),
        merge_status: lower(&v["detailedMergeStatus"]),
        approved: v["approved"].as_bool(),
        notes: v["userNotesCount"].as_u64().unwrap_or(0),
        head_sha: v["diffRefs"]["headSha"].as_str().map(Into::into),
        base_sha: v["diffRefs"]["baseSha"].as_str().map(Into::into),
        start_sha: v["diffRefs"]["startSha"].as_str().map(Into::into),
        // `gid://gitlab/Project/12`
        project_id: v["project"]["id"]
            .as_str()
            .and_then(|g| g.rsplit('/').next()?.parse().ok())
            .unwrap_or(0),
    };
    Some((MrKey::new(project, iid), light))
}

fn nodes(v: &Value) -> Vec<(MrKey, Light)> {
    v["nodes"]
        .as_array()
        .map(|a| a.iter().filter_map(parse_light).collect())
        .unwrap_or_default()
}

pub fn parse_checked(data: &Value, keys: &[MrKey], lists: Option<&Lists>) -> Checked {
    let mut c = Checked::default();
    for (i, k) in keys.iter().enumerate() {
        let node = &data[format!("m{i}")]["mergeRequest"];
        c.mrs.push((k.clone(), parse_light(node).map(|(_, l)| l)));
    }
    if let Some(l) = lists {
        let me = &data["me"];
        if l.mine {
            c.authored = Some(nodes(&me["authored"]));
        }
        if l.review {
            c.review = Some(nodes(&me["review"]));
        }
        if l.assigned {
            c.assigned = Some(nodes(&me["assigned"]));
        }
        for (j, (project, _)) in l.branches.iter().enumerate() {
            c.branches.push((
                project.clone(),
                nodes(&data[format!("p{j}")]["mergeRequests"]),
            ));
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_reads_the_check() {
        let keys = [MrKey::new("g/p", 3), MrKey::new("g/q", 9)];
        let lists = Lists {
            mine: true,
            review: true,
            assigned: false,
            branches: vec![("g/p".into(), vec!["ws/x".into()])],
        };
        let q = light_query(&keys, Some(&lists), FIELDS);
        assert!(q.contains(r#"m1: project(fullPath: "g/q") { mergeRequest(iid: "9")"#));
        assert!(q.contains("reviewRequestedMergeRequests"));
        assert!(!q.contains("assignedMergeRequests"));
        assert!(q.contains(r#"sourceBranches: ["ws/x"]"#));
        let node = json!({
            "iid": "3", "title": "Fix", "webUrl": "https://g/x", "state": "opened",
            "draft": true, "sourceBranch": "ws/x", "targetBranch": "main",
            "updatedAt": "2026-11-06T10:00:00Z", "author": { "username": "me" },
            "headPipeline": { "status": "RUNNING" }, "detailedMergeStatus": "MERGEABLE",
            "approved": false, "userNotesCount": 2,
            "diffRefs": { "baseSha": "b", "headSha": "h", "startSha": "s" },
            "project": { "id": "gid://gitlab/Project/12", "fullPath": "g/p" }
        });
        let data = json!({
            "m0": { "mergeRequest": node.clone() },
            "m1": null,
            "me": { "authored": { "nodes": [node.clone()] }, "review": { "nodes": [] } },
            "p0": { "mergeRequests": { "nodes": [node] } }
        });
        let c = parse_checked(&data, &keys, Some(&lists));
        let l = c.mrs[0].1.as_ref().unwrap();
        assert_eq!(l.pipeline.as_deref(), Some("running"));
        assert!(l.pipeline_active());
        assert_eq!(l.merge_status.as_deref(), Some("mergeable"));
        assert_eq!(l.head_sha.as_deref(), Some("h"));
        assert_eq!(l.project_id, 12);
        assert_eq!(c.mrs[1], (MrKey::new("g/q", 9), None));
        assert_eq!(c.authored.unwrap().len(), 1);
        assert_eq!(c.review.unwrap().len(), 0);
        assert!(c.assigned.is_none());
        assert_eq!(c.branches[0].1[0].0, MrKey::new("g/p", 3));
    }

    #[test]
    fn errors_by_status_not_wording() {
        let api = |status| abromux_gitlab_status(status).context("while checking");
        assert!(matches!(Error::of(&api(401)), Error::Auth(_)));
        assert!(matches!(Error::of(&api(403)), Error::Auth(_)));
        assert_eq!(Error::of(&api(404)), Error::NotFound);
        assert_eq!(Error::of(&api(429)), Error::Throttled);
        assert!(matches!(Error::of(&api(500)), Error::Other(_)));
        assert_eq!(Error::of(&anyhow::Error::from(Paused)), Error::Throttled);
        // A message that merely mentions a status is not that status.
        assert!(matches!(
            Error::of(&anyhow::anyhow!("curl: (401) something")),
            Error::Other(_)
        ));
    }

    /// The plugin client's error for `status`, as its `check` makes it.
    fn abromux_gitlab_status(status: u16) -> anyhow::Error {
        let t = Arc::new(Canned(status));
        Client::new("http://gl.test", "t", t)
            .current_user()
            .unwrap_err()
    }

    struct Canned(u16);

    impl Transport for Canned {
        fn get(&self, _: &str, _: &str) -> anyhow::Result<Response> {
            Ok(Response {
                status: self.0,
                ..Default::default()
            })
        }
    }
}
