//! Merge requests kept between tabs and across restarts, so one opens at
//! once (then refreshes quietly) wherever it was loaded before: in another
//! tab, by abromux's background poller (`src/gitlab` in the app), or before
//! the last quit.
//!
//! - In memory, [`Shared`] (a gpui global): the latest [`Review`] of each
//!   merge request loaded lately. A tab showing one picks up a newer one
//!   put there by anyone ([`GitLabView`](crate::view::GitLabView) observes it).
//! - On disk: the API's answers a load got, recorded by [`fetch`]. Loading
//!   them again with [`replay`] builds the same review without the network.
//!   That keeps this module out of the review's shape: whatever
//!   [`load_review`] asks for is what's kept.
//!
//! Big diffs are capped as they're fetched ([`Limits`]): files past them come
//! as GitLab's "too large" ones do, and are fetched when shown.

use crate::{
    api::{Client, Response, Transport},
    load_review,
    review::Review,
};
use gpui::{App, Global};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

/// Reviews kept in memory besides those tabs hold.
const IN_MEMORY: usize = 16;
/// The recording's format; another one is ignored.
const VERSION: u32 = 1;

static DIR: OnceLock<PathBuf> = OnceLock::new();
static LIMITS: Mutex<Option<Limits>> = Mutex::new(None);

/// Keep recordings in `dir` (abromux: `gitlab-cache/` in its support folder).
/// Without it nothing goes to disk.
pub fn init(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// How much of a merge request's diff a load takes; past it, files come
/// without their diff and are fetched when shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub file_bytes: usize,
    pub mr_bytes: usize,
    pub mr_files: usize,
}

pub fn set_limits(limits: Option<Limits>) {
    *LIMITS.lock().unwrap_or_else(|e| e.into_inner()) = limits;
}

fn limits() -> Option<Limits> {
    *LIMITS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A host as a key: `https://gitlab.com/` and `https://GitLab.com` are one.
pub fn host_key(host: &str) -> String {
    host.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// What a review's freshness is judged by, the same from GitLab's GraphQL
/// (abromux's cheap checks) as from a loaded review: when GitLab last
/// touched it (comments do), its head commit, its pipeline's status, and
/// its comment count.
pub fn signature(updated_at: &str, head_sha: &str, pipeline: &str, notes: u64) -> String {
    let at =
        crate::time::parse(updated_at).map_or_else(|| updated_at.to_string(), |t| t.to_string());
    format!("{at}|{head_sha}|{}|{notes}", pipeline.to_ascii_lowercase())
}

pub fn review_signature(r: &Review) -> String {
    let mr = &r.mr;
    signature(
        &mr.updated_at,
        mr.diff_refs.as_ref().map_or("", |d| d.head_sha.as_str()),
        mr.head_pipeline.as_ref().map_or("", |p| p.status.as_str()),
        mr.user_notes_count as u64,
    )
}

/// `group/project` of a loaded review.
pub fn review_path(r: &Review) -> String {
    r.mr.references
        .full
        .rsplit_once('!')
        .map_or("", |(p, _)| p)
        .to_string()
}

// ---- in memory -------------------------------------------------------------

pub struct Entry {
    pub review: Rc<Review>,
    /// When it was fetched (a replayed one: when it was put here).
    pub fetched: Instant,
    /// Bigger is newer; each put gets the next.
    pub serial: u64,
    /// Its recording's size on disk (0: not recorded).
    pub bytes: u64,
    pub replayed: bool,
}

type Key = (String, u64, u64);

/// The reviews loaded lately, newest per merge request.
#[derive(Default)]
pub struct Shared {
    entries: HashMap<Key, Entry>,
    order: VecDeque<Key>,
    /// Project ids by (host, `group/project`), learnt from what's loaded and
    /// from abromux's checks, so a link's merge request is found by path.
    paths: HashMap<(String, String), u64>,
    serial: u64,
}

impl Global for Shared {}

impl Shared {
    pub fn get(&self, host: &str, project: u64, iid: u64) -> Option<&Entry> {
        self.entries.get(&(host_key(host), project, iid))
    }

    pub fn project_id(&self, host: &str, path: &str) -> Option<u64> {
        self.paths
            .get(&(host_key(host), path.to_ascii_lowercase()))
            .copied()
    }

    /// Everything put since `serial` (exclusive), oldest first.
    pub fn since(&self, serial: u64) -> impl Iterator<Item = (&str, &Entry)> {
        let mut v: Vec<(&str, &Entry)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.serial > serial)
            .map(|((h, _, _), e)| (h.as_str(), e))
            .collect();
        v.sort_by_key(|(_, e)| e.serial);
        v.into_iter()
    }

    pub fn serial(&self) -> u64 {
        self.serial
    }
}

pub fn shared(cx: &App) -> Option<&Shared> {
    cx.try_global::<Shared>()
}

/// `review` as the latest of its merge request. An older one than what's
/// there (a slow load landing after a newer one) is dropped.
pub fn put(
    cx: &mut App,
    host: &str,
    review: Rc<Review>,
    fetched: Instant,
    bytes: u64,
    replayed: bool,
) {
    let key = (host_key(host), review.mr.project_id, review.mr.iid);
    let path = review_path(&review).to_ascii_lowercase();
    let s = cx.default_global::<Shared>();
    if let Some(old) = s.entries.get(&key)
        && old.fetched > fetched
    {
        return;
    }
    s.serial += 1;
    let serial = s.serial;
    if !path.is_empty() {
        s.paths.insert((key.0.clone(), path), key.1);
    }
    s.order.retain(|k| *k != key);
    s.order.push_back(key.clone());
    s.entries.insert(
        key,
        Entry {
            review,
            fetched,
            serial,
            bytes,
            replayed,
        },
    );
    while s.order.len() > IN_MEMORY {
        if let Some(k) = s.order.pop_front() {
            s.entries.remove(&k);
        }
    }
}

pub fn learn_project(cx: &mut App, host: &str, path: &str, id: u64) {
    if id != 0 && !path.is_empty() {
        cx.default_global::<Shared>()
            .paths
            .insert((host_key(host), path.to_ascii_lowercase()), id);
    }
}

/// Drop merge request `iid` of `project`: from memory and disk.
pub fn forget(cx: &mut App, host: &str, project: u64, iid: u64) {
    let key = (host_key(host), project, iid);
    if let Some(s) = cx.try_global::<Shared>()
        && s.entries.contains_key(&key)
    {
        let s = cx.global_mut::<Shared>();
        s.entries.remove(&key);
        s.order.retain(|k| *k != key);
    }
    forget_file(host, project, iid);
}

/// Delete the recording of `iid` of `project`, leaving memory as it is.
pub fn forget_file(host: &str, project: u64, iid: u64) {
    if let Some(p) = file(host, project, iid) {
        let _ = std::fs::remove_file(p);
    }
}

// ---- on disk ---------------------------------------------------------------

/// One answer as kept: the body as text (the API answers JSON).
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
    body: String,
}

#[derive(Serialize, Deserialize)]
struct Recording {
    version: u32,
    host: String,
    project: u64,
    iid: u64,
    /// When it was made, seconds since the epoch.
    saved_at: i64,
    /// By URL; GraphQL's by `POST <url> <body>`.
    answers: Vec<(String, Saved)>,
}

fn dir() -> Option<PathBuf> {
    DIR.get().map(|d| d.join("reviews"))
}

/// The cache's folder (`gitlab-cache/`), once `init` has set it.
pub(crate) fn root() -> Option<PathBuf> {
    DIR.get().cloned()
}

fn file(host: &str, project: u64, iid: u64) -> Option<PathBuf> {
    let h: String = host_key(host)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(dir()?.join(h).join(format!("{project}-{iid}.json")))
}

/// Every recording on disk: (host, project, iid, bytes).
pub fn saved() -> Vec<(String, u64, u64, u64)> {
    let Some(dir) = dir() else { return vec![] };
    let mut out = vec![];
    for host in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        for f in std::fs::read_dir(host.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = f.file_name().to_string_lossy().into_owned();
            let Some((p, i)) = name.strip_suffix(".json").and_then(|n| n.split_once('-')) else {
                continue;
            };
            let (Ok(p), Ok(i)) = (p.parse(), i.parse()) else {
                continue;
            };
            let bytes = f.metadata().map_or(0, |m| m.len());
            // The host as recorded (the folder's name lost its punctuation).
            let host = read(&f.path()).map(|r| r.host).unwrap_or_default();
            out.push((host, p, i, bytes));
        }
    }
    out
}

fn read(path: &std::path::Path) -> Option<Recording> {
    let r: Recording = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (r.version == VERSION).then_some(r)
}

/// When the recording of `iid` was made, if there is one.
pub fn saved_at(host: &str, project: u64, iid: u64) -> Option<i64> {
    read(&file(host, project, iid)?).map(|r| r.saved_at)
}

/// Load merge request `iid` of `project` as [`load_review`] does, and keep
/// what GitLab answered on disk. Blocks. Gives the review and the
/// recording's size.
pub fn fetch(c: &Client, project: u64, iid: u64) -> anyhow::Result<(Review, u64)> {
    let rec = Arc::new(Recorder {
        inner: c.transport(),
        limits: limits(),
        got: Default::default(),
        sizes: Default::default(),
    });
    let r = load_review(&c.with_transport(rec.clone()), project, iid)?;
    let answers = std::mem::take(&mut *rec.got.lock().unwrap_or_else(|e| e.into_inner()));
    let bytes = save(&Recording {
        version: VERSION,
        host: host_key(c.host()),
        project: r.mr.project_id,
        iid: r.mr.iid,
        saved_at: crate::time::now(),
        answers,
    })
    .unwrap_or(0);
    Ok((r, bytes))
}

fn save(rec: &Recording) -> Option<u64> {
    let path = file(&rec.host, rec.project, rec.iid)?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    let bytes = serde_json::to_vec(rec).ok()?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &bytes).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(bytes.len() as u64)
}

/// The review last [`fetch`]ed of `iid`, from disk, with no network. Blocks.
pub fn replay(host: &str, project: u64, iid: u64) -> Option<(Review, u64)> {
    let path = file(host, project, iid)?;
    let bytes = std::fs::metadata(&path).ok()?.len();
    let rec = read(&path)?;
    let replay = Arc::new(Replay(rec.answers.into_iter().collect()));
    let c = Client::new(&rec.host, "", replay);
    load_review(&c, project, iid).ok().map(|r| (r, bytes))
}

/// Passes requests on, keeping their answers, and caps diffs.
struct Recorder {
    inner: Arc<dyn Transport>,
    limits: Option<Limits>,
    got: Mutex<Vec<(String, Saved)>>,
    /// Diff bytes and files taken so far.
    sizes: Mutex<(usize, usize)>,
}

impl Recorder {
    fn keep(&self, key: String, r: anyhow::Result<Response>) -> anyhow::Result<Response> {
        let mut r = r?;
        if let Some(l) = self.limits
            && r.status == 200
            && is_diffs(&key)
            && let Some(body) = self.cap(&r.body, l)
        {
            r.body = body;
        }
        if let Ok(body) = std::str::from_utf8(&r.body) {
            let saved = Saved {
                status: r.status,
                next_page: r.next_page,
                total: r.total,
                body: body.to_string(),
            };
            self.got
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((key, saved));
        }
        Ok(r)
    }

    /// A page of a merge request's diffs with what's past `l` left out, as
    /// GitLab leaves out what's past its own limits. None: unchanged.
    fn cap(&self, body: &[u8], l: Limits) -> Option<Vec<u8>> {
        let mut files: Vec<serde_json::Value> = serde_json::from_slice(body).ok()?;
        let mut sizes = self.sizes.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed = false;
        for f in &mut files {
            let len = f["diff"].as_str().map_or(0, str::len);
            sizes.1 += 1;
            if len > l.file_bytes || sizes.0 + len > l.mr_bytes || sizes.1 > l.mr_files {
                f["diff"] = "".into();
                f["too_large"] = true.into();
                changed = true;
            } else {
                sizes.0 += len;
            }
        }
        changed.then(|| serde_json::to_vec(&files).ok()).flatten()
    }
}

/// `…/merge_requests/<iid>/diffs?…`: the review's diff, a page of it.
fn is_diffs(url: &str) -> bool {
    url.split('?')
        .next()
        .is_some_and(|p| p.ends_with("/diffs") && p.contains("/merge_requests/"))
}

fn graphql_key(url: &str, body: &[u8]) -> String {
    format!("POST {url} {}", String::from_utf8_lossy(body))
}

impl Transport for Recorder {
    fn get(&self, url: &str, token: &str) -> anyhow::Result<Response> {
        self.keep(url.to_string(), self.inner.get(url, token))
    }

    fn get_many(&self, urls: &[String], token: &str) -> Vec<anyhow::Result<Response>> {
        let got = self.inner.get_many(urls, token);
        urls.iter()
            .zip(got)
            .map(|(u, r)| self.keep(u.clone(), r))
            .collect()
    }

    fn send(&self, method: &str, url: &str, token: &str, body: &[u8]) -> anyhow::Result<Response> {
        let r = self.inner.send(method, url, token, body);
        // A load only reads; GraphQL is a POST that reads.
        if method == "POST" && url.ends_with("/api/graphql") {
            return self.keep(graphql_key(url, body), r);
        }
        r
    }
}

/// Answers from a recording; anything it doesn't have fails.
struct Replay(HashMap<String, Saved>);

impl Replay {
    fn answer(&self, key: &str) -> anyhow::Result<Response> {
        let s = self
            .0
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("not in the cache: {key}"))?;
        Ok(Response {
            status: s.status,
            next_page: s.next_page,
            total: s.total,
            body: s.body.clone().into_bytes(),
            ..Default::default()
        })
    }
}

impl Transport for Replay {
    fn get(&self, url: &str, _: &str) -> anyhow::Result<Response> {
        self.answer(url)
    }

    fn send(&self, _: &str, url: &str, _: &str, body: &[u8]) -> anyhow::Result<Response> {
        self.answer(&graphql_key(url, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_match_across_apis() {
        // GraphQL has no milliseconds and shouts the status.
        assert_eq!(
            signature("2026-11-06T10:20:30Z", "abc", "RUNNING", 3),
            signature("2026-11-06T10:20:30.123Z", "abc", "running", 3)
        );
        assert_ne!(
            signature("2026-11-06T10:20:30Z", "abc", "", 3),
            signature("2026-11-06T10:20:31Z", "abc", "", 3)
        );
    }

    #[test]
    fn caps_diffs_past_the_limits() {
        let rec = Recorder {
            inner: Arc::new(Replay(HashMap::new())),
            limits: None,
            got: Default::default(),
            sizes: Default::default(),
        };
        let l = Limits {
            file_bytes: 10,
            mr_bytes: 25,
            mr_files: 4,
        };
        let page = |diffs: &[&str]| {
            serde_json::to_vec(
                &diffs
                    .iter()
                    .map(|d| serde_json::json!({ "new_path": "f", "diff": d }))
                    .collect::<Vec<_>>(),
            )
            .unwrap()
        };
        let out = |b: Option<Vec<u8>>| -> Vec<(String, bool)> {
            let v: Vec<serde_json::Value> = serde_json::from_slice(&b.unwrap()).unwrap();
            v.iter()
                .map(|f| {
                    (
                        f["diff"].as_str().unwrap().to_string(),
                        f["too_large"].as_bool().unwrap_or(false),
                    )
                })
                .collect()
        };
        // Too big a file; then the merge request's bytes run out on page 2,
        // and its file count.
        let got = out(rec.cap(&page(&["12345", "this one is too big", "1234567890"]), l));
        assert_eq!(
            got,
            [
                ("12345".into(), false),
                ("".into(), true),
                ("1234567890".into(), false)
            ]
        );
        // 25 bytes in 4 files: just fits; a 5th file doesn't.
        let got = out(rec.cap(&page(&["1234567890", "1"]), l));
        assert_eq!(got, [("1234567890".into(), false), ("".into(), true)]);
        assert_eq!(rec.cap(&page(&[]), l), None);
        assert!(is_diffs(
            "http://h/api/v4/projects/1/merge_requests/2/diffs?per_page=100&page=1"
        ));
        assert!(!is_diffs(
            "http://h/api/v4/projects/1/merge_requests/2/discussions?page=1"
        ));
    }

    #[test]
    fn replays_what_was_recorded() {
        let rec = Recorder {
            inner: Arc::new(Replay(
                [(
                    "http://h/api/v4/user".to_string(),
                    Saved {
                        status: 200,
                        next_page: None,
                        total: None,
                        body: "{}".into(),
                    },
                )]
                .into_iter()
                .collect(),
            )),
            limits: None,
            got: Default::default(),
            sizes: Default::default(),
        };
        assert!(rec.get("http://h/api/v4/user", "t").is_ok());
        assert!(rec.get("http://h/api/v4/nope", "t").is_err());
        let got = rec.got.lock().unwrap();
        assert_eq!(got.len(), 1);
        let replay = Replay(got.iter().cloned().collect());
        assert_eq!(replay.get("http://h/api/v4/user", "").unwrap().body, b"{}");
    }
}
